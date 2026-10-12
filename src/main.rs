//! whatsapp-tui: a lightweight WhatsApp client for the terminal. The
//! WhatsApp side lives in a Go helper (bridge/, whatsmeow) this binary
//! carries; this side is the TUI, fed by the helper's JSON lines. It also
//! tells other programs what is unread and takes their commands (`events`,
//! `ipc`, `cli`).

mod app;
mod attach;
mod avatar;
mod bigemoji;
mod bridge;
mod cli;
mod drafts;
mod emoji;
mod emoji_panel;
mod events;
mod ipc;
mod links;
mod player;
mod ticks;
mod tray;
mod ui;

use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture,
};
use std::io::IsTerminal;
use std::sync::mpsc;
use std::time::Duration;

pub enum Ev {
    Term(event::Event),
    Bridge(bridge::Event),
    /// The clipboard read, files picked, drafts loaded: off the UI thread.
    Attach(attach::Ev),
    /// Where the audio playing is.
    Player(player::Ev),
    /// A command from another process: the CLI, the bar widget, the tray.
    Remote(ipc::Cmd),
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
    if let Some(cmd) = start {
        let _ = tx.send(Ev::Remote(cmd));
    }
    // On unless `tray = false` in the config, and only in the first one.
    let mut tray = (owner && bridge::setting("tray").is_none_or(|v| v != "false")).then(|| tray::Tray::start(tx.clone()));
    let mut term = ratatui::init();
    // Asks the terminal what graphics it speaks, before anything else reads
    // input. Half blocks make a smudge of a photo: those go without.
    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .ok()
        .filter(|p| p.protocol_type() != ratatui_image::picker::ProtocolType::Halfblocks);
    let mut app = app::App::new(tx.clone(), picker, owner);
    app.starting();

    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    // Whether the window has the keyboard: the open chat only counts as read then.
    let _ = crossterm::execute!(std::io::stdout(), EnableFocusChange);
    std::thread::spawn(move || {
        while let Ok(e) = event::read() {
            if tx.send(Ev::Term(e)).is_err() {
                break;
            }
        }
    });

    while !app.quit {
        app.publish();
        if let Some(t) = &mut tray {
            // A pause dims it, like the widget.
            t.set(if app.paused() { 0 } else { app.attention() });
        }
        let _ = term.draw(|f| ui::draw(f, &mut app));
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

    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste, DisableFocusChange);
    app.save_drafts();
    ratatui::restore();
    app.unpublish();
}
