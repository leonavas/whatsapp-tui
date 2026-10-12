//! Drafts, as on WhatsApp Web: what's typed (and attached) in a chat and
//! not sent stays with it when another is opened, shows in the chat list,
//! and is kept across runs in drafts.json.

use crate::app::App;
use crate::attach::{prepare, Attachment, Ev, Kind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Drafts {
    /// The text of chats other than the open one, whose text is the box's.
    pub texts: HashMap<String, String>,
    /// What's on disk, to write only changes.
    written: String,
    last_write: Option<Instant>,
    /// Nothing's written before the last run's drafts are back.
    restored: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct Saved {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<SavedFile>,
}

#[derive(Serialize, Deserialize)]
struct SavedFile {
    path: PathBuf,
    name: String,
    kind: Kind,
    #[serde(default)]
    temp: bool,
}

fn file() -> PathBuf {
    crate::bridge::data_dir().join("drafts.json")
}

impl App {
    /// Opening another chat: the box's text stays with the one left, and
    /// the new one's comes back.
    pub fn switch_draft(&mut self, to: &str) {
        let text = std::mem::take(&mut self.input);
        if let Some(from) = self.open.clone() {
            if text.trim().is_empty() {
                self.drafts.texts.remove(&from);
            } else {
                self.drafts.texts.insert(from, text);
            }
        }
        self.input = self.drafts.texts.remove(to).unwrap_or_default();
        self.caret = self.input.chars().count();
        self.attach.menu = None;
    }

    /// A chat's draft for its row in the list: its text, else what's
    /// attached. None for the open chat, which shows it in the box.
    pub fn draft_line(&self, jid: &str) -> Option<String> {
        if self.open.as_deref() == Some(jid) {
            return None;
        }
        let text = self.drafts.texts.get(jid).map(|t| t.trim()).filter(|t| !t.is_empty());
        text.map(|t| t.replace('\n', " ")).or_else(|| self.attach.files.get(jid)?.first().map(Attachment::draft_label))
    }

    fn snapshot(&self) -> String {
        let mut all: HashMap<&str, Saved> = HashMap::new();
        for (chat, t) in &self.drafts.texts {
            all.entry(chat).or_default().text = t.clone();
        }
        if let Some(open) = self.open.as_deref().filter(|_| !self.input.trim().is_empty()) {
            all.entry(open).or_default().text = self.input.clone();
        }
        for (chat, list) in &self.attach.files {
            all.entry(chat).or_default().files =
                list.iter().map(|a| SavedFile { path: a.path.clone(), name: a.name.clone(), kind: a.kind, temp: a.temp }).collect();
        }
        all.retain(|_, s| !s.text.trim().is_empty() || !s.files.is_empty());
        serde_json::to_string(&all).unwrap_or_default()
    }

    /// Writes the drafts when they changed, at most once a second.
    pub fn drafts_tick(&mut self) {
        if self.drafts.last_write.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.save_drafts();
    }

    pub fn save_drafts(&mut self) {
        if !self.drafts.restored {
            return;
        }
        let snap = self.snapshot();
        if snap == self.drafts.written {
            return;
        }
        self.drafts.last_write = Some(Instant::now());
        let path = file();
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &snap).and_then(|_| std::fs::rename(&tmp, &path)).is_ok() {
            self.drafts.written = snap;
        }
    }

    /// Reads the last run's drafts off the UI thread (pictures need their
    /// thumbnails), dropping files gone since and pasted ones no draft holds.
    pub fn load_drafts(&self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let saved: HashMap<String, Saved> =
                std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            let mut texts = HashMap::new();
            let mut files: HashMap<String, Vec<Attachment>> = HashMap::new();
            let mut kept = Vec::new();
            for (chat, s) in saved {
                if !s.text.trim().is_empty() {
                    texts.insert(chat.clone(), s.text);
                }
                for f in s.files {
                    // As the menu choice that gives it the same kind.
                    let pick = match f.kind {
                        Kind::Document => 0,
                        Kind::Image | Kind::Video => 1,
                        Kind::Audio => 2,
                    };
                    if let Some(mut a) = prepare(&f.path, pick) {
                        (a.name, a.temp) = (f.name, f.temp);
                        kept.push(a.path.clone());
                        files.entry(chat.clone()).or_default().push(a);
                    }
                }
            }
            if let Ok(dir) = std::fs::read_dir(crate::attach::paste_dir()) {
                for e in dir.flatten().filter(|e| !kept.contains(&e.path())) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            let _ = tx.send(crate::Ev::Attach(Ev::Restored { texts, files }));
        });
    }

    /// The last run's texts, under what's been typed since.
    pub fn restore_drafts(&mut self, texts: HashMap<String, String>) {
        for (chat, text) in texts {
            if self.open.as_deref() == Some(chat.as_str()) {
                if self.input.is_empty() {
                    self.input = text;
                    self.caret = self.input.chars().count();
                }
            } else {
                self.drafts.texts.entry(chat).or_insert(text);
            }
        }
        self.drafts.restored = true;
    }
}
