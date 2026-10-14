//! A message's menu, WhatsApp Web's chevron in the corner of the hovered
//! bubble (or a right-click on it): reply, copy, edit yours, react, forward,
//! delete. A reply waits over the message box, one per chat, until sent.
//! Delete goes into selecting messages, a checkbox by each, and asks whether
//! for you or for everyone, the latter only where WhatsApp allows it: your
//! own messages, recent enough, not in the chat with yourself. Forward
//! selects the same way, then asks where to (`forward`).

use crate::app::{App, Phase};
use crate::attach::{MENU_BG, MENU_HOVER};
use crate::bridge::Msg;
use crate::chatmenu::{boxed, separator};
use crate::react::SMILEY;
use crate::ui::{box_corners, display_name, draw_deco, green, name_color, truncate, wrap, BL, CAP_L, CAP_R, DAY_PILL, META_IN, TEXT, TL};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use unicode_width::UnicodeWidthStr;

// Nerd Font icons.
const CHEVRON: &str = "\u{f107}";
const REPLY: &str = "\u{f112}";
const COPY: &str = "\u{f0c5}";
const EDIT: &str = "\u{f040}";
const TRASH: &str = "\u{f1f8}";
const CLOSE: &str = "\u{f00d}";
pub(crate) const BOX_OFF: &str = "\u{f0131}";
pub(crate) const BOX_ON: &str = "\u{f0135}";

/// The column the checkboxes take while selecting.
pub const CHECK_W: u16 = 6;
/// A selected message's rows.
const PICKED: Color = Color::Rgb(0x2E, 0x2F, 0x2F);

/// WhatsApp takes edits for 15 minutes, and deletes for everyone for
/// about two and a half days.
const EDIT_SECS: i64 = 15 * 60;
const REVOKE_SECS: i64 = 60 * 3600;

/// What the bridge writes for a deleted or edited message.
const REVOKED: &str = "🚫 ";
const REVOKED_MINE: &str = "🚫 Você apagou esta mensagem";
pub(crate) const EDITED: &str = " (editada)";

#[derive(Clone, Copy, PartialEq)]
enum Act {
    Reply,
    Copy,
    Edit,
    React,
    Forward,
    Sep,
    Delete,
}

#[derive(Clone, Copy, PartialEq)]
enum Choice {
    Cancel,
    Me,
    Everyone,
}

struct Popup {
    chat: String,
    id: String,
    sel: Act,
    /// The cell it hangs under, and whether its right edge lines up there
    /// (the chevron) or its left (the pointer).
    at: Position,
    right: bool,
}

/// Messages picked to delete, in the open chat.
pub struct Select {
    pub chat: String,
    pub ids: Vec<String>,
    /// To forward, not to delete: only what can go is picked.
    pub forward: bool,
    /// The message Space picks, moved by ↑↓.
    pub at: Option<String>,
}

/// One of your messages being edited in the message box; what was typed
/// there before comes back after.
pub struct Editing {
    pub chat: String,
    pub id: String,
    pub original: String,
    saved: (String, usize),
}

#[derive(Default)]
pub struct MsgMenu {
    popup: Option<Popup>,
    pub select: Option<Select>,
    /// The delete dialog over the selection, with its highlighted button.
    confirm: Option<Choice>,
    pub editing: Option<Editing>,
    /// "Encaminhar": the dialog of chats to forward the selection to.
    pub fwd: crate::forward::Forward,
    /// The message each chat's next message replies to.
    pub replies: HashMap<String, String>,
    // Where the last draw put things, for the mouse.
    chevron: Option<(Rect, String)>,
    rows: Vec<(Rect, Act)>,
    area: Rect,
    close_btn: Rect,
    trash_btn: Rect,
    edit_close: Rect,
    reply_close: Rect,
    dialog: Rect,
    buttons: Vec<(Rect, Choice)>,
}

impl MsgMenu {
    pub fn selecting(&self, chat: &str) -> bool {
        self.select.as_ref().is_some_and(|s| s.chat == chat)
    }

    pub fn editing(&self, chat: &str) -> bool {
        self.editing.as_ref().is_some_and(|e| e.chat == chat)
    }
}

fn revoked(m: &Msg) -> bool {
    m.media.is_none() && m.text.starts_with(REVOKED)
}

/// What the bridge writes for a message whose content it didn't keep (a
/// contact, a place, a poll, media from before the keys were kept).
const LABELS: [&str; 8] = ["📷 Foto", "🎥 Vídeo", "🎤 Áudio", "📄 ", "💟 Figurinha", "👤 ", "📍 Localização", "📊 "];

/// Whether it can be forwarded: not deleted nor a business's card, its
/// media with keys (ours once uploaded), its content kept.
pub(crate) fn forwardable(m: &Msg) -> bool {
    if m.card.is_some() || revoked(m) {
        return false;
    }
    match &m.media {
        Some(_) => !m.unavailable && !(m.from_me && m.status == 0),
        None => !m.text.trim().is_empty() && !LABELS.iter().any(|l| m.text.starts_with(l)),
    }
}

/// The text a copy takes: a photo's or a file's caption, not its label.
pub(crate) fn plain(m: &Msg) -> String {
    let t = m.text.strip_suffix(EDITED).unwrap_or(&m.text);
    match &m.media {
        Some(_) => t.split_once(" · ").map(|(_, c)| c.to_string()).unwrap_or_default(),
        None if revoked(m) => String::new(),
        None => t.to_string(),
    }
}

fn label(a: Act) -> (&'static str, &'static str) {
    match a {
        Act::Reply => (REPLY, "Responder"),
        Act::Copy => (COPY, "Copiar"),
        Act::Edit => (EDIT, "Editar"),
        Act::React => (SMILEY, "Reagir"),
        Act::Forward => (crate::forward::ICON, "Encaminhar"),
        Act::Delete => (TRASH, "Apagar"),
        Act::Sep => ("", ""),
    }
}

fn choice_label(c: Choice) -> &'static str {
    match c {
        Choice::Cancel => "Cancelar",
        Choice::Me => "Apagar para mim",
        Choice::Everyone => "Apagar para todos",
    }
}

fn step(items: &[Act], cur: Act, delta: isize) -> Act {
    let n = items.len() as isize;
    let mut i = items.iter().position(|x| *x == cur).unwrap_or(0) as isize;
    for _ in 0..n {
        i = (i + delta).rem_euclid(n);
        if items[i as usize] != Act::Sep {
            break;
        }
    }
    items[i as usize]
}

impl App {
    fn find_msg(&self, chat: &str, id: &str) -> Option<&Msg> {
        self.msgs.get(chat).and_then(|l| l.iter().find(|m| m.id == id))
    }

    /// The chat with yourself: what's deleted there is only ever for you.
    fn self_chat(&self, chat: &str) -> bool {
        !self.me.is_empty() && chat.strip_suffix("@s.whatsapp.net") == Some(self.me.as_str())
    }

    fn writable(&self, chat: &str) -> bool {
        self.chat(chat).is_none_or(|c| c.read_only.is_empty())
    }

    fn editable(&self, chat: &str, m: &Msg) -> bool {
        m.from_me
            && m.media.is_none()
            && m.card.is_none()
            && !revoked(m)
            && chrono::Utc::now().timestamp() - m.ts < EDIT_SECS
            && self.writable(chat)
    }

    fn acts(&self, chat: &str, id: &str) -> Vec<Act> {
        let Some(m) = self.find_msg(chat, id) else { return Vec::new() };
        let mut v = Vec::new();
        if !revoked(m) && self.writable(chat) {
            v.push(Act::Reply);
        }
        if !plain(m).is_empty() {
            v.push(Act::Copy);
        }
        if self.editable(chat, m) {
            v.push(Act::Edit);
        }
        if !revoked(m) && self.writable(chat) {
            v.push(Act::React);
        }
        if forwardable(m) {
            v.push(Act::Forward);
        }
        if !v.is_empty() {
            v.push(Act::Sep);
        }
        v.push(Act::Delete);
        v
    }

    /// The dialog's buttons: for everyone only when each picked message
    /// can still be deleted for everyone.
    fn choices(&self) -> Vec<Choice> {
        let Some(s) = self.msg_menu.select.as_ref() else { return Vec::new() };
        let now = chrono::Utc::now().timestamp();
        let everyone = !self.self_chat(&s.chat)
            && self.writable(&s.chat)
            && s.ids.iter().all(|id| self.find_msg(&s.chat, id).is_some_and(|m| m.from_me && !revoked(m) && now - m.ts < REVOKE_SECS));
        let mut v = vec![Choice::Cancel, Choice::Me];
        if everyone {
            v.push(Choice::Everyone);
        }
        v
    }

    fn open_msg_menu(&mut self, id: String, at: Position, right: bool) {
        let Some(chat) = self.open.clone() else { return };
        self.react_bar = None;
        self.chat_menu.popup = None;
        self.account.menu = None;
        self.attach.menu = None;
        self.emoji_panel.open = false;
        let sel = self.acts(&chat, &id).first().copied().unwrap_or(Act::Delete);
        self.msg_menu.popup = Some(Popup { chat, id, sel, at, right });
    }

    fn choose_msg_act(&mut self, a: Act) {
        let Some(p) = self.msg_menu.popup.take() else { return };
        match a {
            Act::Sep => self.msg_menu.popup = Some(p),
            Act::Reply => self.start_reply(&p.chat, &p.id),
            Act::Copy => {
                let text = self.find_msg(&p.chat, &p.id).map(plain).unwrap_or_default();
                copy(&text);
                self.note("Mensagem copiada");
            }
            Act::Edit => self.start_edit(&p.chat, &p.id),
            Act::React => {
                let anchor = self.bubbles.iter().find(|h| h.id == p.id).map(|h| Rect { height: 1, ..h.rect });
                self.open_react_bar(p.id, anchor.unwrap_or(Rect::new(p.at.x, p.at.y, 1, 1)));
            }
            Act::Forward => {
                self.cancel_edit();
                self.msg_menu.select = Some(Select { chat: p.chat, ids: vec![p.id.clone()], forward: true, at: Some(p.id) });
                self.msg_menu.confirm = None;
            }
            Act::Delete => {
                self.cancel_edit();
                self.msg_menu.select = Some(Select { chat: p.chat, ids: vec![p.id.clone()], forward: false, at: Some(p.id) });
                self.msg_menu.confirm = None;
            }
        }
    }

    /// The message the open chat's reply quotes, while it's there to quote.
    fn reply_msg(&self) -> Option<&Msg> {
        let chat = self.open.as_ref()?;
        let id = self.msg_menu.replies.get(chat)?;
        self.find_msg(chat, id).filter(|m| !revoked(m))
    }

    pub fn replying(&self) -> bool {
        self.reply_msg().is_some()
    }

    /// "Responder": the message waits over the box, which takes the keys.
    fn start_reply(&mut self, chat: &str, id: &str) {
        self.cancel_edit();
        self.msg_menu.replies.insert(chat.into(), id.into());
        self.focus_input();
    }

    pub fn cancel_reply(&mut self) {
        if let Some(chat) = self.open.clone() {
            self.msg_menu.replies.remove(&chat);
        }
    }

    /// Sending: the message the chat's reply quotes, if it's still there.
    pub fn take_reply(&mut self, chat: &str) -> Option<String> {
        let id = self.msg_menu.replies.remove(chat)?;
        self.find_msg(chat, &id).filter(|m| !revoked(m)).map(|m| m.id.clone())
    }

    /// Alt+R: a reply to the last message; again, to the one before.
    fn reply_step(&mut self) {
        let Some(chat) = self.open.clone() else { return };
        if !self.writable(&chat) {
            return;
        }
        let list: Vec<&Msg> = self.msgs.get(&chat).map(|l| l.iter().filter(|m| !revoked(m)).collect()).unwrap_or_default();
        let cur = self.reply_msg().map(|m| m.id.clone());
        let at = cur.and_then(|id| list.iter().position(|m| m.id == id)).unwrap_or(list.len());
        let Some(id) = at.checked_sub(1).map(|i| list[i].id.clone()) else { return };
        self.start_reply(&chat, &id);
    }

    /// Up in an empty message box: your last message, if it can still be edited.
    pub fn edit_last(&mut self) -> bool {
        let Some(chat) = self.open.clone() else { return false };
        let last = self.msgs.get(&chat).and_then(|l| l.iter().rev().find(|m| m.from_me)).cloned();
        match last.filter(|m| self.editable(&chat, m)) {
            Some(m) => {
                self.start_edit(&chat, &m.id);
                true
            }
            None => false,
        }
    }

    fn start_edit(&mut self, chat: &str, id: &str) {
        let Some(m) = self.find_msg(chat, id).cloned() else { return };
        if !self.editable(chat, &m) {
            return self.note("Essa mensagem não pode mais ser editada");
        }
        self.cancel_edit();
        let text = plain(&m);
        let saved = (std::mem::replace(&mut self.input, text.clone()), self.caret);
        self.caret = text.chars().count();
        self.msg_menu.editing = Some(Editing { chat: chat.into(), id: id.into(), original: text, saved });
        self.scroll = 0;
        self.focus_input();
    }

    /// Out of the edit: what was typed before comes back.
    pub fn cancel_edit(&mut self) {
        if let Some(e) = self.msg_menu.editing.take() {
            (self.input, self.caret) = e.saved;
        }
    }

    /// Enter while editing: the new text goes, unless it's the same or empty.
    pub fn send_edit(&mut self) {
        let Some(e) = self.msg_menu.editing.as_ref() else { return };
        let (chat, id) = (e.chat.clone(), e.id.clone());
        let text = self.input.trim().to_string();
        if text.is_empty() || text == e.original.trim() {
            return self.cancel_edit();
        }
        if self.phase != Phase::Ready {
            return self.note("Ainda conectando; tente de novo em instantes.");
        }
        let ok = self.find_msg(&chat, &id).cloned().is_some_and(|m| self.editable(&chat, &m));
        if !ok {
            self.cancel_edit();
            return self.note("Essa mensagem não pode mais ser editada");
        }
        if let Some(m) = self.msgs.get_mut(&chat).and_then(|l| l.iter_mut().find(|m| m.id == id)) {
            m.text = format!("{text}{EDITED}");
        }
        self.cmd(json!({"c": "edit", "chat": chat, "id": id, "text": text}));
        self.cancel_edit();
    }

    fn toggle_pick(&mut self, id: &str) {
        let ok = self.msg_menu.select.as_ref().is_some_and(|s| !s.forward || self.find_msg(&s.chat, id).is_some_and(forwardable));
        if let Some(s) = self.msg_menu.select.as_mut().filter(|_| ok) {
            s.at = Some(id.to_string());
            match s.ids.iter().position(|x| x == id) {
                Some(i) => {
                    s.ids.remove(i);
                }
                None => s.ids.push(id.to_string()),
            }
        }
    }

    /// ↑↓ while selecting: the message Space picks, the ones it can pick
    /// only; off screen, it's brought to the middle.
    fn pick_step(&mut self, delta: isize) {
        let Some(s) = self.msg_menu.select.as_ref() else { return };
        let ids: Vec<&str> = self.msgs.get(&s.chat).map(|l| l.iter().filter(|m| !s.forward || forwardable(m)).map(|m| m.id.as_str()).collect()).unwrap_or_default();
        let Some(last) = ids.len().checked_sub(1) else { return };
        let i = match s.at.as_ref().and_then(|a| ids.iter().position(|x| x == a)) {
            Some(i) => (i as isize + delta).clamp(0, last as isize) as usize,
            None => last,
        };
        let id = ids[i].to_string();
        let area = self.msgs_area;
        let whole = self.bubbles.iter().find(|h| h.id == id).is_some_and(|h| h.rect.y > area.y && h.rect.bottom() < area.bottom());
        if !whole {
            self.jump = Some(id.clone());
        }
        if let Some(s) = self.msg_menu.select.as_mut() {
            s.at = Some(id);
        }
    }

    fn ask_delete(&mut self) {
        if self.msg_menu.select.as_ref().is_some_and(|s| !s.ids.is_empty()) {
            self.msg_menu.confirm = Some(Choice::Me);
        }
    }

    fn decide(&mut self, c: Choice) {
        self.msg_menu.confirm = None;
        let everyone = match c {
            Choice::Cancel => return,
            Choice::Me => false,
            Choice::Everyone => true,
        };
        if everyone && self.phase != Phase::Ready {
            return self.note("Ainda conectando; tente de novo em instantes.");
        }
        let Some(s) = self.msg_menu.select.take() else { return };
        if self.msg_menu.editing.as_ref().is_some_and(|e| s.ids.contains(&e.id)) {
            self.cancel_edit();
        }
        if let Some(list) = self.msgs.get_mut(&s.chat) {
            if everyone {
                for m in list.iter_mut().filter(|m| s.ids.contains(&m.id)) {
                    m.text = REVOKED_MINE.into();
                    m.media = None;
                    m.card = None;
                    m.reactions.clear();
                }
            } else {
                list.retain(|m| !s.ids.contains(&m.id));
            }
        }
        self.cmd(json!({"c": "delete_msgs", "chat": s.chat, "ids": s.ids, "everyone": everyone}));
    }

    /// Messages deleted for you, here or on another device.
    pub fn removed(&mut self, chat: &str, ids: &[String]) {
        if let Some(list) = self.msgs.get_mut(chat) {
            list.retain(|m| !ids.contains(&m.id));
        }
        if let Some(s) = self.msg_menu.select.as_mut().filter(|s| s.chat == chat) {
            s.ids.retain(|id| !ids.contains(id));
        }
        if self.msg_menu.popup.as_ref().is_some_and(|p| p.chat == chat && ids.contains(&p.id)) {
            self.msg_menu.popup = None;
        }
        if self.msg_menu.editing.as_ref().is_some_and(|e| e.chat == chat && ids.contains(&e.id)) {
            self.cancel_edit();
        }
    }

    /// The menu's, the dialog's and the selection's keys. True when taken.
    pub fn msg_menu_key(&mut self, k: KeyEvent) -> bool {
        if self.msg_menu.fwd.picker.is_some() {
            return self.forward_key(k);
        }
        let go = matches!(k.code, KeyCode::Enter | KeyCode::Char(' '));
        if let Some(p) = self.msg_menu.popup.as_ref() {
            let items = self.acts(&p.chat, &p.id);
            let Some(p) = self.msg_menu.popup.as_mut() else { return false };
            match k.code {
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => p.sel = step(&items, p.sel, -1),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => p.sel = step(&items, p.sel, 1),
                KeyCode::Esc | KeyCode::Char('q') => self.msg_menu.popup = None,
                _ if go => {
                    let sel = p.sel;
                    self.choose_msg_act(sel);
                }
                _ => {}
            }
            return true;
        }
        if let Some(cur) = self.msg_menu.confirm {
            let items = self.choices();
            let i = items.iter().position(|c| *c == cur).unwrap_or(0);
            let n = items.len();
            match k.code {
                KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => self.msg_menu.confirm = Some(items[(i + n - 1) % n]),
                KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => self.msg_menu.confirm = Some(items[(i + 1) % n]),
                KeyCode::Esc => self.msg_menu.confirm = None,
                KeyCode::Enter => self.decide(cur),
                _ => {}
            }
            return true;
        }
        if self.msg_menu.select.is_none() && k.modifiers.contains(KeyModifiers::ALT) && matches!(k.code, KeyCode::Char('r' | 'R')) {
            self.reply_step();
            return true;
        }
        if let Some(s) = self.msg_menu.select.as_ref() {
            let forward = s.forward;
            match k.code {
                KeyCode::Esc => self.msg_menu.select = None,
                KeyCode::Up | KeyCode::Char('k') => self.pick_step(-1),
                KeyCode::Down | KeyCode::Char('j') => self.pick_step(1),
                KeyCode::Char(' ') => {
                    if let Some(id) = s.at.clone() {
                        self.toggle_pick(&id);
                    }
                }
                KeyCode::Enter if forward => self.open_forward(),
                KeyCode::Enter | KeyCode::Delete | KeyCode::Backspace if !forward => self.ask_delete(),
                // The conversation still scrolls.
                KeyCode::PageUp | KeyCode::PageDown => return false,
                _ => {}
            }
            return true;
        }
        false
    }

    /// The chevron, a right-click on a bubble, the menu, the dialog and the
    /// selection's clicks. True when the event was theirs.
    pub fn msg_menu_mouse(&mut self, m: MouseEvent) -> bool {
        if self.msg_menu.fwd.picker.is_some() {
            return self.forward_mouse(m);
        }
        let at = Position::new(m.column, m.row);
        let down = matches!(m.kind, MouseEventKind::Down(_));
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        let menu = &self.msg_menu;
        if let Some(p) = menu.popup.as_ref() {
            let hit = menu.rows.iter().find(|(r, _)| r.contains(at)).map(|(_, a)| *a);
            let inside = menu.area.contains(at);
            let sel = p.sel;
            match m.kind {
                MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                    if let (Some(a), Some(p)) = (hit.filter(|a| *a != Act::Sep && *a != sel), self.msg_menu.popup.as_mut()) {
                        p.sel = a;
                    }
                }
                MouseEventKind::Down(MouseButton::Left) if hit.is_some() => self.choose_msg_act(hit.unwrap_or(Act::Sep)),
                MouseEventKind::Down(b) if !inside => {
                    self.msg_menu.popup = None;
                    // A right-click on another bubble opens its menu instead.
                    if b == MouseButton::Right {
                        return self.msg_menu_mouse(m);
                    }
                }
                _ => {}
            }
            return true;
        }
        if let Some(cur) = menu.confirm {
            let hit = menu.buttons.iter().find(|(r, _)| r.contains(at)).map(|(_, c)| *c);
            match m.kind {
                MouseEventKind::Moved => {
                    if let Some(c) = hit.filter(|c| *c != cur) {
                        self.msg_menu.confirm = Some(c);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) if hit.is_some() => self.decide(hit.unwrap_or(Choice::Cancel)),
                MouseEventKind::Down(_) if !menu.dialog.contains(at) => self.msg_menu.confirm = None,
                _ => {}
            }
            return true;
        }
        if menu.select.is_some() {
            if !down {
                return false;
            }
            if left && menu.close_btn.contains(at) {
                self.msg_menu.select = None;
            } else if left && menu.trash_btn.contains(at) {
                if menu.select.as_ref().is_some_and(|s| s.forward) {
                    self.open_forward();
                } else {
                    self.ask_delete();
                }
            } else if self.newest_btn.contains(at) || !(self.msgs_area.contains(at) || self.input_area.contains(at)) {
                return false;
            } else if left {
                let id = self.bubbles.iter().find(|h| at.y >= h.rect.y && at.y < h.rect.bottom()).map(|h| h.id.clone());
                if let Some(id) = id.filter(|_| self.msgs_area.contains(at)) {
                    self.toggle_pick(&id);
                }
            }
            return true;
        }
        if left {
            if let Some((r, id)) = menu.chevron.clone().filter(|(r, _)| r.contains(at)) {
                self.open_msg_menu(id, Position::new(r.right(), r.bottom()), true);
                return true;
            }
            if self.msg_menu.editing.is_some() && self.msg_menu.edit_close.contains(at) {
                self.cancel_edit();
                return true;
            }
            if self.replying() && self.msg_menu.reply_close.contains(at) {
                self.cancel_reply();
                return true;
            }
        }
        if m.kind == MouseEventKind::Down(MouseButton::Right) && self.msgs_area.contains(at) {
            let Some(id) = self.bubbles.iter().find(|h| h.rect.contains(at)).map(|h| h.id.clone()) else { return false };
            self.open_msg_menu(id, Position::new(at.x, at.y + 1), false);
            return true;
        }
        false
    }
}

/// Copies to the clipboard: wl-copy or xclip, else the terminal's OSC 52.
pub(crate) fn copy(text: &str) {
    use std::process::{Command, Stdio};
    let mut cmd = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Command::new("wl-copy")
    } else {
        let mut c = Command::new("xclip");
        c.args(["-selection", "clipboard"]);
        c
    };
    if let Ok(mut child) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        // They stay to serve the clipboard: reaped off the UI thread.
        std::thread::spawn(move || child.wait());
        return;
    }
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = out.flush();
}

fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// The chevron in the top right corner of the hovered bubble (or the one
/// whose menu is open), on its color.
pub fn draw_chevron(f: &mut Frame, app: &mut App) {
    app.msg_menu.chevron = None;
    if app.msg_menu.select.is_some() || app.react_who.is_some() {
        return;
    }
    let id = app.msg_menu.popup.as_ref().map(|p| p.id.clone()).or_else(|| app.hover_msg.clone());
    let Some(h) = id.and_then(|id| app.bubbles.iter().find(|h| h.id == id)) else { return };
    if h.rect.width < 6 {
        return;
    }
    let (x, y) = (h.rect.right() - 2, h.rect.y);
    let bg = f.buffer_mut()[(x - 1, y)].bg;
    f.buffer_mut()[(x, y)].set_symbol(CHEVRON).set_fg(META_IN).set_bg(bg);
    app.msg_menu.chevron = Some((Rect::new(x - 1, y, 2, 1), h.id.clone()));
}

/// While selecting: a checkbox by each bubble, the picked ones' rows lit.
pub fn draw_checks(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(s) = app.msg_menu.select.as_ref() else { return };
    for h in &app.bubbles {
        // Forwarding, what can't go has no checkbox.
        if s.forward && !app.find_msg(&s.chat, &h.id).is_some_and(forwardable) {
            continue;
        }
        let on = s.ids.contains(&h.id);
        if on {
            for y in crate::ui::lit_rows(h.rect, area, app.picker.is_some()) {
                for x in area.left()..area.right() {
                    let cell = &mut f.buffer_mut()[(x, y)];
                    if cell.bg == Color::Reset {
                        cell.set_bg(PICKED);
                    }
                }
            }
        }
        let (glyph, fg) = if on { (BOX_ON, green()) } else { (BOX_OFF, META_IN) };
        let y = h.rect.y + h.rect.height.saturating_sub(1) / 2;
        f.buffer_mut()[(area.x + 2, y)].set_symbol(glyph).set_fg(fg);
        // The one Space picks.
        if s.at.as_ref() == Some(&h.id) {
            f.buffer_mut()[(area.x, y)].set_symbol("▸").set_fg(green());
        }
    }
}

/// Instead of the message box while selecting: ✕, how many, the trash (or
/// forwarding, the forward button).
pub fn draw_select_bar(f: &mut Frame, app: &mut App, area: Rect) {
    let n = app.msg_menu.select.as_ref().map_or(0, |s| s.ids.len());
    let forward = app.msg_menu.select.as_ref().is_some_and(|s| s.forward);
    let bg = Style::new().bg(DAY_PILL);
    let count = match n {
        1 => "1 selecionada".to_string(),
        0 if forward => "Selecionar mensagens".to_string(),
        _ if forward => format!("{n} itens selecionados"),
        _ => format!("{n} selecionadas"),
    };
    let trash = if n > 0 { bg.fg(TEXT) } else { bg.fg(META_IN) };
    let w = area.width as usize;
    let used = 8 + count.width();
    let mut mid = vec![Span::styled("   ", bg), Span::styled(CLOSE, bg.fg(TEXT)), Span::styled("    ", bg), Span::styled(count, bg.fg(TEXT))];
    mid.push(Span::styled(" ".repeat(w.saturating_sub(used + 4)), bg));
    mid.push(Span::styled(if forward { crate::forward::ICON } else { TRASH }, trash));
    mid.push(Span::styled("   ", bg));
    let blank = Line::from(Span::styled(" ".repeat(w), bg));
    let mut lines = vec![blank.clone(); area.height as usize];
    if let Some(l) = lines.get_mut(area.height as usize / 2) {
        *l = Line::from(mid);
    }
    f.render_widget(Paragraph::new(lines), area);
    let y = area.y + area.height / 2;
    app.msg_menu.close_btn = Rect::new(area.x + 2, y, 3, 1);
    app.msg_menu.trash_btn = Rect::new(area.right().saturating_sub(5), y, 3, 1);
}

/// Over the message box while editing: what's edited, and ✕ to stop.
pub fn draw_edit_strip(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(e) = app.msg_menu.editing.as_ref() else { return };
    let w = area.width as usize;
    let title = format!("   {EDIT}  Editar mensagem");
    let first = e.original.lines().next().unwrap_or("").to_string();
    let lines = vec![
        Line::from(vec![
            Span::styled(title.clone(), Style::new().fg(green()).add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(w.saturating_sub(title.width() + 4))),
            Span::styled(CLOSE, Style::new().fg(META_IN)),
        ]),
        Line::from(Span::styled(format!("      {}", truncate(&first, w.saturating_sub(10))), Style::new().fg(META_IN))),
    ];
    f.render_widget(Paragraph::new(lines), area);
    app.msg_menu.edit_close = Rect::new(area.right().saturating_sub(5), area.y, 3, 1);
}

/// What a reply shows over the message box, its text `width` wide: who
/// wrote the message, in their color, and up to two lines of it. None
/// while editing, which has the box.
fn reply_view(app: &App, width: usize) -> Option<(String, Color, Vec<String>)> {
    if app.open.as_ref().is_some_and(|c| app.msg_menu.editing(c)) {
        return None;
    }
    let m = app.reply_msg()?;
    let who = if m.from_me { "Você".to_string() } else { display_name(&m.sender_name, &m.sender) };
    let text = m.text.strip_suffix(EDITED).unwrap_or(&m.text);
    let (text, _) = crate::markup::mentions(text, &m.mentions);
    let mut lines = wrap(&text, width.max(1));
    if lines.len() > 2 {
        lines[1] = truncate(&format!("{}…", lines[1].trim_end()), width);
    }
    lines.truncate(2);
    Some((who, name_color(&m.sender), lines))
}

/// Inside the message box, over the text: the reply's rows and a blank one
/// under them; `width` is the box's area's.
pub fn reply_rows(app: &App, width: u16) -> u16 {
    reply_view(app, width.saturating_sub(14) as usize).map_or(0, |(_, _, l)| l.len() as u16 + 2)
}

/// The reply over the message box, as WhatsApp Web's: a bar in the author's
/// color, their name and the message on a darker box, ✕ to stop replying.
pub fn draw_reply(f: &mut Frame, app: &mut App, area: Rect) {
    app.msg_menu.reply_close = Rect::default();
    let w = area.width as usize;
    let Some((who, color, text)) = reply_view(app, w.saturating_sub(8)) else { return };
    let bg = Style::new().bg(DAY_PILL);
    let close_row = text.len() / 2;
    let rows = std::iter::once((who, bg.fg(color).add_modifier(Modifier::BOLD))).chain(text.into_iter().map(|l| (l, bg.fg(META_IN))));
    let lines: Vec<Line> = rows
        .enumerate()
        .map(|(i, (s, st))| {
            let s = truncate(&s, w.saturating_sub(8));
            let close = if i == close_row { CLOSE } else { " " };
            Line::from(vec![
                Span::styled("▎ ", bg.fg(color)),
                Span::styled(s.clone(), st),
                Span::styled(" ".repeat(w.saturating_sub(5 + s.width())), bg),
                Span::styled(close, bg.fg(META_IN)),
                Span::styled("  ", bg),
            ])
        })
        .collect();
    let h = lines.len();
    f.render_widget(Paragraph::new(lines), area);
    if app.picker.is_some() && w > 2 {
        for (row, col, d) in box_corners(0, h, 0, w, DAY_PILL, TL | BL) {
            draw_deco(f, app, d, area.x + col as u16, area.y + row as u16);
        }
    }
    app.msg_menu.reply_close = Rect::new(area.right().saturating_sub(4), area.y + close_row as u16, 3, 1);
}

/// The menu, and the delete dialog over the selection.
pub fn draw(f: &mut Frame, app: &mut App) {
    // Gone from the screen: a selection or edit of another chat ends.
    if app.msg_menu.select.as_ref().is_some_and(|s| app.open.as_ref() != Some(&s.chat)) {
        app.msg_menu.select = None;
        app.msg_menu.confirm = None;
    }
    if app.msg_menu.editing.as_ref().is_some_and(|e| app.open.as_ref() != Some(&e.chat)) {
        app.msg_menu.editing = None;
    }
    if app.msg_menu.select.is_none() {
        app.msg_menu.confirm = None;
    }
    draw_popup(f, app);
    draw_dialog(f, app);
    crate::forward::draw(f, app);
}

fn draw_popup(f: &mut Frame, app: &mut App) {
    app.msg_menu.rows.clear();
    app.msg_menu.area = Rect::default();
    let Some(p) = app.msg_menu.popup.as_ref() else { return };
    let (chat, id, sel, at, right) = (p.chat.clone(), p.id.clone(), p.sel, p.at, p.right);
    if app.open.as_ref() != Some(&chat) || app.find_msg(&chat, &id).is_none() {
        app.msg_menu.popup = None;
        return;
    }
    let items = app.acts(&chat, &id);
    let screen = f.area();
    let lw = items.iter().map(|a| label(*a).1.width()).max().unwrap_or(0) as u16;
    let w = (lw + 14).min(screen.width);
    let h = (items.len() as u16 + 2).min(screen.height);
    let x = if right { at.x.saturating_sub(w) } else { at.x };
    let x = x.clamp(screen.x, screen.right().saturating_sub(w));
    let y = if at.y + h <= screen.bottom() { at.y } else { at.y.saturating_sub(h + 1).max(screen.y) };
    let area = Rect::new(x, y, w, h);
    let lines: Vec<Line<'static>> = items
        .iter()
        .map(|&a| {
            if a == Act::Sep {
                return separator(w);
            }
            let row = if a == sel { Style::new().bg(MENU_HOVER) } else { Style::new().bg(MENU_BG) };
            let (icon, text) = label(a);
            let fg = if a == Act::Delete { Color::Rgb(0xF1, 0x5C, 0x6D) } else { TEXT };
            let pad = (w as usize).saturating_sub(7 + text.width());
            Line::from(vec![
                Span::styled(" ", Style::new().bg(MENU_BG)),
                Span::styled("  ", row),
                Span::styled(icon, row.fg(META_IN)),
                Span::styled("  ", row),
                Span::styled(text, row.fg(fg)),
                Span::styled(" ".repeat(pad), row),
                Span::styled(" ", Style::new().bg(MENU_BG)),
            ])
        })
        .collect();
    for (n, a) in items.iter().enumerate() {
        if *a != Act::Sep {
            app.msg_menu.rows.push((Rect::new(x, y + 1 + n as u16, w, 1), *a));
        }
    }
    app.msg_menu.area = area;
    boxed(f, app, area, lines);
}

/// "Deseja apagar a mensagem?", at the top of the conversation, as WhatsApp
/// Web asks it.
fn draw_dialog(f: &mut Frame, app: &mut App) {
    app.msg_menu.buttons.clear();
    app.msg_menu.dialog = Rect::default();
    let Some(cur) = app.msg_menu.confirm else { return };
    let n = app.msg_menu.select.as_ref().map_or(0, |s| s.ids.len());
    let choices = app.choices();
    let title = if n == 1 { "Deseja apagar a mensagem?".to_string() } else { format!("Deseja apagar {n} mensagens?") };
    let btn_w = |c: Choice| choice_label(c).width() + 4;
    let row_w: usize = choices.iter().map(|c| btn_w(*c) + 2).sum::<usize>();
    let area = app.msgs_area;
    let w = (row_w.max(title.width()) + 8).min(area.width as usize) as u16;
    let h = 6u16.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let rect = Rect::new(x, area.y + 1, w, h);
    let bg = Style::new().bg(MENU_BG);
    let blank = || Line::from(Span::styled(" ".repeat(w as usize), bg));
    let mut spans = vec![Span::styled(" ".repeat((w as usize).saturating_sub(row_w + 3)), bg)];
    let mut bx = rect.x + (w as u16).saturating_sub(row_w as u16 + 3);
    let y = rect.y + 4;
    for c in &choices {
        let lit = *c == cur;
        let (fg, fill) = if lit { (Color::Rgb(0x11, 0x1B, 0x21), green()) } else { (green(), MENU_HOVER) };
        let text = Style::new().fg(fg).bg(fill).add_modifier(Modifier::BOLD);
        spans.extend([
            Span::styled(CAP_L, Style::new().fg(fill).bg(MENU_BG)),
            Span::styled(format!(" {} ", choice_label(*c)), text),
            Span::styled(CAP_R, Style::new().fg(fill).bg(MENU_BG)),
            Span::styled("  ", bg),
        ]);
        app.msg_menu.buttons.push((Rect::new(bx, y, btn_w(*c) as u16, 1), *c));
        bx += btn_w(*c) as u16 + 2;
    }
    spans.push(Span::styled("   ", bg));
    let title_pad = (w as usize).saturating_sub(3 + title.width());
    let lines = vec![
        Line::from(vec![
            Span::styled("   ", bg),
            Span::styled(title, bg.fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled(" ".repeat(title_pad), bg),
        ]),
        blank(),
        blank(),
        Line::from(spans),
    ];
    f.render_widget(Clear, rect);
    app.msg_menu.dialog = rect;
    boxed(f, app, rect, lines);
}
