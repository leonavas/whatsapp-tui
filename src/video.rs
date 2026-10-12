//! Videos, played by mpv: in the media viewer (`viewer`), over the whole
//! terminal (kitty's graphics, through mpv's `--vo=kitty`), or in a window
//! of its own. mpv writes its frames to the same terminal as the TUI, and
//! two writers there break each other's escapes: while one plays inside,
//! the TUI doesn't draw at all, and once it's gone everything is drawn again.

use ratatui::layout::Rect;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// From the thread waiting on mpv.
pub enum Ev {
    /// `ok`: it exited well (or was stopped), not because it couldn't play.
    Ended { gen: u64, ok: bool },
}

/// Where in the terminal to play it: its cells, and a cell's pixels.
#[derive(Clone, Copy)]
pub struct Spot {
    pub rect: Rect,
    pub cell: (u32, u32),
}

pub struct Video {
    /// mpv playing inside the terminal, and what: the window plays it if
    /// that fails.
    pid: Option<u32>,
    playing: Option<(String, bool)>,
    gen: u64,
    /// Asked to stop: past a moment, it's killed.
    stopping: Option<Instant>,
    tx: Sender<crate::Ev>,
}

impl Video {
    pub fn new(tx: Sender<crate::Ev>) -> Video {
        Video { pid: None, playing: None, gen: 0, stopping: None, tx }
    }

    /// mpv has the screen: nothing else may be drawn until it's gone.
    pub fn covers(&self) -> bool {
        self.pid.is_some()
    }

    /// Plays it over `s`, a GIF looped and muted, till it ends or is stopped.
    pub fn play(&mut self, path: &str, gif: bool, s: Spot) -> std::io::Result<()> {
        let Rect { x, y, width, height } = s.rect;
        let mut cmd = Command::new("mpv");
        // Its own config could put it anywhere else; the terminal's input
        // stays the TUI's. Its screen is left as it is (no alternate screen,
        // no clearing), and the picture goes over the viewer's cells.
        cmd.args(["--no-config", "--no-terminal", "--really-quiet", "--idle=no", "--keep-open=no", "--osd-level=0", "--osc=no"])
            .args(["--vo=kitty", "--vo-kitty-alt-screen=no", "--vo-kitty-config-clear=no", "--sws-scaler=fast-bilinear"])
            .arg(format!("--vo-kitty-left={}", x + 1))
            .arg(format!("--vo-kitty-top={}", y + 1))
            .arg(format!("--vo-kitty-cols={width}"))
            .arg(format!("--vo-kitty-rows={height}"))
            .arg(format!("--vo-kitty-width={}", width as u32 * s.cell.0))
            .arg(format!("--vo-kitty-height={}", height as u32 * s.cell.1));
        if shm() {
            cmd.arg("--vo-kitty-use-shm=yes");
        }
        if gif {
            cmd.args(["--loop-file=inf", "--no-audio"]);
        }
        let mut child = cmd.arg("--").arg(path).stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::null()).spawn()?;
        self.gen += 1;
        self.pid = Some(child.id());
        self.playing = Some((path.to_string(), gif));
        let (tx, gen) = (self.tx.clone(), self.gen);
        std::thread::spawn(move || {
            let ok = child.wait().is_ok_and(|s| s.success());
            let _ = tx.send(crate::Ev::Video(Ev::Ended { gen, ok }));
        });
        Ok(())
    }

    /// Stops it: mpv, asked nicely, takes its picture off the screen.
    pub fn stop(&mut self) {
        match self.pid {
            Some(pid) if self.stopping.is_none() => {
                signal(pid, libc::SIGTERM);
                self.stopping = Some(Instant::now());
            }
            _ => {}
        }
    }

    /// One that won't go is killed.
    pub fn tick(&mut self) {
        if let (Some(pid), Some(at)) = (self.pid, self.stopping) {
            if at.elapsed() > Duration::from_millis(1500) {
                signal(pid, libc::SIGKILL);
            }
        }
    }

    /// mpv's gone: true when it was inside the terminal, which then wants
    /// drawing again from scratch. One that couldn't play there (the
    /// terminal said no to its pictures) goes to a window.
    pub fn on(&mut self, ev: Ev) -> bool {
        let Ev::Ended { gen, ok } = ev;
        if gen != self.gen || self.pid.is_none() {
            return false;
        }
        let stopped = self.stopping.take().is_some();
        self.pid = None;
        if let Some((path, gif)) = self.playing.take().filter(|_| !ok && !stopped) {
            let _ = window(&path, gif);
        }
        true
    }

    /// On the way out: mpv cleans up while the screen is still the TUI's.
    pub fn shutdown(&mut self) {
        let Some(pid) = self.pid.take() else { return };
        signal(pid, libc::SIGTERM);
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until && alive(pid) {
            std::thread::sleep(Duration::from_millis(20));
        }
        if alive(pid) {
            signal(pid, libc::SIGKILL);
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Outside the app: in mpv's window, or without mpv in whatever opens videos.
pub fn external(path: &str, gif: bool) -> Result<(), String> {
    match window(path, gif) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::links::open(path);
            Ok(())
        }
        Err(e) => Err(format!("não tocou o vídeo: {e}")),
        Ok(()) => Ok(()),
    }
}

/// In a window of its own, the TUI carrying on; quiet and reaped off the
/// main thread, as `links::open` does.
fn window(path: &str, gif: bool) -> std::io::Result<()> {
    let mut cmd = Command::new("mpv");
    cmd.args(["--no-terminal", "--really-quiet", "--force-window=immediate", "--keep-open=no"]);
    if let Some(name) = Path::new(path).file_name() {
        cmd.arg(format!("--title=Vídeo · {}", name.to_string_lossy()));
    }
    if gif {
        cmd.args(["--loop-file=inf", "--no-audio"]);
    }
    let mut child = cmd.arg("--").arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Frames through shared memory where the terminal is known to read it
/// (kitty, ghostty): much less to write than the pixels in escapes.
fn shm() -> bool {
    let var = |k: &str| std::env::var(k).unwrap_or_default().to_lowercase();
    std::env::var_os("KITTY_WINDOW_ID").is_some() || var("TERM").contains("kitty") || var("TERM").contains("ghostty") || var("TERM_PROGRAM") == "ghostty"
}

fn signal(pid: u32, sig: libc::c_int) {
    // SAFETY: kill(2) on a child of ours; at worst it's already gone.
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks the process is there.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}
