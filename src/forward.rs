//! "Encaminhar mensagens para": the messages picked in the open chat go to
//! the chats checked here, as in WhatsApp Web's dialog: a search over the
//! recent chats, yours first, a checkbox by each, and the button that sends
//! to all of them.

use crate::app::{App, Phase};
use crate::attach::{MENU_BG, MENU_HOVER};
use crate::bridge::Chat;
use crate::chatmenu::boxed;
use crate::msgmenu::{BOX_OFF, BOX_ON};
use crate::ui::{avatar_cells, dim, display_name, draw_avatar, green, phone, truncate, CAP_L, CAP_R, FIELD, META_IN, SEARCH, TEXT};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;
use serde_json::json;
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

// Nerd Font icons.
const CLOSE: &str = "\u{f00d}";
const SEND: &str = "\u{f1d8}";
/// WhatsApp's forward arrow: the menu's, the selection's button.
pub const ICON: &str = "\u{f064}";
/// Over a forwarded message's text.
pub const LABEL: &str = "↪";

/// From this forwarding score on, WhatsApp says it was forwarded often.
pub const OFTEN: u32 = 5;

/// Each chat's rows: its name, the line under it, a row of air.
const ROW_H: u16 = 3;

/// A chat the messages can go to.
struct Target {
    jid: String,
    name: String,
    sub: String,
    group: bool,
}

#[derive(Default)]
pub struct Picker {
    query: String,
    /// The chats checked, in the order they were.
    checked: Vec<String>,
    /// The highlighted chat, Enter's.
    cur: Option<String>,
    /// The first chat shown, kept with the highlight unless the wheel moved it.
    offset: usize,
    follow: bool,
}

#[derive(Default)]
pub struct Forward {
    pub picker: Option<Picker>,
    /// Under each chat's name: a group's members, a contact's recado. Asked
    /// of the bridge once.
    pub subs: HashMap<String, String>,
    asked: bool,
    // Where the last draw put things, for the mouse.
    area: Rect,
    list: Rect,
    close_btn: Rect,
    send_btn: Rect,
    rows: Vec<(Rect, String)>,
}

fn toggle(p: &mut Picker, jid: String) {
    match p.checked.iter().position(|j| *j == jid) {
        Some(i) => {
            p.checked.remove(i);
        }
        None => p.checked.push(jid.clone()),
    }
    p.cur = Some(jid);
}

impl App {
    /// The selection's forward button: the dialog, over it.
    pub(crate) fn open_forward(&mut self) {
        if !self.msg_menu.select.as_ref().is_some_and(|s| s.forward && !s.ids.is_empty()) {
            return;
        }
        if !self.msg_menu.fwd.asked {
            self.msg_menu.fwd.asked = true;
            self.cmd(json!({"c": "chat_subs"}));
        }
        self.msg_menu.fwd.picker = Some(Picker { follow: true, ..Default::default() });
    }

    /// The chats it can go to, the latest first and yours on top, as far as
    /// the search matches; the ones nobody can write to left out.
    fn targets(&self, query: &str) -> Vec<Target> {
        let mine = (!self.me.is_empty()).then(|| format!("{}@s.whatsapp.net", self.me));
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
        let digits: String = query.chars().filter(char::is_ascii_digit).collect();
        let mut chats: Vec<&Chat> = self
            .chats
            .iter()
            .filter(|c| c.read_only.is_empty() && c.jid != "0@s.whatsapp.net" && !c.jid.ends_with("@broadcast") && !c.jid.ends_with("@newsletter"))
            .collect();
        chats.sort_by_key(|c| (mine.as_ref() != Some(&c.jid), std::cmp::Reverse(c.last_ts)));
        chats
            .into_iter()
            .map(|c| {
                let you = mine.as_ref() == Some(&c.jid);
                let name = if you { format!("{} (você)", phone(&c.jid)) } else { display_name(&c.name, &c.jid) };
                let sub = match self.msg_menu.fwd.subs.get(&c.jid).filter(|s| !s.is_empty()) {
                    _ if you => "Mensagens para mim".to_string(),
                    Some(s) => s.replace('\n', " "),
                    None if !c.group && name != phone(&c.jid) => phone(&c.jid),
                    None => String::new(),
                };
                Target { jid: c.jid.clone(), name, sub, group: c.group }
            })
            .filter(|t| {
                let n = t.name.to_lowercase();
                words.iter().all(|w| n.contains(w.as_str())) || (digits.len() >= 3 && t.jid.split('@').next().is_some_and(|u| u.contains(&digits)))
            })
            .collect()
    }

    /// The send button: the picked messages, in the conversation's order,
    /// to every chat checked.
    fn send_forward(&mut self) {
        let Some(p) = self.msg_menu.fwd.picker.as_ref().filter(|p| !p.checked.is_empty()) else { return };
        if self.phase != Phase::Ready {
            return self.note("Ainda conectando; tente de novo em instantes.");
        }
        let to = p.checked.clone();
        self.msg_menu.fwd.picker = None;
        let Some(s) = self.msg_menu.select.take() else { return };
        let ids: Vec<String> = self.msgs.get(&s.chat).map(|l| l.iter().filter(|m| s.ids.contains(&m.id)).map(|m| m.id.clone()).collect()).unwrap_or_default();
        self.cmd(json!({"c": "forward", "chat": s.chat, "ids": ids, "to": to}));
    }

    /// The dialog's keys: typing searches, ↑↓ move, Enter checks the chat,
    /// Alt+Enter (or Ctrl+Enter) sends, Esc clears the search, then closes.
    pub(crate) fn forward_key(&mut self, k: KeyEvent) -> bool {
        let Some(p) = self.msg_menu.fwd.picker.as_ref() else { return false };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if k.code == KeyCode::Enter && (ctrl || alt) {
            self.send_forward();
            return true;
        }
        if k.code == KeyCode::Esc && p.query.is_empty() {
            self.msg_menu.fwd.picker = None;
            return true;
        }
        let list: Vec<String> = self.targets(&p.query).into_iter().map(|t| t.jid).collect();
        let Some(p) = self.msg_menu.fwd.picker.as_mut() else { return false };
        let at = p.cur.as_ref().and_then(|j| list.iter().position(|x| x == j));
        let mut searched = false;
        match k.code {
            KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                let down = matches!(k.code, KeyCode::Down | KeyCode::Tab);
                let i = match at {
                    Some(i) if down => (i + 1).min(list.len().saturating_sub(1)),
                    Some(i) => i.saturating_sub(1),
                    None => 0,
                };
                p.cur = list.get(i).cloned();
                p.follow = true;
            }
            // Found by the search, checked: the search clears for the next.
            KeyCode::Enter => {
                if let Some(j) = at.or((!list.is_empty()).then_some(0)).map(|i| list[i].clone()) {
                    toggle(p, j);
                    p.query.clear();
                    p.follow = true;
                }
            }
            KeyCode::Esc => {
                p.query.clear();
                searched = true;
            }
            KeyCode::Backspace => {
                p.query.pop();
                searched = true;
            }
            KeyCode::Char('u') if ctrl => {
                p.query.clear();
                searched = true;
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                p.query.push(c);
                searched = true;
            }
            _ => {}
        }
        if searched {
            (p.cur, p.offset, p.follow) = (None, 0, true);
        }
        true
    }

    /// The dialog's clicks, the wheel over its list. While it's open every
    /// event is its: a click outside closes it.
    pub(crate) fn forward_mouse(&mut self, m: MouseEvent) -> bool {
        if self.msg_menu.fwd.picker.is_none() {
            return false;
        }
        let at = Position::new(m.column, m.row);
        let fwd = &self.msg_menu.fwd;
        let hit = fwd.rows.iter().find(|(r, _)| r.contains(at)).map(|(_, j)| j.clone());
        let (inside, in_list) = (fwd.area.contains(at), fwd.list.contains(at));
        let (close, send) = (fwd.close_btn.contains(at), fwd.send_btn.contains(at));
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        match m.kind {
            _ if left && close => self.msg_menu.fwd.picker = None,
            _ if left && send => self.send_forward(),
            MouseEventKind::Down(_) if !inside => self.msg_menu.fwd.picker = None,
            _ => {
                let Some(p) = self.msg_menu.fwd.picker.as_mut() else { return true };
                match m.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        if let Some(j) = hit {
                            toggle(p, j);
                        }
                    }
                    MouseEventKind::Moved => {
                        if hit.is_some() {
                            p.cur = hit;
                        }
                    }
                    MouseEventKind::ScrollUp if in_list => (p.offset, p.follow) = (p.offset.saturating_sub(1), false),
                    MouseEventKind::ScrollDown if in_list => (p.offset, p.follow) = (p.offset + 1, false),
                    _ => {}
                }
            }
        }
        true
    }
}

/// A row of the dialog, its color to the edge.
fn row(mut spans: Vec<Span<'static>>, w: u16, bg: Color) -> Line<'static> {
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    spans.push(Span::raw(" ".repeat((w as usize).saturating_sub(used))));
    Line::from(spans).style(Style::new().bg(bg))
}

/// The dialog, in the middle of the screen, as tall as it lets it.
pub fn draw(f: &mut Frame, app: &mut App) {
    let fwd = &mut app.msg_menu.fwd;
    fwd.rows.clear();
    fwd.area = Rect::default();
    fwd.list = Rect::default();
    fwd.close_btn = Rect::default();
    fwd.send_btn = Rect::default();
    // The selection gone (another chat opened), the dialog goes with it.
    if !app.msg_menu.select.as_ref().is_some_and(|s| s.forward) {
        app.msg_menu.fwd.picker = None;
    }
    let Some(p) = app.msg_menu.fwd.picker.as_ref() else { return };
    let (query, checked, cur, follow) = (p.query.clone(), p.checked.clone(), p.cur.clone(), p.follow);
    let targets = app.targets(&query);
    let screen = f.area();
    let w = 60u16.min(screen.width.saturating_sub(4)).max(30u16.min(screen.width));
    let h = 44u16.min(screen.height.saturating_sub(4)).max(14u16.min(screen.height));
    let rect = Rect::new(screen.x + (screen.width - w) / 2, screen.y + (screen.height - h) / 2, w, h);
    // Inside the blank rows boxed adds over and under.
    let n = h.saturating_sub(2);
    let y0 = rect.y + 1;
    let base = Style::new().bg(MENU_BG);
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(row(
        vec![Span::raw("  "), Span::styled(CLOSE, base.fg(META_IN)), Span::raw("   "), Span::styled("Encaminhar mensagens para", base.fg(TEXT).add_modifier(Modifier::BOLD))],
        w,
        MENU_BG,
    ));
    lines.push(row(Vec::new(), w, MENU_BG));

    // The search, a pill across, always with the keys.
    let field = Style::new().bg(FIELD);
    let room = (w as usize).saturating_sub(4 + 2 + 4 + 1);
    let (text, fg) = if query.is_empty() { ("Pesquisar nome ou número".to_string(), META_IN) } else { (query.clone(), TEXT) };
    let text = truncate(&text, room);
    let pad = room.saturating_sub(text.width()) + 1;
    lines.push(row(
        vec![
            Span::raw("  "),
            Span::styled(CAP_L, base.fg(FIELD)),
            Span::styled(format!(" {SEARCH}  "), field.fg(green())),
            Span::styled(text, field.fg(fg)),
            Span::styled(" ".repeat(pad), field),
            Span::styled(CAP_R, base.fg(FIELD)),
        ],
        w,
        MENU_BG,
    ));
    let caret = rect.x + 3 + 4 + query.width().min(room) as u16;
    lines.push(row(Vec::new(), w, MENU_BG));
    let header = if targets.is_empty() { "Nenhuma conversa encontrada" } else { "Conversas recentes" };
    lines.push(row(vec![Span::styled(format!("  {header}"), base.fg(META_IN))], w, MENU_BG));
    lines.push(row(Vec::new(), w, MENU_BG));

    // The chats, scrolled to keep the highlighted one in view.
    let foot = if checked.is_empty() { 1 } else { 3 };
    let list_h = n.saturating_sub(lines.len() as u16 + foot);
    let fit = (list_h / ROW_H).max(1) as usize;
    let list_y = y0 + lines.len() as u16;
    let ci = cur.as_ref().and_then(|j| targets.iter().position(|t| t.jid == *j));
    let mut offset = app.msg_menu.fwd.picker.as_ref().map_or(0, |p| p.offset);
    if let (true, Some(ci)) = (follow, ci) {
        offset = offset.min(ci);
        while ci >= offset + fit {
            offset += 1;
        }
    }
    offset = offset.min(targets.len().saturating_sub(fit));
    if let Some(p) = app.msg_menu.fwd.picker.as_mut() {
        p.offset = offset;
    }
    let cells = avatar_cells(app).filter(|_| app.settings.on(crate::config::PHOTOS));
    // The text's column: after the checkbox, and the picture when there are.
    let lead = cells.map_or(6, |(aw, _)| 7 + aw as usize);
    let text_w = (w as usize).saturating_sub(lead + 2);
    let mut pictures = Vec::new();
    for (i, t) in targets.iter().enumerate().skip(offset).take(fit) {
        let y = list_y + ((i - offset) as u16) * ROW_H;
        let bg = if cur.as_ref() == Some(&t.jid) { MENU_HOVER } else { MENU_BG };
        let on = Style::new().bg(bg);
        let (glyph, gfg) = if checked.contains(&t.jid) { (BOX_ON, green()) } else { (BOX_OFF, META_IN) };
        let gap = " ".repeat(lead - 3);
        lines.push(row(
            vec![Span::raw("  "), Span::styled(glyph, on.fg(gfg)), Span::raw(gap.clone()), Span::styled(truncate(&t.name, text_w), on.fg(TEXT))],
            w,
            bg,
        ));
        lines.push(row(vec![Span::raw("   "), Span::raw(gap), Span::styled(truncate(&t.sub, text_w), on.fg(META_IN))], w, bg));
        lines.push(row(Vec::new(), w, bg));
        app.msg_menu.fwd.rows.push((Rect::new(rect.x, y, w, ROW_H), t.jid.clone()));
        if let Some((aw, _)) = cells {
            pictures.push((Rect::new(rect.x + 5, y, aw, 2), t.jid.clone(), t.group));
        }
    }
    while lines.len() < n.saturating_sub(foot) as usize {
        lines.push(row(Vec::new(), w, MENU_BG));
    }

    // Who it goes to, and the send button.
    if !checked.is_empty() {
        let all = app.targets("");
        let mine = format!("{}@s.whatsapp.net", app.me);
        let names: Vec<String> = checked
            .iter()
            .map(|j| all.iter().find(|t| t.jid == *j).map_or_else(|| phone(j), |t| if t.jid == mine { "Você".to_string() } else { t.name.clone() }))
            .collect();
        let bar = Style::new().bg(MENU_HOVER);
        let btn_w = 2 + SEND.width() + 2;
        let names = truncate(&names.join(", "), (w as usize).saturating_sub(btn_w + 6));
        let pad = (w as usize).saturating_sub(2 + names.width() + btn_w + 2);
        let send = Style::new().fg(Color::Rgb(0x11, 0x1B, 0x21)).bg(green());
        lines.push(row(Vec::new(), w, MENU_BG));
        lines.push(row(
            vec![
                Span::styled("  ", bar),
                Span::styled(names, bar.fg(META_IN)),
                Span::styled(" ".repeat(pad), bar),
                Span::styled(CAP_L, bar.fg(green())),
                Span::styled(format!(" {SEND} "), send),
                Span::styled(CAP_R, bar.fg(green())),
                Span::styled("  ", bar),
            ],
            w,
            MENU_HOVER,
        ));
        let sy = y0 + lines.len() as u16 - 1;
        app.msg_menu.fwd.send_btn = Rect::new(rect.right().saturating_sub(btn_w as u16 + 2), sy, btn_w as u16, 1);
    }
    let hint = truncate("  ↑↓ move · Enter marca · Alt+Enter envia · Esc fecha", w as usize);
    lines.push(row(vec![Span::styled(hint, dim().bg(MENU_BG))], w, MENU_BG));
    lines.truncate(n as usize);

    let fwd = &mut app.msg_menu.fwd;
    fwd.area = rect;
    fwd.list = Rect::new(rect.x, list_y, w, list_h);
    fwd.close_btn = Rect::new(rect.x + 1, y0, 3, 1);
    boxed(f, app, rect, lines);
    for (r, jid, group) in pictures {
        if r.bottom() <= rect.bottom() {
            draw_avatar(f, app, &jid, group, r);
        }
    }
    f.set_cursor_position(Position::new(caret.min(rect.right().saturating_sub(4)), y0 + 2));
}
