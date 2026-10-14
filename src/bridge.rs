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
    /// Marked as unread by hand: its one unread may be no message at all.
    #[serde(default)]
    pub marked: bool,
    pub pinned: bool,
    /// When it was pinned: the last pinned comes first.
    #[serde(default)]
    pub pin_ts: i64,
    pub archived: bool,
    pub muted: bool,
    #[serde(default)]
    pub favorite: bool,
    pub preview: String,
    pub from_me: bool,
    /// The last message's, when it's ours.
    #[serde(default)]
    pub status: u8,
    /// Why nothing can be sent here; empty when it can.
    #[serde(default)]
    pub read_only: String,
    /// WhatsApp itself, or a business with WhatsApp's check by its name.
    #[serde(default)]
    pub verified: bool,
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
    /// Who wrote the quoted message, and the name its quote shows.
    #[serde(default)]
    pub quote_from: String,
    #[serde(default)]
    pub quote_name: String,
    /// The quoted message's id: a click on the quote goes to it.
    #[serde(default)]
    pub quote_id: String,
    /// The emojis under it, the most used first.
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    /// The @s in its text that name someone.
    #[serde(default)]
    pub mentions: Vec<Mention>,
    /// A photo, video, audio, document or sticker, downloadable on demand.
    #[serde(default)]
    pub media: Option<Media>,
    /// A business's title, footer and buttons around the text.
    #[serde(default)]
    pub card: Option<Card>,
    /// Forwarded, how many times over (WhatsApp's score); 0 when not.
    #[serde(default)]
    pub forwarded: u32,
    /// Its media couldn't be had, here or from the phone.
    #[serde(skip)]
    pub unavailable: bool,
    /// 0 pending, 1 sent, 2 delivered, 3 read.
    pub status: u8,
}

/// One emoji under a message: how many sent it, and whether you did.
/// An @ in a message's text: the digits it's written with, who that is
/// and the name shown in their place.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Mention {
    pub user: String,
    pub jid: String,
    pub name: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Reaction {
    pub emoji: String,
    pub count: u32,
    pub mine: bool,
    /// Who sent it, the first to react first.
    pub by: Vec<Reactor>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Reactor {
    pub jid: String,
    pub name: String,
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
    /// A photo's or video's size in pixels, when the message says.
    pub width: u32,
    pub height: u32,
    /// A JPEG of it on disk: the message's thumbnail, or a video's frame.
    pub thumb: String,
    /// A video that loops without sound.
    pub gif: bool,
    /// The file, once it's downloaded.
    pub file: String,
}

/// What a business's message has besides its text.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Card {
    pub title: String,
    pub footer: String,
    pub buttons: Vec<Button>,
}

/// A row at the bottom of a business's message; only the ones with a URL
/// do something here.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Button {
    pub text: String,
    pub url: String,
    /// url, reply, call, copy or list.
    pub kind: String,
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
    /// `me` is your number, the user part of your JID.
    Connecting {
        #[serde(default)]
        name: String,
        #[serde(default)]
        me: String,
    },
    Ready {
        #[serde(default)]
        name: String,
        #[serde(default)]
        me: String,
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
    /// A chat's messages cleared, or the chat deleted, here or on the phone.
    Cleared {
        chat: String,
        #[serde(default)]
        deleted: bool,
    },
    /// Messages deleted for you, here or on another device.
    Removed { chat: String, ids: Vec<String> },
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
    /// The info panel's picture at full size, after its `Profile`; an empty
    /// path when there's none to show, `error` when it didn't come.
    Photo {
        chat: String,
        path: String,
        #[serde(default)]
        error: String,
    },
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
    /// A better picture of a downloaded video: a frame of it.
    Thumb { chat: String, id: String, path: String },
    /// Under each chat's name in the forward dialog: a group's members, a
    /// contact's recado.
    ChatSubs { subs: std::collections::HashMap<String, String> },
    /// Whether a sticker is among your favorites, asked or just changed.
    StickerFav { chat: String, id: String, fav: bool },
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
        // The last run's log kept beside, for what happened before a restart.
        let _ = std::fs::rename(data.join("bridge.log"), data.join("bridge.log.1"));
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
