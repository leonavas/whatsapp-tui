//! The Go helper (bridge/): written to the cache once per build, started with
//! the data folder, read line by line on its own thread.

use crate::Ev;
use serde::Deserialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

const HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/whatsapp-tui-bridge"));

#[derive(Deserialize, Clone, Debug)]
pub struct Chat {
    pub jid: String,
    pub name: String,
    #[serde(default)]
    pub group: bool,
    pub last_ts: i64,
    pub unread: u32,
    pub pinned: bool,
    pub archived: bool,
    pub muted: bool,
    pub preview: String,
    pub from_me: bool,
    /// The last message's, when it's ours.
    #[serde(default)]
    pub status: u8,
    /// Why nothing can be sent here; empty when it can.
    #[serde(default)]
    pub read_only: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Msg {
    pub chat: String,
    pub id: String,
    pub sender: String,
    pub sender_name: String,
    pub from_me: bool,
    pub ts: i64,
    pub text: String,
    #[serde(default)]
    pub quote: String,
    /// A photo, video, audio, document or sticker, downloadable on demand.
    #[serde(default)]
    pub media: Option<Media>,
    /// Its media couldn't be had, here or from the phone.
    #[serde(skip)]
    pub unavailable: bool,
    /// 0 pending, 1 sent, 2 delivered, 3 read.
    pub status: u8,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Media {
    /// audio, image, video, document, sticker.
    pub kind: String,
    pub mime: String,
    pub seconds: u32,
    /// A voice note rather than an audio file.
    pub ptt: bool,
}

/// Someone in the address book we have no chat with yet.
#[derive(Deserialize, Clone, Debug)]
pub struct Contact {
    pub jid: String,
    pub name: String,
    pub phone: String,
}

/// A phone number typed in the search, checked on WhatsApp.
#[derive(Deserialize, Clone, Debug)]
pub struct Number {
    pub jid: String,
    pub phone: String,
    pub exists: bool,
}

/// What the contact (or group) info panel shows.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Profile {
    pub chat: String,
    pub name: String,
    pub push_name: String,
    pub phone: String,
    pub about: String,
    pub business: String,
    pub category: String,
    pub address: String,
    pub email: String,
    /// A business account, with a verified name or not.
    pub biz: bool,
    pub description: String,
    pub website: Vec<String>,
    pub hours: Vec<Hours>,
    /// The business's UTC offset in seconds, when it set a time zone.
    pub tz: Option<i32>,
    /// Google Maps on its address (or coordinates).
    pub maps: String,
    /// The full-size picture on disk.
    pub photo: String,
    pub created: i64,
    pub members: Vec<Member>,
    pub error: String,
}

/// One opening of a business: `day` 0 is Monday, times in minutes after
/// midnight; `mode` is specific_hours, open_24h or appointment_only.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Hours {
    pub day: u8,
    pub mode: String,
    pub open: u16,
    pub close: u16,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Member {
    pub jid: String,
    pub name: String,
    pub phone: String,
    pub admin: bool,
    pub me: bool,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Event {
    Qr { code: String },
    Paired,
    /// `name` is your WhatsApp name, as last saved.
    Connecting {
        #[serde(default)]
        name: String,
    },
    Ready {
        #[serde(default)]
        name: String,
    },
    LoggedOut,
    Sync { progress: u32 },
    Chats { chats: Vec<Chat> },
    History { chat: String, msgs: Vec<Msg> },
    /// `new`: someone else's message, just stored; not an edit, a deletion
    /// nor one of yours.
    Message {
        msg: Msg,
        #[serde(default)]
        new: bool,
    },
    /// A chat's newest unread messages, for the snapshot's preview.
    Unread { chat: String, msgs: Vec<Msg> },
    Status { chat: String, ids: Vec<String>, status: u8 },
    Read { chat: String },
    Typing { chat: String, who: String, on: bool },
    Search {
        query: String,
        #[serde(default)]
        contacts: Vec<Contact>,
        #[serde(default)]
        messages: Vec<Msg>,
        #[serde(default)]
        number: Option<Number>,
    },
    Profile(Profile),
    /// A profile picture on disk; an empty path when there's none.
    Avatar { chat: String, path: String },
    /// A message's media downloaded to `path`, or why it couldn't be.
    Media {
        chat: String,
        id: String,
        #[serde(default)]
        path: String,
        #[serde(default)]
        error: String,
    },
    Error {
        text: String,
        #[serde(default)]
        fatal: bool,
    },
    /// The helper's stdout closed: it exited.
    #[serde(skip)]
    Exited,
    #[serde(other)]
    Other,
}

/// The app's folders' name, under the data and cache homes.
pub const NAME: &str = "whatsapp-tui";

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join(NAME)
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join(NAME)
}

/// The hooks, and `config` with the few settings there are.
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join(NAME)
}

/// A `key = value` line of `~/.config/whatsapp-tui/config`.
pub fn setting(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_dir().join("config")).ok()?;
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
    })
}

/// The app was called zap: its folders move to the new names, the paired
/// session with them, so nobody has to scan the QR code again. Nothing is
/// removed; a new folder already there wins and the old one stays put.
pub fn migrate() {
    for home in [("XDG_DATA_HOME", ".local/share"), ("XDG_CACHE_HOME", ".cache")] {
        let base = xdg(home.0, home.1);
        let (old, new) = (base.join("zap"), base.join(NAME));
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
        }
    }
    let data = data_dir();
    // With its write-ahead log: what that holds isn't in the database yet.
    if data.join("zap.db").exists() && !data.join(format!("{NAME}.db")).exists() {
        for ext in ["-wal", "-shm", ""] {
            let old = data.join(format!("zap.db{ext}"));
            if old.exists() {
                let _ = std::fs::rename(&old, data.join(format!("{NAME}.db{ext}")));
            }
        }
    }
    // Drafts point at pictures pasted into the old cache.
    let drafts = data.join("drafts.json");
    let old_cache = xdg("XDG_CACHE_HOME", ".cache").join("zap").display().to_string();
    if let Ok(text) = std::fs::read_to_string(&drafts) {
        if text.contains(&old_cache) {
            let _ = std::fs::write(&drafts, text.replace(&old_cache, &cache_dir().display().to_string()));
        }
    }
}

pub(crate) fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
}

/// The helper, written to `~/.cache/whatsapp-tui/bridge-<hash>/` once per
/// build, older copies removed. `WHATSAPP_TUI_BRIDGE` points at another one
/// (development).
fn program() -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    if let Some(p) = std::env::var_os("WHATSAPP_TUI_BRIDGE") {
        return Ok(p.into());
    }
    let hash: String = Sha256::digest(HELPER).iter().take(6).map(|b| format!("{b:02x}")).collect();
    let cache = cache_dir();
    let dir = cache.join(format!("bridge-{hash}"));
    let path = dir.join("whatsapp-tui-bridge");
    if path.exists() {
        return Ok(path);
    }
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&cache)?;
        for e in std::fs::read_dir(&cache)?.flatten() {
            if e.file_name().to_string_lossy().starts_with("bridge-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(".whatsapp-tui-bridge.tmp");
        std::fs::write(&tmp, HELPER)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, &path)
    };
    write().map_err(|e| format!("não preparou o bridge: {e}"))?;
    Ok(path)
}

pub struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Bridge {
    pub fn spawn(tx: Sender<Ev>) -> Result<Bridge, String> {
        let data = data_dir();
        std::fs::create_dir_all(&data).map_err(|e| format!("não criou {}: {e}", data.display()))?;
        let log = std::fs::File::create(data.join("bridge.log")).map_err(|e| e.to_string())?;
        let mut child = Command::new(program()?)
            .arg(&data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log)
            .spawn()
            .map_err(|e| format!("não iniciou o bridge: {e}"))?;
        let stdout = child.stdout.take().expect("piped stdout");
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(e) = serde_json::from_str::<Event>(&line) {
                    if tx.send(Ev::Bridge(e)).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send(Ev::Bridge(Event::Exited));
        });
        let stdin = child.stdin.take();
        Ok(Bridge { child, stdin })
    }

    pub fn send(&mut self, cmd: Value) {
        if let Some(w) = &mut self.stdin {
            let _ = writeln!(w, "{cmd}");
        }
    }
}

impl Drop for Bridge {
    /// Closing stdin tells the helper to disconnect; it gets a moment to.
    fn drop(&mut self) {
        self.stdin.take();
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
    }
}
