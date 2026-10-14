//! whatsapp-tui: a lightweight WhatsApp client for the terminal. The
//! WhatsApp side lives in a Go helper (bridge/, whatsmeow) this binary
//! carries; this side is the TUI, fed by the helper's JSON lines. It also
//! tells other programs what is unread and takes their commands (`events`,
//! `ipc`, `cli`).

mod app;
mod attach;
mod avatar;
mod badge;
mod backend;
mod bigemoji;
mod account;
mod bridge;
mod chatmenu;
mod cli;
mod config;
mod divider;
mod drafts;
mod emoji;
mod emoji_panel;
mod events;
mod forward;
mod ipc;
mod links;
mod markup;
mod media;
mod msgmenu;
mod photo;
mod player;
mod react;
mod select;
mod ticks;
mod tray;
mod shortcuts;
mod theme;
mod ui;
mod update;
mod video;
mod viewer;

use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use std::io::IsTerminal;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub enum Ev {
    Term(event::Event),
    Bridge(bridge::Event),
    /// The clipboard read, files picked, drafts loaded: off the UI thread.
    Attach(attach::Ev),
    /// Where the audio playing is.
    Player(player::Ev),
    /// A video's mpv gone.
    Video(video::Ev),
    /// A photo (or a video's picture) read from disk, off the UI thread;
    /// None when it couldn't be.
    Picture(String, Option<image::DynamicImage>),
    /// A command from another process: the CLI, the bar widget, the tray.
    Remote(ipc::Cmd),
    /// A newer release, or how a check or an install went (`update`).
    Update(update::Note),
}

/// One cell's size in pixels, from the tty (TIOCGWINSZ), when it reports pixels.
fn cell_px() -> Option<(u16, u16)> {
    let s = crossterm::terminal::window_size().ok()?;
    (s.columns > 0 && s.rows > 0 && s.width > 0 && s.height > 0).then(|| (s.width / s.columns, s.height / s.rows))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-V" | "--version") => return println!("whatsapp-tui {}", env!("CARGO_PKG_VERSION")),
        Some("-h" | "--help") => return print!("{}", cli::usage()),
        Some("--new-instance") | None => {}
        Some(a) if a.starts_with('-') => {
            eprint!("whatsapp-tui: unknown option {a}\n{}", cli::usage());
            std::process::exit(2);
        }
        _ => {}
    }
    bridge::migrate();
    let start = match cli::run(&args) {
        Some(cli::Next::Exit(code)) => std::process::exit(code),
        Some(cli::Next::Start(cmd)) => cmd,
        None if args.is_empty() || args[0] == "--new-instance" => None,
        None => {
            eprint!("{}", cli::usage());
            std::process::exit(2);
        }
    };
    // One already running is brought to the front instead of starting
    // another; this one doesn't even need a terminal for that.
    let new_instance = args.first().map(String::as_str) == Some("--new-instance");
    let server = match if new_instance { None } else { ipc::claim() } {
        Some(ipc::Claim::Owner(s)) => Some(s),
        Some(ipc::Claim::Running) => {
            if let Err(e) = ipc::send(&serde_json::json!({ "cmd": "focus" })) {
                eprintln!("whatsapp-tui is already running, but didn't answer: {e}");
                std::process::exit(1);
            }
            // Only Hyprland lets us bring the other window up; elsewhere, say where it is.
            if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
                eprintln!("whatsapp-tui is already open in another window");
            }
            return;
        }
        None => None,
    };
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("whatsapp-tui: needs a terminal");
        std::process::exit(1);
    }
    // The one that owns the socket speaks for whatsapp-tui; a second one keeps quiet.
    let owner = server.is_some();
    let (tx, rx) = mpsc::channel();
    if let Some(s) = server {
        s.serve(tx.clone());
    }
    update::background(tx.clone());
    if let Some(cmd) = start {
        let _ = tx.send(Ev::Remote(cmd));
    }
    // Only in the first one; the settings turn it on and off as it runs.
    let (tray_tx, mut tray) = (tx.clone(), None::<tray::Tray>);
    // ratatui's setup (raw mode, the alternate screen, the panic hook), then
    // its terminal on the backend that draws wide emojis right.
    drop(ratatui::init());
    let mut term = ratatui::Terminal::new(backend::Wide(ratatui::backend::CrosstermBackend::new(std::io::stdout())))
        .expect("failed to initialize terminal");
    // Asks the terminal what graphics it speaks, before anything else reads
    // input. Half blocks make a smudge of a photo: those go without.
    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .ok()
        .filter(|p| p.protocol_type() != ratatui_image::picker::ProtocolType::Halfblocks);
    // The theme's greens, also asked of the terminal before the reader starts.
    let mut theme = theme::Watch::start();
    let mut app = app::App::new(tx.clone(), picker, owner);
    app.starting();

    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    // So Ctrl+I (italic) comes apart from Tab, and Shift+Enter from Enter,
    // where the terminal speaks kitty's protocol, as in slick.
    let _ = crossterm::execute!(std::io::stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    // Whether the window has the keyboard: the open chat only counts as read then.
    let _ = crossterm::execute!(std::io::stdout(), EnableFocusChange);
    std::thread::spawn(move || {
        while let Ok(e) = event::read() {
            if tx.send(Ev::Term(e)).is_err() {
                break;
            }
        }
    });

    // Zooming the terminal changes the cell's pixels, not always the columns
    // (so no resize comes): measured on resize, on focus and every second.
    let (mut cell, mut cell_at) = (cell_px(), Instant::now());
    while !app.quit {
        // A video played inside the terminal: its frames are gone, and so
        // are the pictures mpv took off with them, and the mouse it let go.
        if std::mem::take(&mut app.repaint) {
            use std::io::Write;
            let _ = write!(std::io::stdout(), "\x1b_Ga=d,d=A,q=2\x1b\\");
            app.protocols.clear();
            let _ = term.clear();
            let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
            let _ = term.hide_cursor();
        }
        if !app.video.covers() && (std::mem::take(&mut app.recheck_cell) || cell_at.elapsed() >= Duration::from_secs(1)) {
            cell_at = Instant::now();
            let now = cell_px();
            if let Some((w, h)) = now.filter(|_| now != cell) {
                cell = now;
                app.set_font_size(w, h);
                let _ = term.clear();
            }
        }
        theme.poll();
        app.publish();
        // Dropping it takes it off the bar.
        if (owner && app.settings.on(config::TRAY)) != tray.is_some() {
            tray = tray.is_none().then(|| tray::Tray::start(tray_tx.clone()));
        }
        if let Some(t) = &mut tray {
            // A pause dims it, like the widget.
            t.set(if app.paused() { 0 } else { app.attention() });
        }
        // mpv has the screen while a video plays inside it; one about to
        // starts once the viewer under it is on screen.
        if !app.video.covers() {
            let _ = term.draw(|f| ui::draw(f, &mut app));
            app.after_draw();
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            // A history sync arrives in bursts: take all of it before drawing again.
            Ok(ev) => {
                app.on(ev);
                while let Ok(ev) = rx.try_recv() {
                    app.on(ev);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        app.tick();
    }

    app.video.shutdown();
    let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags, DisableMouseCapture, DisableBracketedPaste, DisableFocusChange);
    app.save_drafts();
    ratatui::restore();
    app.unpublish();
    // Into the installed update: dropping the app stops the bridge; the lock
    // and the socket close with this process image (close-on-exec) and the
    // new one claims them again, in this same terminal.
    if app.account.restart {
        drop(app);
        drop(term);
        eprintln!("whatsapp-tui: não reiniciou: {}", update::restart());
    }
}
