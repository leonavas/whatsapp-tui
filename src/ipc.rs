//! One whatsapp-tui per session. The first one holds a lock in the runtime
//! dir and listens on a Unix socket next to it; a later one finds the lock
//! taken, hands the running one what it was asked and exits. The socket
//! speaks JSON lines (`{"cmd":"open","chat":"…"}`), so scripts and the
//! Omarchy widget can use it too; `{"cmd":"subscribe"}` turns a connection
//! into a feed of events (see `events`).

use crate::Ev;
use std::fs::{File, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Duration;

/// What another process asks the running whatsapp-tui to do.
pub enum Cmd {
    /// Bring the window to the front.
    Focus,
    /// A chat, scrolled to a message.
    Open { chat: String, id: Option<String>, focus: bool },
    MarkRead(String),
    /// Pause notifications until then (Unix seconds), or resume them.
    Pause(i64),
    Resume,
    Quit,
}

impl Cmd {
    fn parse(v: &serde_json::Value) -> Option<Cmd> {
        let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
        Some(match v["cmd"].as_str()? {
            "focus" => Cmd::Focus,
            "open" => Cmd::Open { chat: s("chat")?, id: s("id"), focus: v["focus"].as_bool() != Some(false) },
            "mark-read" => Cmd::MarkRead(s("chat")?),
            "pause" => {
                let until = match (v["until"].as_i64(), v["minutes"].as_i64().filter(|m| *m > 0)) {
                    (Some(t), _) => t,
                    (None, Some(m)) => chrono::Local::now().timestamp() + m * 60,
                    _ => return None,
                };
                Cmd::Pause(until)
            }
            "resume" => Cmd::Resume,
            "quit" => Cmd::Quit,
            _ => return None,
        })
    }
}

/// `$XDG_RUNTIME_DIR/whatsapp-tui`, a private tmpfs that goes away with the
/// session; the cache folder when there is none.
pub fn dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(|d| PathBuf::from(d).join(crate::bridge::NAME))
        .unwrap_or_else(|| crate::bridge::cache_dir().join("run"))
}

fn address() -> PathBuf {
    dir().join("whatsapp-tui.sock")
}

fn lock_path() -> PathBuf {
    dir().join("whatsapp-tui.lock")
}

pub enum Claim {
    /// We are the one; `serve` starts answering the others.
    Owner(Server),
    /// Another whatsapp-tui holds the lock.
    Running,
}

pub struct Server {
    /// Held for as long as the process lives; the system lets go of it on exit.
    lock: File,
    listener: UnixListener,
}

fn make_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = dir();
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    Ok(dir)
}

/// Whether some whatsapp-tui holds the lock right now.
pub fn running() -> bool {
    let Ok(lock) = File::options().create(true).truncate(false).write(true).open(lock_path()) else { return false };
    matches!(lock.try_lock(), Err(TryLockError::WouldBlock))
}

/// Takes the lock, or says someone has it. Anything else that goes wrong
/// (no runtime dir, no socket) is `None`: the app runs, just without the rest.
pub fn claim() -> Option<Claim> {
    use std::os::unix::fs::PermissionsExt;
    make_dir().ok()?;
    let lock = File::options().create(true).truncate(false).write(true).open(lock_path()).ok()?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Some(Claim::Running),
        Err(_) => return None,
    }
    // Whatever is there belonged to one that is gone: the lock says so.
    let path = address();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).ok()?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Some(Claim::Owner(Server { lock, listener }))
}

/// Connections that asked for events.
static SUBSCRIBERS: Mutex<Vec<UnixStream>> = Mutex::new(Vec::new());

impl Server {
    /// Answers in its own thread; every command becomes an `Ev::Remote`.
    pub fn serve(self, events: Sender<Ev>) {
        std::thread::spawn(move || {
            let _lock = self.lock;
            for conn in self.listener.incoming().flatten() {
                let events = events.clone();
                std::thread::spawn(move || {
                    let Ok(read) = conn.try_clone() else { return };
                    for line in BufReader::new(read).lines().map_while(Result::ok) {
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                        if v["cmd"] == "subscribe" {
                            let _ = conn.set_write_timeout(Some(Duration::from_secs(2)));
                            if let (Ok(c), Ok(mut subs)) = (conn.try_clone(), SUBSCRIBERS.lock()) {
                                subs.push(c);
                            }
                        } else if let Some(cmd) = Cmd::parse(&v) {
                            if events.send(Ev::Remote(cmd)).is_err() {
                                return;
                            }
                        }
                    }
                });
            }
        });
    }
}

/// One line to every subscriber; the ones that went away are dropped.
pub fn broadcast(line: &str) {
    let Ok(mut subs) = SUBSCRIBERS.lock() else { return };
    subs.retain_mut(|s| writeln!(s, "{line}").and_then(|_| s.flush()).is_ok());
}

/// A connection to the running whatsapp-tui. The lock can be taken a moment
/// before the socket is up, so a refused connection gets a few more tries.
pub fn connect() -> std::io::Result<UnixStream> {
    let mut tries = 0;
    loop {
        match UnixStream::connect(address()) {
            Ok(c) => return Ok(c),
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Sends one command to the running whatsapp-tui.
pub fn send(cmd: &serde_json::Value) -> std::io::Result<()> {
    let mut conn = connect()?;
    writeln!(conn, "{cmd}")?;
    conn.flush()
}

/// Prints the running whatsapp-tui's events, one JSON line each, until
/// stdout goes away. Waits for it when it isn't running, and again after it quits.
pub fn follow() -> i32 {
    let mut out = std::io::stdout();
    loop {
        if let Ok(mut conn) = UnixStream::connect(address()) {
            if writeln!(conn, "{}", serde_json::json!({ "cmd": "subscribe" })).is_ok() {
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                        return 0;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
