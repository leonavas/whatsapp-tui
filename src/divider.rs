//! The column between the chat list and the conversation: dragged with the
//! mouse it resizes the list, as slick's border does. Far enough to the
//! left the list keeps only the pictures, its own narrow width; dragged
//! back past there, the whole list again.

use crate::app::{App, Focus, Item};
use crate::config;
use crate::ui::{avatar_cells, dim, display_name, draw_avatar, draw_deco, green, name_color, phone, truncate, Deco, CAP_L, CAP_R, FIELD, META_IN, SEARCH, SELECTED, TEXT};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

/// The narrowest the whole list goes: a name and its time still read.
pub const SIDE_MIN: u16 = 24;
/// What the conversation keeps, however wide the list is dragged.
const CONV_MIN: u16 = 48;

pub struct Divider {
    /// The list's width as last dragged; None until then: its share of the screen.
    pub width: Option<u16>,
    /// Down to the pictures.
    pub compact: bool,
    /// While dragged: how it was, to open again as wide when it ends narrow.
    drag: Option<(Option<u16>, bool)>,
    /// The pointer's over it.
    hover: bool,
    /// The pointer over the narrow list: the name beside its row.
    tip: Option<Position>,
    // From the last draw, for the mouse.
    area: Rect,
    body: Rect,
}

impl Divider {
    pub fn load() -> Divider {
        Divider {
            width: config::sidebar_width(),
            compact: config::sidebar_compact(),
            drag: None,
            hover: false,
            tip: None,
            area: Rect::default(),
            body: Rect::default(),
        }
    }
}

/// The picture's columns in the narrow list; two for the initials without pictures.
pub(crate) fn slot(app: &App) -> u16 {
    avatar_cells(app).filter(|_| app.settings.on(config::PHOTOS)).map_or(2, |(w, _)| w)
}

/// The narrow list: a cell, the picture, two for the unread count and a cell.
fn compact_width(app: &App) -> u16 {
    slot(app) + 5
}

/// The widest the list goes in `total` columns, the conversation keeping its room.
fn fit(w: u16, total: u16) -> u16 {
    let max = total.saturating_sub(1 + CONV_MIN).max(SIDE_MIN.min(total / 2));
    w.min(max).max(SIDE_MIN.min(max))
}

/// The list's width in `total` columns; a smaller window narrows it, its
/// own width kept for when it grows again.
pub fn side_width(app: &App, total: u16) -> u16 {
    if app.side_compact() {
        return compact_width(app).min(total / 2);
    }
    match app.divider.width {
        Some(w) => fit(w, total),
        // Until dragged, WhatsApp Web's share of the screen.
        None => (total * 3 / 10).clamp(32, 48).min(total / 2),
    }
}

impl App {
    /// The list is down to the pictures; a search shows it whole while it lasts.
    pub fn side_compact(&self) -> bool {
        self.divider.compact && self.focus != Focus::Search && self.search.trim().is_empty()
    }

    /// While dragged, the divider has the mouse; passing over, it lights.
    /// First of all, so nothing else takes the drag.
    pub fn divider_held(&mut self, m: MouseEvent) -> bool {
        if let Some(start) = self.divider.drag {
            if m.kind == MouseEventKind::Drag(MouseButton::Left) {
                self.drag_divider(m.column);
                return true;
            }
            // Let go, also outside the window, where no Up comes.
            self.drop_divider(start);
            if matches!(m.kind, MouseEventKind::Up(_)) {
                return true;
            }
        }
        if m.kind == MouseEventKind::Moved {
            let at = Position::new(m.column, m.row);
            self.divider.hover = self.divider.area.contains(at);
            self.divider.tip = (self.side_compact() && self.side_list.contains(at)).then_some(at);
        }
        false
    }

    /// A press on the divider starts dragging it; after the popups had theirs.
    pub fn divider_mouse(&mut self, m: MouseEvent) -> bool {
        if m.kind != MouseEventKind::Down(MouseButton::Left) || !self.divider.area.contains(Position::new(m.column, m.row)) {
            return false;
        }
        self.divider.drag = Some((self.divider.width, self.divider.compact));
        true
    }

    fn drag_divider(&mut self, col: u16) {
        let body = self.divider.body;
        let w = col.saturating_sub(body.x);
        // Past halfway from the narrowest whole list to the narrow one, it snaps there.
        let compact = w < (compact_width(self) + SIDE_MIN) / 2;
        self.divider.compact = compact;
        if compact {
            // Out of the search, or the list stays whole for it.
            if self.focus == Focus::Search && self.search.trim().is_empty() {
                self.focus = Focus::Sidebar;
            }
        } else {
            self.divider.width = Some(fit(w, body.width));
        }
        self.divider.tip = None;
    }

    fn drop_divider(&mut self, (width, compact): (Option<u16>, bool)) {
        let d = &mut self.divider;
        d.drag = None;
        // Down to the pictures, it opens again as wide as it was.
        if d.compact {
            d.width = width;
        }
        if (d.width, d.compact) == (width, compact) {
            return;
        }
        let w = d.width.map_or(String::new(), |w| w.to_string());
        let _ = config::set(&[("sidebar_width", w), ("sidebar_compact", d.compact.to_string())]);
    }
}

/// The divider, green while dragged or under the pointer.
pub fn draw_bar(f: &mut Frame, app: &mut App, body: Rect, sep: Rect) {
    (app.divider.area, app.divider.body) = (sep, body);
    let (glyph, style) = if app.divider.drag.is_some() || app.divider.hover { ("┃", Style::new().fg(green())) } else { ("│", dim()) };
    let bar: Vec<Line> = (0..sep.height).map(|_| Line::from(glyph).style(style)).collect();
    f.render_widget(Paragraph::new(bar), sep);
}

/// A picture at (x, y), two rows tall; without pictures, the initials in
/// the name's colour.
pub(crate) fn face(f: &mut Frame, app: &mut App, jid: Option<&str>, name: &str, group: bool, x: u16, y: u16) {
    let w = slot(app);
    match jid.filter(|_| avatar_cells(app).is_some() && app.settings.on(config::PHOTOS)) {
        Some(jid) => draw_avatar(f, app, jid, group, Rect::new(x, y, w, 2)),
        None => {
            let color = jid.map_or(TEXT, name_color);
            let r = Rect::new(x, y, w, 1).intersection(f.area());
            f.render_widget(Paragraph::new(Span::styled(initials(name), Style::new().fg(color).bold())), r);
        }
    }
}

fn initials(name: &str) -> String {
    let s: String = name.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphabetic())).take(2).flat_map(char::to_uppercase).collect();
    if s.is_empty() {
        "#".into()
    } else {
        s
    }
}

/// The unread count beside the picture, green as the list's; "…" while typing.
fn badge(unread: u32, muted: bool, typing: bool) -> Vec<Span<'static>> {
    if unread > 0 {
        let n = if unread > 9 { "9+".to_string() } else { unread.to_string() };
        let bg = if muted { META_IN } else { green() };
        let pad = " ".repeat(2 - n.width());
        return vec![Span::styled(n, Style::new().fg(Color::Black).bg(bg).bold()), Span::raw(pad)];
    }
    if typing {
        return vec![Span::styled("… ", Style::new().fg(green()))];
    }
    vec![Span::raw("  ")]
}

/// A glyph in the middle of the picture's column.
fn centered(glyph: &str, style: Style, w: u16) -> Vec<Span<'static>> {
    let before = (w as usize).saturating_sub(glyph.width()) / 2;
    let after = (w as usize).saturating_sub(before + glyph.width());
    vec![Span::raw(" ".repeat(before)), Span::styled(glyph.to_string(), style), Span::raw(" ".repeat(after))]
}

/// The list down to the pictures: the search a round button on top, each
/// chat its picture and its unread count, the rows lit and spaced as the
/// whole list's.
pub fn sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    app.chips.clear();
    app.chat_menu.chevron = None;
    let w = slot(app);
    // The search: a click opens the field, and the whole list with it.
    let icon = format!(" {SEARCH} ");
    let btn_w = icon.width() as u16 + 2;
    let btn = Rect::new(area.x + 1 + area.width.saturating_sub(2 + btn_w) / 2, area.y, btn_w, 1).intersection(area);
    app.search_area = btn;
    let pill = Line::from(vec![
        Span::styled(CAP_L, Style::new().fg(FIELD)),
        Span::styled(icon, Style::new().fg(META_IN).bg(FIELD)),
        Span::styled(CAP_R, Style::new().fg(FIELD)),
    ]);
    f.render_widget(Paragraph::new(pill), btn);

    let list = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
    app.side_list = list;
    app.side_hits.clear();
    let items = app.items();
    if items.is_empty() {
        return;
    }
    // Scrolled and lit as the whole list does.
    let cur = app.cursor_index(&items);
    let rows = |from: usize, to: usize| items[from..to].iter().map(|i| i.height()).sum::<u16>();
    let mut offset = app.side_offset.min(items.len() - 1);
    if app.follow {
        offset = offset.min(cur);
        while offset < cur && rows(offset, cur + 1) > list.height {
            offset += 1;
        }
    }
    while offset > 0 && rows(offset - 1, items.len()) <= list.height {
        offset -= 1;
    }
    app.side_offset = offset;
    let idle = app.cursor.is_none() && app.open.is_none() && app.search.trim().is_empty();
    let lit = if app.focus == Focus::Input || idle && app.focus == Focus::Sidebar {
        app.open.as_ref().map(|j| format!("c:{j}"))
    } else {
        items.get(cur).and_then(Item::key)
    };

    let inner = Rect { x: list.x + 1, width: list.width.saturating_sub(2), ..list };
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut faces: Vec<(u16, Option<String>, String, bool)> = Vec::new();
    let mut lit_at = None;
    let mut y = list.y;
    for item in items.iter().skip(offset) {
        let h = item.height();
        if y + h > list.bottom() {
            break;
        }
        let key = item.key();
        let on = key.is_some() && key == lit;
        let mut first = vec![Span::raw(" ")];
        match item {
            Item::Chat(c) => {
                first.push(Span::raw(" ".repeat(w as usize)));
                first.extend(badge(c.unread, c.muted, app.typing.contains_key(&c.jid)));
                faces.push((y, Some(c.jid.clone()), display_name(&c.name, &c.jid), c.group));
            }
            Item::Contact(c) => faces.push((y, Some(c.jid.clone()), c.name.clone(), false)),
            Item::Number(n) => faces.push((y, Some(n.jid.clone()), phone(&n.jid), false)),
            Item::Archived => {
                let n = app.chats.iter().filter(|c| c.archived && c.unread > 0).count() as u32;
                first.extend(centered(crate::chatmenu::ARCHIVE, Style::new().fg(META_IN), w));
                first.extend(badge(n, false, false));
            }
            Item::Back => first.extend(centered("←", Style::new().fg(TEXT).bold(), w)),
            Item::Header(_) => first.push(Span::styled("─".repeat(w as usize), dim())),
            Item::Message(_) => first.extend(centered("•", Style::new().fg(META_IN), w)),
            Item::Info(_) => {}
        }
        let bg = if on { Style::new().bg(SELECTED) } else { Style::new() };
        let used: usize = first.iter().map(|s| s.content.width()).sum();
        first.push(Span::raw(" ".repeat(width.saturating_sub(used))));
        lines.push(Line::from(first).style(bg));
        while lines.len() < (y + h - list.y) as usize {
            lines.push(Line::from(" ".repeat(width)).style(bg));
        }
        if on {
            lit_at = Some((y, h));
        }
        if let Some(k) = key {
            app.side_hits.push((Rect::new(list.x, y, list.width, h), k));
        }
        y += h;
    }
    f.render_widget(Paragraph::new(lines), inner);
    // The lit row's round ends, as the whole list pads it.
    if let Some((top, h)) = lit_at.filter(|_| app.picker.is_some() && width > 1) {
        let (x0, cols) = (inner.x, width as u16);
        let below = top + h - 1;
        for x in x0..x0 + cols {
            f.buffer_mut()[(x, below)].set_bg(Color::Reset);
        }
        draw_deco(f, app, Deco::Edge(true, cols, SELECTED), x0, below);
        let blank = |f: &mut Frame, y: u16| (x0..x0 + cols).all(|x| f.buffer_mut()[(x, y)].symbol() == " ");
        if top > inner.y && blank(f, top - 1) {
            draw_deco(f, app, Deco::Edge(false, cols, SELECTED), x0, top - 1);
        }
    }
    for (y, jid, name, group) in faces {
        face(f, app, jid.as_deref(), &name, group, inner.x + 1, y);
    }
}

/// The name of the row under the pointer, beside the narrow list, over the conversation.
pub fn draw_tip(f: &mut Frame, app: &mut App) {
    let Some(at) = app.divider.tip.filter(|_| app.side_compact() && app.divider.drag.is_none()) else { return };
    let Some((r, key)) = app.side_hits.iter().find(|(r, _)| r.contains(at)).cloned() else { return };
    let label = match app.items().into_iter().find(|i| i.key().as_ref() == Some(&key)) {
        Some(Item::Chat(c)) => display_name(&c.name, &c.jid),
        Some(Item::Contact(c)) => c.name.clone(),
        Some(Item::Number(n)) => phone(&n.jid),
        Some(Item::Archived | Item::Back) => "Arquivadas".into(),
        _ => return,
    };
    let screen = f.area();
    let x = app.divider.area.right();
    let label = truncate(&label, (screen.right().saturating_sub(x + 4) as usize).min(40));
    let rect = Rect::new(x, r.y, label.width() as u16 + 4, 1).intersection(screen);
    let line = Line::from(vec![
        Span::styled(CAP_L, Style::new().fg(FIELD)),
        Span::styled(format!(" {label} "), Style::new().fg(TEXT).bg(FIELD)),
        Span::styled(CAP_R, Style::new().fg(FIELD)),
    ]);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(line), rect);
}
