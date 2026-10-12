//! What whatsapp-tui tells other programs: a snapshot of what is unread, in
//! `state.json` next to the socket (see `ipc`), and events, as JSON lines,
//! to the socket's subscribers and to the hooks in
//! `~/.config/whatsapp-tui/hooks/<event>.d/`. The Omarchy widget reads the
//! snapshot; scripts can follow `whatsapp-tui events`. docs/events.md has the format.

use crate::app::{App, Phase};
use crate::bridge::{Chat, Msg};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

/// Bumped when a field changes meaning or goes away; new fields don't bump it.
const VERSION: u32 = 1;
/// Unread messages kept per chat for the preview.
const PREVIEW: usize = 5;
/// Where `text` is cut.
const TEXT_MAX: usize = 500;

/// What a chat's preview was fetched for: its newest message and unread count.
type Key = (i64, u32);

#[derive(Default)]
pub struct State {
    /// Off in a second whatsapp-tui (`--new-instance`): the first one speaks for it.
    pub on: bool,
    /// The chat list came at least once: before it, every badge would read 0.
    pub chats_loaded: bool,
    /// When this one started: messages older than that were waiting already,
    /// not news.
    pub started: i64,
    /// Notifications paused until then, as `pause.json` keeps it.
    pub paused: Option<i64>,
    previews: HashMap<String, (Key, Vec<Msg>)>,
    fetching: HashMap<String, Key>,
    /// The last snapshot, without its time, to write only what changed.
    last: Option<Value>,
}

impl State {
    pub fn new(on: bool) -> State {
        State { on, started: chrono::Local::now().timestamp(), paused: load_pause(), ..Default::default() }
    }
}

// ---- the pause -------------------------------------------------------------

/// WhatsApp has no do-not-disturb of its own: the pause lives here, in the
/// data folder, so it outlasts a restart and the CLI can set it with the app closed.
fn pause_path() -> PathBuf {
    crate::bridge::data_dir().join("pause.json")
}

/// The pause still running, if any.
pub fn load_pause() -> Option<i64> {
    let text = std::fs::read_to_string(pause_path()).ok()?;
    let until = serde_json::from_str::<Value>(&text).ok()?["until"].as_i64()?;
    (until > chrono::Local::now().timestamp()).then_some(until)
}

pub fn save_pause(until: Option<i64>) -> std::io::Result<()> {
    let path = pause_path();
    match until {
        Some(t) => {
            std::fs::create_dir_all(crate::bridge::data_dir())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json!({ "until": t }).to_string())?;
            std::fs::rename(&tmp, &path)
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

// ---- delivery --------------------------------------------------------------

/// Events and snapshots go out from a thread of their own: a slow hook or
/// subscriber never holds up the interface.
enum Job {
    Event(String, String),
    State(String),
}

fn worker() -> &'static Sender<Job> {
    static TX: OnceLock<Sender<Job>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            for job in rx {
                match job {
                    Job::Event(name, line) => deliver(&name, &line),
                    Job::State(text) => write_state(&text),
                }
            }
        });
        tx
    })
}

fn envelope(name: &str, data: Value) -> String {
    json!({ "v": VERSION, "event": name, "at": chrono::Local::now().timestamp(), "data": data }).to_string()
}

fn emit(name: &str, data: Value) {
    let _ = worker().send(Job::Event(name.to_string(), envelope(name, data)));
}

fn deliver(name: &str, line: &str) {
    crate::ipc::broadcast(line);
    run_hooks(name, line);
}

/// Every executable in `hooks/<event>.d/`, with the event on stdin and its
/// name in `WHATSAPP_TUI_EVENT`. Started and left to run: nobody waits for them.
fn run_hooks(name: &str, line: &str) {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::bridge::config_dir().join("hooks").join(format!("{name}.d"));
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut hooks: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
        .collect();
    hooks.sort();
    for hook in hooks {
        let child = std::process::Command::new(&hook)
            .env("WHATSAPP_TUI_EVENT", name)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut c) = child {
            if let Some(mut stdin) = c.stdin.take() {
                let _ = writeln!(stdin, "{line}");
            }
            // Reaped in the background, so no zombie is left behind.
            std::thread::spawn(move || c.wait());
        }
    }
}

pub fn state_path() -> PathBuf {
    crate::ipc::dir().join("state.json")
}

/// Written beside and renamed over, so a reader never sees half of it.
fn write_state(text: &str) {
    let path = state_path();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

// ---- text ------------------------------------------------------------------

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// WhatsApp's marks as HTML: ```mono```, *bold*, _italic_, ~strike~,
/// everything else escaped.
fn markup(text: &str) -> String {
    let parts: Vec<&str> = text.split("```").collect();
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i % 2 == 1 && i + 1 < parts.len() {
            out.push_str(&format!("<tt>{}</tt>", escape(part)));
        } else {
            // A ``` left without its pair stays as it was typed.
            if i % 2 == 1 {
                out.push_str("```");
            }
            out.push_str(&inline(part));
        }
    }
    out
}

/// A mark opens after a non-word character and before a non-space, and
/// closes after a non-space and before a non-word character, on one line.
fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let tag = match c {
            '*' => Some("b"),
            '_' => Some("i"),
            '~' => Some("s"),
            _ => None,
        };
        let opens = tag.is_some()
            && (i == 0 || !chars[i - 1].is_alphanumeric())
            && chars.get(i + 1).is_some_and(|n| !n.is_whitespace() && *n != c);
        let close = opens
            .then(|| {
                (i + 2..chars.len()).take_while(|&j| chars[j] != '\n').find(|&j| {
                    chars[j] == c && !chars[j - 1].is_whitespace() && chars.get(j + 1).is_none_or(|n| !n.is_alphanumeric())
                })
            })
            .flatten();
        match (tag, close) {
            (Some(t), Some(j)) => {
                let inner: String = chars[i + 1..j].iter().collect();
                out.push_str(&format!("<{t}>{}</{t}>", inline(&inner)));
                i = j + 1;
            }
            _ => {
                out.push_str(&escape(&c.to_string()));
                i += 1;
            }
        }
    }
    out
}

fn cut(text: &str) -> String {
    if text.chars().count() > TEXT_MAX {
        text.chars().take(TEXT_MAX).collect::<String>() + "…"
    } else {
        text.to_string()
    }
}

fn message_json(m: &Msg) -> Value {
    let text = cut(m.text.trim());
    json!({ "id": m.id, "ts": m.ts, "sender": m.sender, "author": m.sender_name, "text": text, "html": markup(&text) })
}

/// The chat's profile picture as the bridge saved it, when it is there.
fn avatar(jid: &str) -> Option<String> {
    let name = jid.replace(['@', '.', ':'], "_");
    let path = crate::bridge::data_dir().join("avatars").join(format!("{name}.jpg"));
    path.is_file().then(|| path.display().to_string())
}

fn kind(jid: &str) -> &'static str {
    if jid.ends_with("@g.us") {
        "group"
    } else {
        "dm"
    }
}

impl App {
    fn conv_json(&self, jid: &str) -> Value {
        let chat = self.chat(jid);
        let name = self.name_of(jid).unwrap_or_else(|| jid.split('@').next().unwrap_or(jid).to_string());
        json!({ "id": jid, "kind": kind(jid), "name": name, "muted": chat.is_some_and(|c| c.muted), "avatar": avatar(jid) })
    }

    /// Events go out once `app.started` has, and only from the first one.
    fn speaking(&self) -> bool {
        self.events.on && self.events.last.is_some()
    }

    pub fn paused(&self) -> bool {
        self.events.paused.is_some()
    }

    /// Unread chats that ping, the badge: neither muted nor archived.
    pub fn attention(&self) -> usize {
        self.chats.iter().filter(|c| c.unread > 0 && !c.muted && !c.archived).count()
    }

    pub fn set_pause(&mut self, until: Option<i64>) {
        let until = until.filter(|t| *t > chrono::Local::now().timestamp());
        self.events.paused = until;
        if let Err(e) = save_pause(until) {
            self.note(format!("não salvou a pausa: {e}"));
        }
    }

    /// A message someone else just sent, as the bridge first stored it.
    /// Older than this run, it was waiting already: the snapshot has it.
    pub(crate) fn announce(&self, m: &Msg) {
        if !self.speaking() || m.from_me || m.ts < self.events.started - 5 {
            return;
        }
        let conv = self.conv_json(&m.chat);
        let muted = conv["muted"] == true;
        let seen = self.focused && self.open.as_deref() == Some(m.chat.as_str());
        let notify = !muted && !self.paused() && !seen;
        emit("message.new", json!({ "conversation": conv, "message": message_json(m), "muted": muted, "notify": notify }));
    }

    pub(crate) fn on_unread(&mut self, chat: String, msgs: Vec<Msg>) {
        let Some(key) = self.events.fetching.remove(&chat) else { return };
        let keep = msgs[msgs.len().saturating_sub(PREVIEW)..].to_vec();
        self.events.previews.insert(chat, (key, keep));
    }

    /// The chats in the snapshot: unread and not archived.
    fn listed(&self) -> impl Iterator<Item = &Chat> {
        self.chats.iter().filter(|c| c.unread > 0 && !c.archived)
    }

    /// Writes the snapshot when something in it changed, and tells the
    /// subscribers what: counts, chats read, a pause.
    pub fn publish(&mut self) {
        if !self.events.on || !self.events.chats_loaded || matches!(self.phase, Phase::Qr(_) | Phase::LoggedOut | Phase::Dead(_)) {
            return;
        }
        if self.events.paused.is_some_and(|t| t <= chrono::Local::now().timestamp()) {
            self.set_pause(None);
        }
        // Previews for the chats whose newest message or count moved.
        let want: Vec<(String, Key)> = self
            .listed()
            .map(|c| (c.jid.clone(), (c.last_ts, c.unread)))
            .filter(|(j, k)| self.events.previews.get(j).is_none_or(|p| p.0 != *k) && self.events.fetching.get(j) != Some(k))
            .collect();
        for (jid, key) in want {
            self.events.fetching.insert(jid.clone(), key);
            self.cmd(json!({"c": "unread", "chat": jid, "limit": PREVIEW}));
        }
        let unread: HashSet<String> = self.listed().map(|c| c.jid.clone()).collect();
        self.events.previews.retain(|j, _| unread.contains(j));

        let (mut direct, mut groups, mut muted, mut archived) = (0u32, 0u32, 0u32, 0u32);
        let (mut direct_messages, mut group_messages) = (0u64, 0u64);
        let mut convs: Vec<(i64, Value)> = Vec::new();
        for c in self.chats.iter().filter(|c| c.unread > 0) {
            if c.archived {
                archived += 1;
                continue;
            }
            if c.group {
                groups += 1;
                group_messages += c.unread as u64;
            } else {
                direct += 1;
                direct_messages += c.unread as u64;
            }
            muted += u32::from(c.muted);
            let messages: Vec<Value> = self.events.previews.get(&c.jid).map(|p| p.1.iter().map(message_json).collect()).unwrap_or_default();
            let mut v = self.conv_json(&c.jid);
            v["unread"] = json!(c.unread);
            v["latest"] = json!(c.last_ts);
            v["quiet"] = json!(c.muted);
            v["messages"] = json!(messages);
            convs.push((c.last_ts, v));
        }
        convs.sort_by(|a, b| b.0.cmp(&a.0));
        let paused = json!({ "paused": self.events.paused.is_some(), "until": self.events.paused });
        let snapshot = json!({
            "v": VERSION,
            "running": true,
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
            "user": self.me_name,
            "connected": self.phase == Phase::Ready,
            "paused": paused,
            "attention": self.attention(),
            "totals": {
                "chats": direct + groups, "messages": direct_messages + group_messages,
                "direct": direct, "groups": groups, "muted": muted,
                "direct_messages": direct_messages, "group_messages": group_messages, "archived": archived,
            },
            "conversations": convs.into_iter().map(|c| c.1).collect::<Vec<_>>(),
        });
        if self.events.last.as_ref() == Some(&snapshot) {
            return;
        }
        match self.events.last.take() {
            None => emit("app.started", json!({ "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION"), "user": self.me_name })),
            Some(old) => {
                if old["totals"] != snapshot["totals"] || old["attention"] != snapshot["attention"] {
                    emit("counts.changed", json!({ "attention": snapshot["attention"], "totals": snapshot["totals"] }));
                }
                let ids = |v: &Value| -> HashSet<String> {
                    v["conversations"].as_array().into_iter().flatten().filter_map(|c| c["id"].as_str().map(String::from)).collect()
                };
                let now = ids(&snapshot);
                for id in ids(&old).difference(&now) {
                    emit("conversation.read", json!({ "conversation": self.conv_json(id) }));
                }
                if old["paused"] != snapshot["paused"] {
                    emit("pause.changed", snapshot["paused"].clone());
                }
            }
        }
        let mut text = snapshot.clone();
        text["updated"] = json!(chrono::Local::now().timestamp());
        let _ = worker().send(Job::State(text.to_string()));
        self.events.last = Some(snapshot);
    }

    /// On the way in: a snapshot left by one that died without its goodbye
    /// (its terminal closed) would show its old badges until the first real one.
    pub fn starting(&self) {
        if self.events.on {
            write_state(&json!({ "v": VERSION, "running": false, "pid": std::process::id(), "updated": chrono::Local::now().timestamp() }).to_string());
        }
    }

    /// On the way out: the snapshot says so, and the subscribers hear it.
    /// Done here and now, since the process ends right after.
    pub fn unpublish(&mut self) {
        if !self.events.on {
            return;
        }
        write_state(&json!({ "v": VERSION, "running": false, "pid": std::process::id(), "updated": chrono::Local::now().timestamp() }).to_string());
        if self.events.last.is_some() {
            deliver("app.stopped", &envelope("app.stopped", json!({ "pid": std::process::id() })));
        }
    }
}
