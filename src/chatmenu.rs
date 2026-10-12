//! The chat menu, WhatsApp Web's chevron on a chat row: archive, pin, mark
//! read or unread, mute, favorite, clear and delete. A right-click on a
//! chat, the chevron on the highlighted one, or `m` opens it. What it does
//! goes to the bridge, which tells WhatsApp: the phone and the other devices
//! follow.

use crate::app::{App, Focus};
use crate::attach::{MENU_BG, MENU_HOVER};
use crate::bridge::Chat;
use crate::ui::{box_corners, draw_deco, META_IN, TEXT};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

// Nerd Font (Material Design and Font Awesome) icons.
pub(crate) const ARCHIVE: &str = "\u{f187}";
pub(crate) const PINNED: &str = "\u{f0403}";
const UNPIN: &str = "\u{f0404}";
const UNREAD: &str = "\u{f0e0}";
const READ: &str = "\u{f2b6}";
pub(crate) const MUTED: &str = "\u{f009b}";
const BELL: &str = "\u{f009a}";
const HEART: &str = "\u{f02d5}";
const UNHEART: &str = "\u{f0759}";
const ERASE: &str = "\u{f056}";
const TRASH: &str = "\u{f1f8}";

/// WhatsApp lets no more than this many chats be pinned.
const MAX_PINNED: usize = 3;

#[derive(Clone, Copy, PartialEq)]
enum Act {
    Archive,
    Pin,
    Read,
    Mute,
    Favorite,
    Sep,
    Clear,
    Delete,
}

/// "Silenciar notificações" for: seconds, 0 for always.
const MUTES: [(i64, &str); 3] = [(8 * 3600, "8 horas"), (7 * 24 * 3600, "1 semana"), (0, "Sempre")];

pub struct Popup {
    pub jid: String,
    sel: Act,
    /// The mute submenu, open, with its highlighted row.
    sub: Option<usize>,
    /// Clear or delete was chosen once; choosing it again does it.
    confirm: bool,
    /// The cell it hangs under, and whether its right edge lines up there
    /// (the chevron, the keyboard) or its left (the pointer).
    at: Position,
    right: bool,
}

#[derive(Default)]
pub struct ChatMenu {
    pub popup: Option<Popup>,
    /// The chevron on the highlighted chat row, and its chat.
    pub chevron: Option<(Rect, String)>,
    // Where the last draw put things, for the mouse.
    rows: Vec<(Rect, Act)>,
    sub_rows: Vec<(Rect, usize)>,
    area: Rect,
    sub_area: Rect,
}

fn acts(c: &Chat) -> Vec<Act> {
    let mut v = vec![Act::Archive];
    // Archiving unpins: an archived chat has no pin to offer.
    if !c.archived {
        v.push(Act::Pin);
    }
    v.extend([Act::Read, Act::Mute, Act::Favorite, Act::Sep, Act::Clear, Act::Delete]);
    v
}

/// Its icon, label and what goes at the right, flipped by the chat's state.
fn label(a: Act, c: &Chat, confirm: bool) -> (&'static str, &'static str, &'static str) {
    match a {
        Act::Archive if c.archived => (ARCHIVE, "Desarquivar conversa", ""),
        Act::Archive => (ARCHIVE, "Arquivar conversa", ""),
        Act::Pin if c.pinned => (UNPIN, "Desafixar conversa", ""),
        Act::Pin => (PINNED, "Fixar conversa", ""),
        Act::Read if c.unread > 0 => (READ, "Marcar como lida", ""),
        Act::Read => (UNREAD, "Marcar como não lida", ""),
        Act::Mute if c.muted => (BELL, "Reativar notificações", ""),
        Act::Mute => (MUTED, "Silenciar notificações", "›"),
        Act::Favorite if c.favorite => (UNHEART, "Remover dos Favoritos", ""),
        Act::Favorite => (HEART, "Adicionar aos Favoritos", ""),
        Act::Clear if confirm => (ERASE, "Limpar? Enter confirma", ""),
        Act::Clear => (ERASE, "Limpar conversa", ""),
        Act::Delete if confirm => (TRASH, "Apagar? Enter confirma", ""),
        Act::Delete => (TRASH, "Apagar conversa", ""),
        Act::Sep => ("", "", ""),
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
    /// Opens the menu on a chat, under `at`; one popup at a time.
    pub fn open_chat_menu(&mut self, jid: String, at: Position, right: bool) {
        if self.chat(&jid).is_none() {
            return;
        }
        self.account.menu = None;
        self.attach.menu = None;
        self.emoji_panel.open = false;
        self.cursor = Some(format!("c:{jid}"));
        self.follow = true;
        self.chat_menu.popup = Some(Popup { jid, sel: Act::Archive, sub: None, confirm: false, at, right });
    }

    /// `m` on the sidebar: the menu of the highlighted chat, under its chevron.
    pub fn chat_menu_here(&mut self) {
        let Some(crate::app::Item::Chat(c)) = self.current() else { return };
        let key = format!("c:{}", c.jid);
        let at = match self.side_hits.iter().find(|(_, k)| *k == key) {
            Some((r, _)) => Position::new(r.right().saturating_sub(2), r.y + 1),
            None => Position::new(self.side_list.right().saturating_sub(2), self.side_list.y),
        };
        self.open_chat_menu(c.jid, at, true);
    }

    fn act(&mut self, jid: &str, action: &str, extra: serde_json::Value) {
        let mut cmd = json!({"c": "chat_action", "chat": jid, "action": action});
        if let (Some(o), serde_json::Value::Object(e)) = (cmd.as_object_mut(), extra) {
            o.extend(e);
        }
        self.cmd(cmd);
    }

    fn choose_act(&mut self, a: Act) {
        let Some(p) = self.chat_menu.popup.as_ref() else { return };
        let (jid, confirm) = (p.jid.clone(), p.confirm);
        let Some(c) = self.chat(&jid).cloned() else {
            self.chat_menu.popup = None;
            return;
        };
        let mut close = true;
        match a {
            Act::Sep => close = false,
            Act::Archive => self.act(&jid, if c.archived { "unarchive" } else { "archive" }, json!({})),
            Act::Pin if !c.pinned && self.chats.iter().filter(|x| x.pinned).count() >= MAX_PINNED => {
                self.note("Você só pode fixar até 3 conversas");
            }
            Act::Pin => self.act(&jid, if c.pinned { "unpin" } else { "pin" }, json!({})),
            Act::Read if c.unread > 0 => {
                if let Some(x) = self.chats.iter_mut().find(|x| x.jid == jid) {
                    x.unread = 0;
                }
                let quiet = !self.settings.on(crate::config::RECEIPTS);
                self.act(&jid, "read", json!({"quiet": quiet}));
            }
            Act::Read => {
                // Left open it would be read again at once: WhatsApp Web closes it too.
                if self.open.as_deref() == Some(jid.as_str()) {
                    self.close_chat();
                }
                self.act(&jid, "unread", json!({}));
            }
            Act::Mute if c.muted => self.act(&jid, "unmute", json!({})),
            Act::Mute => {
                close = false;
                if let Some(p) = self.chat_menu.popup.as_mut() {
                    (p.sel, p.sub) = (Act::Mute, Some(0));
                }
            }
            Act::Favorite => self.act(&jid, if c.favorite { "unfavorite" } else { "favorite" }, json!({})),
            Act::Clear | Act::Delete if !confirm => {
                close = false;
                if let Some(p) = self.chat_menu.popup.as_mut() {
                    (p.sel, p.confirm) = (a, true);
                }
            }
            Act::Clear => {
                if self.msgs.contains_key(&jid) {
                    self.msgs.insert(jid.clone(), Vec::new());
                }
                self.act(&jid, "clear", json!({}));
            }
            Act::Delete => {
                if self.open.as_deref() == Some(jid.as_str()) {
                    self.close_chat();
                }
                self.msgs.remove(&jid);
                self.chats.retain(|x| x.jid != jid);
                self.act(&jid, "delete", json!({}));
            }
        }
        if close {
            self.chat_menu.popup = None;
        }
    }

    fn choose_mute(&mut self, i: usize) {
        let Some(p) = self.chat_menu.popup.take() else { return };
        self.act(&p.jid, "mute", json!({"secs": MUTES[i].0}));
    }

    /// The menu takes every key while open. True when taken.
    pub fn chat_menu_key(&mut self, k: KeyEvent) -> bool {
        let Some(p) = self.chat_menu.popup.as_ref() else { return false };
        let items = self.chat(&p.jid).map(acts).unwrap_or_default();
        let Some(p) = self.chat_menu.popup.as_mut() else { return false };
        let go = matches!(k.code, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l' | ' '));
        match p.sub {
            Some(s) => match k.code {
                KeyCode::Up | KeyCode::Char('k') => p.sub = Some((s + MUTES.len() - 1) % MUTES.len()),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => p.sub = Some((s + 1) % MUTES.len()),
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => p.sub = None,
                _ if go => self.choose_mute(s),
                _ => {}
            },
            None => match k.code {
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => (p.sel, p.confirm) = (step(&items, p.sel, -1), false),
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => (p.sel, p.confirm) = (step(&items, p.sel, 1), false),
                KeyCode::Esc | KeyCode::Char('m' | 'q') => self.chat_menu.popup = None,
                _ if go => {
                    let sel = p.sel;
                    self.choose_act(sel);
                }
                _ => {}
            },
        }
        true
    }

    /// Hovering, clicks, a click away closes it (and does nothing else);
    /// a right-click on a chat or a click on the chevron opens it. True when
    /// the event was the menu's.
    pub fn chat_menu_mouse(&mut self, m: MouseEvent) -> bool {
        // Under the account's dialogs, the clicks are theirs.
        if self.account.settings.is_some() || self.account.about || self.account.profile.is_some() {
            return false;
        }
        let at = Position::new(m.column, m.row);
        let right_click = m.kind == MouseEventKind::Down(MouseButton::Right);
        // The chat a right-click is on.
        let chat_at = self
            .side_hits
            .iter()
            .find(|(r, _)| right_click && r.contains(at))
            .and_then(|(_, k)| k.strip_prefix("c:"))
            .map(String::from);
        let Some(p) = self.chat_menu.popup.as_ref() else {
            if let Some(jid) = chat_at {
                self.open_chat_menu(jid, Position::new(m.column, m.row + 1), false);
                return true;
            }
            let chevron = self.chat_menu.chevron.clone().filter(|(r, _)| r.contains(at));
            if let Some((r, jid)) = chevron.filter(|_| m.kind == MouseEventKind::Down(MouseButton::Left)) {
                self.open_chat_menu(jid, Position::new(r.right().saturating_sub(1), r.y + 1), true);
                return true;
            }
            return false;
        };
        let menu = &self.chat_menu;
        let hit = menu.rows.iter().find(|(r, _)| r.contains(at)).map(|(_, a)| *a);
        let sub_hit = menu.sub_rows.iter().find(|(r, _)| r.contains(at)).map(|(_, i)| *i);
        let inside = menu.area.contains(at) || menu.sub_area.contains(at);
        let (sel, sub) = (p.sel, p.sub);
        let muted = self.chat(&p.jid).is_some_and(|c| c.muted);
        match m.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                let Some(p) = self.chat_menu.popup.as_mut() else { return true };
                match (hit, sub_hit) {
                    (_, Some(i)) => p.sub = Some(i),
                    // The submenu opens under the pointer, as the account menu's does.
                    (Some(Act::Mute), _) if sub.is_none() && !muted => (p.sel, p.sub, p.confirm) = (Act::Mute, Some(0), false),
                    (Some(a), _) if a != sel && a != Act::Sep => (p.sel, p.sub, p.confirm) = (a, None, false),
                    _ => {}
                }
            }
            MouseEventKind::Down(MouseButton::Left) if sub_hit.is_some() => self.choose_mute(sub_hit.unwrap_or(0)),
            MouseEventKind::Down(MouseButton::Left) if hit.is_some() => {
                let a = hit.unwrap_or(Act::Sep);
                if let Some(p) = self.chat_menu.popup.as_mut() {
                    if p.sel != a {
                        (p.sel, p.confirm) = (a, false);
                    }
                }
                self.choose_act(a);
            }
            MouseEventKind::Down(_) if !inside => {
                self.chat_menu.popup = None;
                // A right-click on another chat opens its menu instead.
                if let Some(jid) = chat_at {
                    self.open_chat_menu(jid, Position::new(m.column, m.row + 1), false);
                }
            }
            _ => {}
        }
        true
    }

    /// Closes the open chat, as Esc on WhatsApp Web does.
    pub(crate) fn close_chat(&mut self) {
        // What was typed stays as its draft.
        self.switch_draft("");
        self.open = None;
        self.profile = None;
        self.focus = Focus::Sidebar;
    }
}

/// The menu, and beside it the mute submenu, in the clip menu's colors.
pub fn draw(f: &mut Frame, app: &mut App) {
    let menu = &mut app.chat_menu;
    menu.rows.clear();
    menu.sub_rows.clear();
    (menu.area, menu.sub_area) = Default::default();
    let Some(p) = menu.popup.as_ref() else { return };
    let (jid, sel, sub, confirm, at, right) = (p.jid.clone(), p.sel, p.sub, p.confirm, p.at, p.right);
    // Gone meanwhile (deleted on the phone): nothing to show it for.
    let Some(c) = app.chat(&jid).cloned() else {
        app.chat_menu.popup = None;
        return;
    };
    let items = acts(&c);
    let screen = f.area();
    let lw = items.iter().map(|a| label(*a, &c, false).1.width().max(label(*a, &c, true).1.width())).max().unwrap_or(0);
    let w = (lw as u16 + 10).min(screen.width);
    let h = (items.len() as u16 + 2).min(screen.height);
    let x = if right { (at.x + 1).saturating_sub(w) } else { at.x };
    let x = x.clamp(screen.x, screen.right().saturating_sub(w));
    // Under the row, or over it where there's no room below.
    let y = if at.y + h <= screen.bottom() { at.y } else { at.y.saturating_sub(h + 1).max(screen.y) };
    let area = Rect::new(x, y, w, h);
    let rows: Vec<(Act, &str, &str, &str)> = items
        .iter()
        .map(|a| {
            let (icon, text, more) = label(*a, &c, confirm && *a == sel);
            (*a, icon, text, more)
        })
        .collect();
    let mut sub_y = None;
    let lines: Vec<Line> = rows
        .iter()
        .map(|&(a, icon, text, more)| {
            let row = match (a == sel, a == Act::Mute && sub.is_some()) {
                (true, _) | (_, true) => Style::new().bg(MENU_HOVER),
                _ => Style::new().bg(MENU_BG),
            };
            if a == Act::Mute {
                sub_y = Some(area.y + 1 + items.iter().position(|x| *x == a).unwrap_or(0) as u16);
            }
            if a == Act::Sep {
                return separator(w);
            }
            let fg = if confirm && a == sel { Color::Yellow } else { TEXT };
            let pad = (w as usize).saturating_sub(8 + text.width() + more.width());
            Line::from(vec![
                Span::styled(" ", Style::new().bg(MENU_BG)),
                Span::styled("  ", row),
                Span::styled(icon, row.fg(META_IN)),
                Span::styled("  ", row),
                Span::styled(text, row.fg(fg)),
                Span::styled(" ".repeat(pad), row),
                Span::styled(more, row.fg(META_IN)),
                Span::styled(" ", row),
                Span::styled(" ", Style::new().bg(MENU_BG)),
            ])
        })
        .collect();
    for (n, (a, ..)) in rows.iter().enumerate() {
        if *a != Act::Sep {
            app.chat_menu.rows.push((Rect::new(x, y + 1 + n as u16, w, 1), *a));
        }
    }
    app.chat_menu.area = area;
    boxed(f, app, area, lines);
    if let (Some(s), Some(sy)) = (sub, sub_y) {
        draw_sub(f, app, area, sy, s);
    }
}

/// The mute choices beside the menu, at its row; on the left when the right has no room.
fn draw_sub(f: &mut Frame, app: &mut App, main: Rect, row_y: u16, sel: usize) {
    let screen = f.area();
    let w = MUTES.iter().map(|(_, l)| l.width()).max().unwrap_or(0) as u16 + 8;
    let h = MUTES.len() as u16 + 2;
    let x = if main.right() + w <= screen.right() { main.right() } else { main.x.saturating_sub(w) };
    let y = row_y.saturating_sub(1).min(screen.bottom().saturating_sub(h)).max(screen.y);
    let area = Rect::new(x, y, w, h).intersection(screen);
    let lines: Vec<Line> = MUTES
        .iter()
        .enumerate()
        .map(|(i, (_, text))| {
            let row = if i == sel { Style::new().bg(MENU_HOVER) } else { Style::new().bg(MENU_BG) };
            let pad = (w as usize).saturating_sub(4 + text.width());
            Line::from(vec![
                Span::styled(" ", Style::new().bg(MENU_BG)),
                Span::styled("  ", row),
                Span::styled(*text, row.fg(TEXT)),
                Span::styled(" ".repeat(pad), row),
                Span::styled(" ", Style::new().bg(MENU_BG)),
            ])
        })
        .collect();
    for i in 0..MUTES.len() {
        app.chat_menu.sub_rows.push((Rect::new(x, y + 1 + i as u16, w, 1), i));
    }
    app.chat_menu.sub_area = area;
    boxed(f, app, area, lines);
}

fn separator(w: u16) -> Line<'static> {
    let bg = Style::new().bg(MENU_BG);
    Line::from(vec![Span::styled("  ", bg), Span::styled("─".repeat((w as usize).saturating_sub(4)), bg.fg(Color::DarkGray)), Span::styled("  ", bg)])
}

/// `lines` in a box of the menu color, a blank row over and under them, its
/// corners rounded where the terminal shows pictures.
fn boxed(f: &mut Frame, app: &mut App, area: Rect, lines: Vec<Line<'static>>) {
    let w = area.width as usize;
    let rounded = app.picker.is_some();
    let bg = Style::new().bg(MENU_BG);
    // The corner cells stay bare: the pictures round them.
    let edge = if rounded { Style::new() } else { bg };
    let blank = Line::from(vec![Span::styled(" ", edge), Span::styled(" ".repeat(w.saturating_sub(2)), bg), Span::styled(" ", edge)]);
    let mut all = vec![blank.clone()];
    all.extend(lines);
    all.push(blank);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(all), area);
    if rounded && area.width > 1 && area.height > 1 {
        for (row, col, d) in box_corners(0, area.height as usize, 0, w, MENU_BG, 0) {
            draw_deco(f, app, d, area.x + col as u16, area.y + row as u16);
        }
    }
}
