//! Reactions: hovering a message shows its quick reactions by the bubble, as
//! Slack's, six emojis and a + for the emoji panel; the message menu's
//! "Reagir" opens them in a bar, as WhatsApp Web's. Your own reaction picked
//! again takes it back.
//! A click on the reactions under a bubble shows who reacted, as WhatsApp
//! Web's box. Also a click on a reply's quote, which goes to the message it quotes.

use crate::app::{App, Phase};
use crate::ui::{avatar_cells, box_corners, display_name, draw_avatar, draw_deco, truncate, Deco, CAP_L, CAP_R, DAY_PILL, META_IN, TEXT};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde_json::json;
use unicode_width::UnicodeWidthStr;

/// WhatsApp's quick reactions, then the +.
const QUICK: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];
/// Each choice's cells: the emoji, a cell each side.
const SLOT: u16 = 4;
/// Under the hovered choice, and under yours.
const HOVER: Color = Color::Rgb(0x3B, 0x3D, 0x3D);
pub const SMILEY: &str = "\u{f118}";

/// A bubble on screen, from the last draw: its visible part, and its
/// quote's, for the clicks.
pub struct Hit {
    pub rect: Rect,
    pub id: String,
    pub from_me: bool,
    pub quote: Option<Rect>,
    /// The reactions under it.
    pub pill: Option<Rect>,
}

/// Who reacted to a message: a tab for all and one per emoji (just the one
/// when there's one), then each person with their emoji, you first.
pub struct Who {
    chat: String,
    id: String,
    /// 0 for all, then the message's emojis in their order.
    tab: usize,
    /// The first person shown.
    top: usize,
    /// The person under the pointer.
    hover: Option<String>,
    // From the last draw.
    area: Rect,
    tabs: Vec<Rect>,
    rows: Vec<(Rect, Person)>,
}

#[derive(Clone)]
struct Person {
    jid: String,
    name: String,
    emoji: String,
    mine: bool,
}

/// The quick reactions by the hovered message, from the last draw.
pub struct Quick {
    pub id: String,
    area: Rect,
    items: Vec<Rect>,
    /// The choice under the pointer.
    sel: Option<usize>,
}

/// The bar of reactions over a message.
pub struct Bar {
    chat: String,
    id: String,
    sel: usize,
    /// What opened it, the smiley or where the right-click was: the bar
    /// goes over it.
    anchor: Rect,
    // From the last draw.
    area: Rect,
    items: Vec<Rect>,
}

impl App {
    pub(crate) fn open_react_bar(&mut self, id: String, anchor: Rect) {
        let Some(chat) = self.open.clone() else { return };
        self.react_bar = Some(Bar { chat, id, sel: 0, anchor, area: Rect::default(), items: Vec::new() });
    }

    /// Reacts to a message with `emoji`; the one you already gave takes it back.
    pub fn react(&mut self, chat: &str, id: &str, emoji: &str) {
        self.react_bar = None;
        if let Some(why) = self.chat(chat).map(|c| c.read_only.clone()).filter(|r| !r.is_empty()) {
            return self.note(why);
        }
        if self.phase != Phase::Ready {
            return self.note("Ainda conectando; tente de novo em instantes.");
        }
        let mine = self
            .msgs
            .get(chat)
            .and_then(|l| l.iter().find(|m| m.id == id))
            .and_then(|m| m.reactions.iter().find(|r| r.mine))
            .map(|r| r.emoji.clone());
        let text = if mine.as_deref() == Some(emoji) { "" } else { emoji };
        self.cmd(json!({"c": "react", "chat": chat, "id": id, "text": text}));
    }

    fn react_pick(&mut self, i: usize) {
        let Some(bar) = self.react_bar.take() else { return };
        self.quick_pick(bar.chat, bar.id, i);
    }

    /// The `i`th quick reaction, or the +: the emoji panel.
    fn quick_pick(&mut self, chat: String, id: String, i: usize) {
        match QUICK.get(i) {
            Some(e) => self.react(&chat, &id, e),
            None => self.emoji_panel_react((chat, id)),
        }
    }

    /// To the message a reply quotes, in the middle of the screen. Older
    /// replies didn't keep its id: the newest earlier message with its text.
    fn jump_to_quote(&mut self, id: &str) {
        let Some(list) = self.open.as_ref().and_then(|c| self.msgs.get(c)) else { return };
        let Some(m) = list.iter().find(|m| m.id == id) else { return };
        let by_id = (!m.quote_id.is_empty()).then(|| list.iter().find(|x| x.id == m.quote_id)).flatten();
        let target = by_id.or_else(|| {
            let q = m.quote.trim();
            list.iter().rev().filter(|x| x.ts <= m.ts && x.id != m.id).find(|x| !q.is_empty() && x.text.trim().starts_with(q))
        });
        match target.map(|t| t.id.clone()) {
            Some(t) => {
                self.flash = Some((t.clone(), std::time::Instant::now()));
                self.jump = Some(t);
            }
            None => self.note("A mensagem original não está carregada"),
        }
    }

    /// While who reacted is shown: ← → the tabs, ↑ ↓ scroll, Esc closes;
    /// any other key closes it and goes on.
    fn who_key(&mut self, k: KeyEvent) -> bool {
        let Some(who) = self.react_who.as_mut() else { return false };
        let n = self.msgs.get(&who.chat).and_then(|l| l.iter().find(|m| m.id == who.id)).map_or(0, |m| tabs_of(m.reactions.len()));
        match k.code {
            KeyCode::Left | KeyCode::BackTab if n > 0 => (who.tab, who.top) = ((who.tab + n - 1) % n, 0),
            KeyCode::Right | KeyCode::Tab if n > 0 => (who.tab, who.top) = ((who.tab + 1) % n, 0),
            KeyCode::Up => who.top = who.top.saturating_sub(1),
            KeyCode::Down => who.top += 1,
            KeyCode::Esc => self.react_who = None,
            _ => {
                self.react_who = None;
                return false;
            }
        }
        true
    }

    /// Clicks on its tabs and people (yours takes your reaction back; anyone
    /// else's opens their info), the wheel; a click away closes it.
    pub(crate) fn who_mouse(&mut self, m: MouseEvent) -> bool {
        let Some(who) = self.react_who.as_mut() else { return false };
        let at = Position::new(m.column, m.row);
        let inside = who.area.contains(at);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if inside => {
                if let Some(i) = who.tabs.iter().position(|r| r.contains(at)) {
                    (who.tab, who.top) = (i, 0);
                } else if let Some(p) = who.rows.iter().find(|(r, _)| r.contains(at)).map(|(_, p)| p.clone()) {
                    let (chat, id) = (who.chat.clone(), who.id.clone());
                    self.react_who = None;
                    if p.mine {
                        self.react(&chat, &id, &p.emoji);
                    } else {
                        self.names.entry(p.jid.clone()).or_insert(p.name);
                        self.show_profile(p.jid);
                    }
                }
            }
            MouseEventKind::Down(_) if !inside => self.react_who = None,
            MouseEventKind::Moved => {
                who.hover = who.rows.iter().find(|(r, _)| r.contains(at)).map(|(_, p)| p.jid.clone());
                return inside;
            }
            MouseEventKind::ScrollUp if inside => who.top = who.top.saturating_sub(1),
            MouseEventKind::ScrollDown if inside => who.top += 1,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.react_who = None;
                return false;
            }
            _ => return inside,
        }
        true
    }

    /// While the bar's open: ← → choose, Enter picks, Esc closes; any other
    /// key closes it and goes on.
    pub fn react_key(&mut self, k: KeyEvent) -> bool {
        if self.who_key(k) {
            return true;
        }
        let Some(bar) = self.react_bar.as_mut() else { return false };
        let n = QUICK.len() + 1;
        match k.code {
            KeyCode::Left | KeyCode::BackTab => bar.sel = (bar.sel + n - 1) % n,
            KeyCode::Right | KeyCode::Tab => bar.sel = (bar.sel + 1) % n,
            KeyCode::Enter => {
                let i = bar.sel;
                self.react_pick(i);
            }
            KeyCode::Esc => self.react_bar = None,
            _ => {
                self.react_bar = None;
                return false;
            }
        }
        true
    }

    /// The bar's clicks and hovering, the quick reactions', a click on a quote; and which message the pointer's over.
    pub fn react_mouse(&mut self, m: MouseEvent) -> bool {
        let at = Position::new(m.column, m.row);
        if let Some(bar) = self.react_bar.as_mut() {
            let hit = bar.items.iter().position(|r| r.contains(at));
            match m.kind {
                MouseEventKind::Moved => {
                    if let Some(i) = hit {
                        bar.sel = i;
                    }
                    return bar.area.contains(at);
                }
                MouseEventKind::Down(MouseButton::Left) if hit.is_some() => self.react_pick(hit.unwrap_or(0)),
                // A click away closes it, and does nothing else.
                MouseEventKind::Down(_) => self.react_bar = None,
                // The wheel moves the message away: it closes, and scrolls.
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown | MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                    self.react_bar = None;
                    return false;
                }
                // The release of the click that opened it, a drag.
                _ => return bar.area.contains(at),
            }
            return true;
        }
        let on_row = |h: &&Hit| self.msgs_area.contains(at) && at.y >= h.rect.y && at.y < h.rect.bottom();
        // Over the quick reactions, their message stays hovered, also on the row over it.
        let on_quick = self.react_btn.as_ref().filter(|q| q.area.contains(at)).map(|q| (q.id.clone(), q.items.iter().position(|r| r.contains(at))));
        match m.kind {
            MouseEventKind::Moved => {
                if let Some(q) = self.react_btn.as_mut() {
                    q.sel = on_quick.as_ref().and_then(|(_, i)| *i);
                }
                if on_quick.is_none() {
                    self.hover_msg = self.bubbles.iter().find(on_row).map(|h| h.id.clone());
                }
                on_quick.is_some()
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((id, i)) = on_quick {
                    if let (Some(i), Some(chat)) = (i, self.open.clone()) {
                        self.quick_pick(chat, id, i);
                    }
                    return true;
                }
                let pill = self.bubbles.iter().find_map(|h| h.pill.filter(|p| p.contains(at)).map(|p| (h.id.clone(), p)));
                if let (Some((id, _)), Some(chat)) = (pill, self.open.clone()) {
                    self.react_who = Some(Who { chat, id, tab: 0, top: 0, hover: None, area: Rect::default(), tabs: Vec::new(), rows: Vec::new() });
                    return true;
                }
                let quoted = self.bubbles.iter().find(|h| h.quote.is_some_and(|q| q.contains(at))).map(|h| h.id.clone());
                if let Some(id) = quoted {
                    self.jump_to_quote(&id);
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}

/// The quick reactions by the hovered message, as Slack's: on its top row,
/// outside the bubble when there's room (to the right of others', to the
/// left of yours), else on the row over it (under it at the top of the
/// screen); the choice under the pointer and yours lit.
pub fn draw_quick(f: &mut Frame, app: &mut App) {
    let prev = app.react_btn.take();
    if app.msg_menu.select.is_some() || app.react_bar.is_some() || app.react_who.is_some() {
        return;
    }
    let Some(id) = app.hover_msg.clone() else { return };
    let Some(h) = app.bubbles.iter().find(|h| h.id == id) else { return };
    let sel = prev.filter(|q| q.id == id).and_then(|q| q.sel);
    let area = app.msgs_area;
    // The caps, a space each item and after the last; each item 2 cells.
    let n = QUICK.len() as u16 + 1;
    let w = n * 3 + 3;
    if area.width < w {
        return;
    }
    let beside = if h.from_me {
        h.rect.x.checked_sub(w + 1).filter(|&x| x >= area.x)
    } else {
        Some(h.rect.right() + 1).filter(|&x| x + w <= area.right())
    };
    let (x, y) = match beside {
        Some(x) => (x, h.rect.y),
        None => {
            let y = if h.rect.y > area.y {
                h.rect.y - 1
            } else if h.rect.bottom() < area.bottom() {
                h.rect.bottom()
            } else {
                return;
            };
            (h.rect.right().saturating_sub(w).max(h.rect.x).min(area.right() - w).max(area.x), y)
        }
    };
    let mine = app
        .open
        .as_ref()
        .and_then(|c| app.msgs.get(c))
        .and_then(|l| l.iter().find(|m| m.id == id))
        .and_then(|m| m.reactions.iter().find(|r| r.mine))
        .map(|r| r.emoji.clone());

    let bg = Style::new().bg(DAY_PILL);
    let mut spans = vec![Span::styled(CAP_L, Style::new().fg(DAY_PILL))];
    let mut items = Vec::new();
    for i in 0..n as usize {
        let (glyph, fg) = match QUICK.get(i) {
            Some(e) => (*e, TEXT),
            None => ("+ ", META_IN),
        };
        let lit = sel == Some(i) || QUICK.get(i).is_some_and(|e| mine.as_deref() == Some(*e));
        let style = if lit { Style::new().fg(fg).bg(HOVER) } else { bg.fg(fg) };
        spans.extend([Span::styled(" ", bg), Span::styled(glyph, style)]);
        items.push(Rect::new(x + 1 + i as u16 * 3, y, 3, 1));
    }
    spans.extend([Span::styled(" ", bg), Span::styled(CAP_R, Style::new().fg(DAY_PILL))]);
    let r = Rect::new(x, y, w, 1);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(Line::from(spans)), r);
    app.react_btn = Some(Quick { id, area: r, items, sel });
}

/// The bar, over the message (under it at the top of the screen): a
/// rounded box, as WhatsApp Web's, the choice under the pointer and yours
/// on a circle.
pub fn draw(f: &mut Frame, app: &mut App) {
    let Some(bar) = app.react_bar.as_ref() else { return };
    if app.open.as_ref() != Some(&bar.chat) {
        app.react_bar = None;
        return;
    }
    // Its message gone from the screen, it waits.
    if !app.bubbles.iter().any(|h| h.id == bar.id) {
        return;
    }
    let anchor = bar.anchor;
    let mine = app
        .msgs
        .get(&bar.chat)
        .and_then(|l| l.iter().find(|m| m.id == bar.id))
        .and_then(|m| m.reactions.iter().find(|r| r.mine))
        .map(|r| r.emoji.clone());
    let sel = bar.sel;
    let area = app.msgs_area;
    let rounded = app.picker.is_some();
    let n = QUICK.len() as u16 + 1;
    let w = n * SLOT + 2;
    if area.width < w + 2 {
        return;
    }
    // Over what opened it, centered on it, as WhatsApp Web's; under it
    // at the top of the screen. A row each side for its rounded halves.
    let y = if anchor.y >= area.y + 3 {
        anchor.y - 2
    } else if anchor.bottom() + 2 < area.bottom() {
        anchor.bottom() + 1
    } else {
        anchor.y
    };
    let mid = anchor.x + anchor.width / 2;
    let x = mid.saturating_sub(w / 2).clamp(area.x, area.right() - w);
    let bar_area = Rect::new(x, y, w, 1);

    let bg = Style::new().bg(DAY_PILL);
    let circle = Style::new().fg(HOVER).bg(DAY_PILL);
    let mut spans = vec![Span::styled(" ", bg)];
    let mut items = Vec::new();
    for i in 0..n as usize {
        let (glyph, pad) = match QUICK.get(i) {
            Some(e) => (*e, ""),
            None => ("+", " "),
        };
        let lit = i == sel || QUICK.get(i).is_some_and(|e| mine.as_deref() == Some(*e));
        let fg = if i == QUICK.len() { META_IN } else { TEXT };
        if lit {
            spans.extend([
                Span::styled(CAP_L, circle),
                Span::styled(glyph, Style::new().fg(fg).bg(HOVER)),
                Span::styled(pad, Style::new().bg(HOVER)),
                Span::styled(CAP_R, circle),
            ]);
        } else {
            spans.extend([Span::styled(" ", bg), Span::styled(glyph, bg.fg(fg)), Span::styled(pad, bg), Span::styled(" ", bg)]);
        }
        items.push(Rect::new(x + 1 + i as u16 * SLOT, y, SLOT, 1));
    }
    spans.push(Span::styled(" ", bg));

    let mut hit_area = bar_area;
    if rounded && y > area.y && y + 1 < area.bottom() {
        // Half a row over it and half under: the text under those rows
        // mustn't show through around the halves.
        for row in [y - 1, y + 1] {
            crate::ui::blank(f.buffer_mut(), Rect::new(x, row, w, 1));
        }
        draw_deco(f, app, Deco::Edge(false, w, DAY_PILL), x, y - 1);
        draw_deco(f, app, Deco::Edge(true, w, DAY_PILL), x, y + 1);
        hit_area = Rect::new(x, y - 1, w, 3);
    } else if !rounded {
        spans.insert(0, Span::styled(CAP_L, Style::new().fg(DAY_PILL)));
        spans.push(Span::styled(CAP_R, Style::new().fg(DAY_PILL)));
    }
    let line_area = if rounded { bar_area } else { Rect::new(x.saturating_sub(1), y, w + 2, 1).intersection(area) };
    f.render_widget(Clear, line_area);
    f.render_widget(Paragraph::new(Line::from(spans)), line_area);
    if let Some(bar) = app.react_bar.as_mut() {
        bar.area = hit_area;
        bar.items = items;
    }
}

/// The tabs for `n` emojis: all and each, or just the one.
fn tabs_of(n: usize) -> usize {
    if n > 1 {
        n + 1
    } else {
        n
    }
}

/// Who reacted, as WhatsApp Web's box over the reactions (under them when
/// there's no room): how many, the tabs, then each person by their picture,
/// their emoji at the right; yours says a click takes it back.
pub fn draw_who(f: &mut Frame, app: &mut App) {
    let Some((chat, id, tab, top, hover)) = app.react_who.as_ref().map(|w| (w.chat.clone(), w.id.clone(), w.tab, w.top, w.hover.clone())) else { return };
    let msg = app.msgs.get(&chat).and_then(|l| l.iter().find(|m| m.id == id));
    let reactions = match msg {
        Some(m) if app.open.as_ref() == Some(&chat) && !m.reactions.is_empty() => m.reactions.clone(),
        _ => {
            app.react_who = None;
            return;
        }
    };
    // Its pill gone from the screen, it waits.
    let Some(anchor) = app.bubbles.iter().find(|h| h.id == id).and_then(|h| h.pill) else { return };
    let tab = tab.min(tabs_of(reactions.len()) - 1);
    let me = app.my_jid().unwrap_or_default();
    let mut people: Vec<Person> = reactions
        .iter()
        .enumerate()
        .filter(|(i, _)| tab == 0 || tab == i + 1)
        .flat_map(|(_, r)| r.by.iter().map(|p| Person { jid: p.jid.clone(), name: p.name.clone(), emoji: r.emoji.clone(), mine: p.jid == me }))
        .collect();
    people.sort_by_key(|p| !p.mine);
    let total: u32 = reactions.iter().map(|r| r.count).sum();

    let area = app.msgs_area;
    let w = 44.min(area.width.saturating_sub(2));
    // The header's seven rows and the box's foot; three rows a person.
    let want = 8 + (people.len() * 3).max(1) as u16;
    let h = want.min(8 + 3 * 6).min(area.height);
    if w < 24 || h < 11 {
        return;
    }
    let shown = ((h - 8) / 3) as usize;
    let top = top.min(people.len().saturating_sub(shown));
    let y = if anchor.y >= area.y + h {
        anchor.y - h
    } else if anchor.bottom() + h <= area.bottom() {
        anchor.bottom()
    } else {
        area.y
    };
    let x = anchor.x.min(area.right() - w).max(area.x);
    let rect = Rect::new(x, y, w, h);

    let rounded = app.picker.is_some();
    let bg = Style::new().bg(DAY_PILL);
    let edge = if rounded { Style::new() } else { bg };
    let iw = w as usize;
    let fill = |spans: &mut Vec<Span<'static>>| {
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        spans.push(Span::styled(" ".repeat(iw.saturating_sub(used)), bg));
    };
    let blank = |edge: Style| Line::from(vec![Span::styled(" ", edge), Span::styled(" ".repeat(iw - 2), bg), Span::styled(" ", edge)]);
    let meta = Style::new().fg(META_IN).bg(DAY_PILL);
    let mut lines = vec![blank(edge)];
    let mut head = vec![Span::styled(format!("  {total} {}", if total == 1 { "reação" } else { "reações" }), meta)];
    fill(&mut head);
    lines.push(Line::from(head));
    lines.push(blank(bg));

    // The tabs, the one shown on a pill.
    let mut labels: Vec<String> = Vec::new();
    if reactions.len() > 1 {
        labels.push(format!("Todas {total}"));
    }
    labels.extend(reactions.iter().map(|r| format!("{} {}", r.emoji, r.count)));
    let mut spans = vec![Span::styled(" ", bg)];
    let mut tabs = Vec::new();
    let mut col = 1u16;
    for (i, l) in labels.iter().enumerate() {
        let tw = l.width() as u16 + 4;
        if col + tw + 1 > w {
            break;
        }
        if i == tab {
            let cap = Style::new().fg(HOVER).bg(DAY_PILL);
            spans.extend([Span::styled(CAP_L, cap), Span::styled(format!(" {l} "), Style::new().fg(TEXT).bg(HOVER)), Span::styled(CAP_R, cap)]);
        } else {
            spans.push(Span::styled(format!("  {l}  "), meta));
        }
        tabs.push(Rect::new(x + col, y + 3, tw, 1));
        col += tw;
    }
    fill(&mut spans);
    lines.push(Line::from(spans));
    lines.push(blank(bg));
    lines.push(Line::from(Span::styled("─".repeat(iw), Style::new().fg(META_IN).bg(DAY_PILL))));
    lines.push(blank(bg));

    // The people, three rows each: the name on the middle one, by the
    // picture centered in all three, as WhatsApp Web centers them.
    let avatar = avatar_cells(app).map(|(w, _)| w);
    let lead = avatar.map_or(2, |a| a as usize + 4);
    let mut rows = Vec::new();
    let mut faces = Vec::new();
    for (k, p) in people.iter().skip(top).take(shown).enumerate() {
        let row_y = y + 7 + k as u16 * 3;
        // Under the pointer, lit as WhatsApp Web's: a click opens their info.
        let bg = if hover.as_ref() == Some(&p.jid) { Style::new().bg(HOVER) } else { bg };
        let meta = meta.patch(bg);
        let fill = |spans: &mut Vec<Span<'static>>| {
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            spans.push(Span::styled(" ".repeat(iw.saturating_sub(used)), bg));
        };
        let name = if p.mine { "Você".to_string() } else { display_name(&p.name, &p.jid) };
        let room = iw.saturating_sub(lead + p.emoji.width() + 4);
        let name = truncate(&name, room);
        let gap = iw.saturating_sub(lead + name.width() + p.emoji.width() + 3);
        let mut air = Vec::new();
        fill(&mut air);
        // Yours has a line under the name: the two by a two-row picture, the air below.
        if !p.mine {
            lines.push(Line::from(air.clone()));
        }
        lines.push(Line::from(vec![
            Span::styled(" ".repeat(lead), bg),
            Span::styled(name, bg.fg(TEXT)),
            Span::styled(" ".repeat(gap), bg),
            Span::styled(p.emoji.clone(), bg),
            Span::styled("   ", bg),
        ]));
        if p.mine {
            let mut under = vec![Span::styled(" ".repeat(lead), bg), Span::styled("Clique para remover", meta)];
            fill(&mut under);
            lines.push(Line::from(under));
        }
        lines.push(Line::from(air));
        rows.push((Rect::new(x, row_y, w, 3), p.clone()));
        if let Some(a) = avatar {
            faces.push((p.jid.clone(), Rect::new(x + 2, row_y, a, if p.mine { 2 } else { 3 })));
        }
    }
    while lines.len() < h as usize - 1 {
        lines.push(blank(bg));
    }
    lines.push(blank(edge));

    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines), rect);
    if rounded {
        for (row, col, d) in box_corners(0, h as usize, 0, iw, DAY_PILL, 0) {
            draw_deco(f, app, d, x + col as u16, y + row as u16);
        }
    }
    for (jid, r) in faces {
        draw_avatar(f, app, &jid, false, r);
    }
    if let Some(who) = app.react_who.as_mut() {
        who.tab = tab;
        who.top = top;
        who.area = rect;
        who.tabs = tabs;
        who.rows = rows;
    }
}
