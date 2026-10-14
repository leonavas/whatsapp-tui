//! The emoji panel over the message box, like WhatsApp Web's: category
//! tabs, a search, and every emoji in a grid by section, the recent ones
//! first. The smiley in the message box or Alt+E opens it.

use crate::app::{App, Focus};
use crate::ui::{box_corners, draw_deco, CAP_L, CAP_R, DAY_PILL, FIELD, green, META_IN, SEARCH, TEXT};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use emojis::{Emoji, Group, UnicodeVersion};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const TABS: [(&str, &str); 8] = [
    ("\u{f118}", "Smileys e pessoas"),
    ("\u{f1b0}", "Animais e natureza"),
    ("\u{f0f4}", "Comidas e bebidas"),
    ("\u{f1e3}", "Atividades"),
    ("\u{f1b9}", "Viagens e lugares"),
    ("\u{f0eb}", "Objetos"),
    ("\u{f08a}", "Símbolos"),
    ("\u{f024}", "Bandeiras"),
];
/// Each emoji's columns in the grid.
const CELL: u16 = 4;
const HEIGHT: u16 = 18;
const MIN_W: u16 = 42;
const MAX_RECENT: usize = 32;
/// Under the highlighted emoji.
const HOVER: Color = Color::Rgb(0x3B, 0x3D, 0x3D);
/// The line the current tab's green sits on.
const RULE: Color = Color::Rgb(0x2E, 0x2F, 0x2F);

#[derive(Default)]
pub struct Panel {
    pub open: bool,
    /// The chat it opened in: another one closes it.
    chat: Option<String>,
    /// Opened by the reaction bar's +: the message (chat, id) the chosen
    /// emoji reacts to, instead of going into the message box.
    pub react: Option<(String, String)>,
    query: String,
    found: Vec<&'static str>,
    /// Most recent first, kept in emoji_recent.json.
    pub recent: Vec<&'static str>,
    /// The recents as they were on opening: the grid doesn't shift under
    /// the mouse as emojis go in.
    shown_recent: Vec<&'static str>,
    /// The highlighted emoji, counting through every section in order.
    sel: usize,
    /// The grid's first row on screen.
    top: usize,
    /// Keep `sel` on screen (off while the wheel scrolls).
    follow: bool,
    // From the last draw, for the keys and the mouse.
    cols: usize,
    rows_shown: usize,
    max_top: usize,
    area: Rect,
    tabs: Vec<Rect>,
    cells: Vec<(Rect, usize)>,
}

/// A row of the grid: a section's title (no emojis), or up to `cols` of its
/// emojis, the first being the `start`th overall.
struct Row<'a> {
    tab: Option<usize>,
    title: Option<&'static str>,
    start: usize,
    items: &'a [&'static str],
}

/// The tab of the section at the top; the last one's when scrolled to the end.
fn current_tab(rows: &[Row], top: usize, max_top: usize) -> Option<usize> {
    let r = if top >= max_top && max_top > 0 { rows.last() } else { rows.get(top) };
    r.and_then(|r| r.tab)
}

/// Emojis the terminal's font likely has: Unicode 15 or older.
fn shows(e: &Emoji) -> bool {
    e.unicode_version() <= UnicodeVersion::new(15, 0)
}

fn tab_of(g: Group) -> usize {
    match g {
        Group::SmileysAndEmotion | Group::PeopleAndBody => 0,
        Group::AnimalsAndNature => 1,
        Group::FoodAndDrink => 2,
        Group::Activities => 3,
        Group::TravelAndPlaces => 4,
        Group::Objects => 5,
        Group::Symbols => 6,
        Group::Flags => 7,
    }
}

/// Each tab's emojis, in CLDR's order; `emojis::iter` already leaves out
/// the skin tones.
fn cats() -> &'static [Vec<&'static str>; 8] {
    static CATS: OnceLock<[Vec<&'static str>; 8]> = OnceLock::new();
    CATS.get_or_init(|| {
        let mut v: [Vec<&'static str>; 8] = Default::default();
        for e in emojis::iter().filter(|e| shows(e)) {
            v[tab_of(e.group())].push(e.as_str());
        }
        v
    })
}

fn recent_path() -> std::path::PathBuf {
    crate::bridge::data_dir().join("emoji_recent.json")
}

impl Panel {
    /// Signed out: the recents were the last account's.
    pub fn forget_recent(&mut self) {
        self.recent.clear();
        self.shown_recent.clear();
        let _ = std::fs::remove_file(recent_path());
    }

    pub fn load() -> Panel {
        let saved: Vec<String> = std::fs::read_to_string(recent_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let recent = saved.iter().filter_map(|s| emojis::get(s)).map(Emoji::as_str).take(MAX_RECENT).collect();
        Panel { recent, ..Default::default() }
    }

    /// An emoji went in, from here or from the `:` popup.
    pub fn used(&mut self, e: &'static str) {
        self.recent.retain(|r| *r != e);
        self.recent.insert(0, e);
        self.recent.truncate(MAX_RECENT);
        let path = recent_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string(&self.recent).unwrap_or_default());
    }

    fn rows(&self) -> Vec<Row<'_>> {
        let mut sections: Vec<(&'static str, Option<usize>, &[&'static str])> = Vec::new();
        if !self.query.is_empty() {
            sections.push(("Resultados", None, &self.found));
        } else {
            if !self.shown_recent.is_empty() {
                sections.push(("Recentes", Some(0), &self.shown_recent));
            }
            sections.extend(TABS.iter().zip(cats()).enumerate().map(|(i, ((_, t), list))| (*t, Some(i), list.as_slice())));
        }
        let mut rows = Vec::new();
        let mut n = 0;
        for (title, tab, list) in sections {
            rows.push(Row { tab, title: Some(title), start: n, items: &[] });
            for chunk in list.chunks(self.cols.max(1)) {
                rows.push(Row { tab, title: None, start: n, items: chunk });
                n += chunk.len();
            }
        }
        rows
    }

    fn selected(&self) -> Option<&'static str> {
        self.rows().iter().find(|r| self.sel >= r.start && self.sel < r.start + r.items.len()).map(|r| r.items[self.sel - r.start])
    }


    /// Moves the highlight: `dx` emojis along, or `dy` rows, keeping the column.
    fn step(&mut self, dx: isize, dy: isize) {
        let sel = {
            let rows = self.rows();
            let cells: Vec<&Row> = rows.iter().filter(|r| !r.items.is_empty()).collect();
            let Some(last) = cells.last() else { return };
            let total = last.start + last.items.len();
            let i = cells.iter().position(|r| self.sel < r.start + r.items.len()).unwrap_or(cells.len() - 1);
            if dy != 0 {
                let j = (i as isize + dy).clamp(0, cells.len() as isize - 1) as usize;
                let col = self.sel.saturating_sub(cells[i].start);
                cells[j].start + col.min(cells[j].items.len() - 1)
            } else {
                (self.sel as isize + dx).clamp(0, total as isize - 1) as usize
            }
        };
        self.sel = sel;
        self.follow = true;
    }

    /// To a tab's section, its title at the top; out of a search.
    fn jump(&mut self, tab: usize) {
        self.query.clear();
        self.found.clear();
        let (top, sel) = {
            let rows = self.rows();
            let at = rows.iter().position(|r| r.tab == Some(tab)).unwrap_or(0);
            (at, rows.get(at + 1).map_or(0, |r| r.start))
        };
        self.top = top;
        self.sel = sel;
        self.follow = false;
    }

    fn searched(&mut self) {
        self.found = crate::emoji::matches(&self.query, &self.recent)
            .into_iter()
            .map(|(e, _)| e)
            .filter(|e| emojis::get(e).is_some_and(shows))
            .collect();
        self.sel = 0;
        self.top = 0;
        self.follow = true;
    }
}

impl App {
    /// The smiley or Alt+E: open the panel, or close it.
    fn emoji_panel_toggle(&mut self) {
        if self.emoji_panel.open {
            self.emoji_panel.open = false;
            return;
        }
        // Only with a message box to put emojis in.
        if self.emoji_btn.width == 0 {
            return;
        }
        self.focus_input();
        if self.focus != Focus::Input {
            return;
        }
        self.attach.menu = None;
        self.chat_menu.popup = None;
        let p = &mut self.emoji_panel;
        p.open = true;
        p.react = None;
        p.chat = self.open.clone();
        p.query.clear();
        p.found.clear();
        p.shown_recent = p.recent.clone();
        p.sel = 0;
        p.top = 0;
        p.follow = true;
    }

    /// The reaction bar's +: the panel, its emoji a reaction to (chat, id).
    pub fn emoji_panel_react(&mut self, target: (String, String)) {
        self.emoji_panel.open = false;
        self.emoji_panel_toggle();
        if self.emoji_panel.open {
            self.emoji_panel.react = Some(target);
        }
    }

    /// The highlighted emoji, at the caret; the panel stays open, as on the Web.
    /// Opened for a reaction, it reacts and closes.
    fn emoji_panel_insert(&mut self) {
        let Some(e) = self.emoji_panel.selected() else { return };
        if let Some((chat, id)) = self.emoji_panel.react.take() {
            self.emoji_panel.used(e);
            self.emoji_panel.open = false;
            return self.react(&chat, &id, e);
        }
        let at = self.input.char_indices().nth(self.caret).map_or(self.input.len(), |(i, _)| i);
        self.input.insert_str(at, e);
        self.caret += e.chars().count();
        self.emoji_panel.used(e);
    }

    /// Alt+E anywhere, then, while open, every key but Ctrl ones: typing
    /// searches. Ahead of the `:` popup and the info panel's Esc.
    pub fn emoji_panel_key(&mut self, k: KeyEvent) -> bool {
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if alt && matches!(k.code, KeyCode::Char('e' | 'E')) && (self.emoji_panel.open || self.emoji_btn.width > 0) {
            self.emoji_panel_toggle();
            return true;
        }
        if !self.emoji_panel.open || self.focus != Focus::Input {
            return false;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.emoji_panel.rows_shown.saturating_sub(1).max(1) as isize;
        let p = &mut self.emoji_panel;
        match k.code {
            KeyCode::Esc => p.open = false,
            KeyCode::Left => p.step(-1, 0),
            KeyCode::Right => p.step(1, 0),
            KeyCode::Up => p.step(0, -1),
            KeyCode::Down => p.step(0, 1),
            KeyCode::PageUp => p.step(0, -page),
            KeyCode::PageDown => p.step(0, page),
            KeyCode::Tab | KeyCode::BackTab => {
                let back = k.code == KeyCode::BackTab || k.modifiers.contains(KeyModifiers::SHIFT);
                let cur = if p.query.is_empty() { current_tab(&p.rows(), p.top, p.max_top) } else { None };
                let n = TABS.len();
                let tab = match cur {
                    Some(t) if back => (t + n - 1) % n,
                    Some(t) => (t + 1) % n,
                    None if back => n - 1,
                    None => 0,
                };
                p.jump(tab);
            }
            KeyCode::Enter => self.emoji_panel_insert(),
            KeyCode::Backspace => {
                if p.query.pop().is_some() {
                    p.searched();
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                p.query.push(c);
                p.searched();
            }
            // Ctrl+N and the like still work.
            _ if ctrl => return false,
            _ => {}
        }
        true
    }

    /// The smiley, and while open, clicks, the wheel and hovering over it.
    pub fn emoji_panel_mouse(&mut self, m: MouseEvent) -> bool {
        let at = Position::new(m.column, m.row);
        let left = m.kind == MouseEventKind::Down(MouseButton::Left);
        if left && self.emoji_btn.contains(at) {
            self.emoji_panel_toggle();
            return true;
        }
        let p = &mut self.emoji_panel;
        if !p.open {
            return false;
        }
        if !p.area.contains(at) {
            // A click away closes it, as on the Web, and does nothing else.
            if matches!(m.kind, MouseEventKind::Down(_)) {
                p.open = false;
                return true;
            }
            return false;
        }
        match m.kind {
            MouseEventKind::ScrollUp => {
                p.top = p.top.saturating_sub(2);
                p.follow = false;
            }
            MouseEventKind::ScrollDown => {
                p.top = (p.top + 2).min(p.max_top);
                p.follow = false;
            }
            MouseEventKind::Moved => {
                if let Some(&(_, i)) = p.cells.iter().find(|(r, _)| r.contains(at)) {
                    p.sel = i;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(t) = p.tabs.iter().position(|r| r.contains(at)) {
                    p.jump(t);
                } else if let Some(&(_, i)) = p.cells.iter().find(|(r, _)| r.contains(at)) {
                    p.sel = i;
                    self.emoji_panel_insert();
                    self.focus = Focus::Input;
                }
            }
            _ => {}
        }
        true
    }
}

/// The panel: a rounded box over the conversation, its bottom left on the
/// message box's; tabs, the search, then the grid.
pub fn draw(f: &mut Frame, app: &mut App) {
    let p = &mut app.emoji_panel;
    p.area = Rect::default();
    p.tabs.clear();
    p.cells.clear();
    if !p.open {
        return;
    }
    // Gone with the message box, another chat or the focus.
    if app.emoji_btn.width == 0 || app.open != p.chat || app.focus != Focus::Input {
        p.open = false;
        return;
    }
    let input = app.input_area;
    let w = (input.width * 3 / 5).max(MIN_W).min(input.width.saturating_sub(2));
    // The field's margin row stays between, as under the `:` popup.
    let h = HEIGHT.min(input.y.saturating_sub(app.msgs_area.y));
    if h < 10 || w < 24 {
        return;
    }
    let area = Rect::new(input.x + 1, input.y - h, w, h);
    let iw = (w - 4) as usize;
    let grid_h = (h - 7) as usize;
    p.cols = iw / CELL as usize;
    p.rows_shown = grid_h;

    let rows = p.rows();
    let max_top = rows.len().saturating_sub(grid_h);
    let mut top = p.top;
    if p.follow {
        if let Some(i) = rows.iter().position(|r| p.sel >= r.start && p.sel < r.start + r.items.len()) {
            // Its section's title along, going up.
            let i_top = if i > 0 && rows[i - 1].title.is_some() { i - 1 } else { i };
            top = top.min(i_top).max((i + 1).saturating_sub(grid_h));
        }
    }
    top = top.min(max_top);
    let cur = if p.query.is_empty() { current_tab(&rows, top, max_top) } else { None };

    let rounded = app.picker.is_some();
    let bg = Style::new().bg(DAY_PILL);
    let edge = if rounded { Style::new() } else { bg };
    let blank = |edge: Style| Line::from(vec![Span::styled(" ", edge), Span::styled(" ".repeat(w as usize - 2), bg), Span::styled(" ", edge)]);
    let mut lines = vec![blank(edge)];

    // Tabs, each icon in the middle of its share, and the line under them.
    let slot = |i: usize| i * iw / TABS.len();
    let mut tabs = vec![Span::styled("  ", bg)];
    let mut rule = vec![Span::styled("  ", bg)];
    let mut tab_rects = Vec::new();
    let mut x = 0;
    for (i, (icon, _)) in TABS.iter().enumerate() {
        let (s, e) = (slot(i), slot(i + 1));
        let mid = s + (e - s - 1) / 2;
        let on = cur == Some(i);
        tabs.push(Span::styled(" ".repeat(mid - x), bg));
        tabs.push(Span::styled(*icon, Style::new().fg(if on { TEXT } else { META_IN }).bg(DAY_PILL)));
        x = mid + 1;
        let (c, fg) = if on { ("━", green()) } else { ("─", RULE) };
        rule.push(Span::styled(c.repeat(e - s), Style::new().fg(fg).bg(DAY_PILL)));
        tab_rects.push(Rect::new(area.x + 2 + s as u16, area.y + 1, (e - s) as u16, 2));
    }
    tabs.push(Span::styled(" ".repeat(iw - x + 2), bg));
    rule.push(Span::styled("  ", bg));
    lines.push(Line::from(tabs));
    lines.push(Line::from(rule));
    lines.push(blank(bg));

    // The search, a pill; what's typed shows its end.
    let field = Style::new().fg(TEXT).bg(FIELD);
    let room = iw.saturating_sub(7);
    let (text, style) = if p.query.is_empty() {
        ("Pesquisar emoji".to_string(), Style::new().fg(META_IN).bg(FIELD))
    } else {
        let mut t: Vec<&str> = p.query.graphemes(true).collect();
        while t.concat().width() > room {
            t.remove(0);
        }
        (t.concat(), field)
    };
    let text_w = text.width().min(room);
    lines.push(Line::from(vec![
        Span::styled("  ", bg),
        Span::styled(CAP_L, Style::new().fg(FIELD).bg(DAY_PILL)),
        Span::styled(format!(" {SEARCH}  "), Style::new().fg(META_IN).bg(FIELD)),
        Span::styled(text, style),
        Span::styled(" ".repeat(room - text_w + 1), field),
        Span::styled(CAP_R, Style::new().fg(FIELD).bg(DAY_PILL)),
        Span::styled("  ", bg),
    ]));
    let caret = Position::new(area.x + 7 + if p.query.is_empty() { 0 } else { text_w as u16 }, area.y + 4);
    lines.push(blank(bg));

    // The grid, centered in the box.
    let lead = 2 + (iw - p.cols * CELL as usize) / 2;
    let mut cells = Vec::new();
    let total = rows.last().map_or(0, |r| r.start + r.items.len());
    for (k, r) in rows.iter().skip(top).take(grid_h).enumerate() {
        let y = area.y + 6 + k as u16;
        let mut spans = vec![Span::styled(" ".repeat(lead), bg)];
        let mut used = lead;
        if let Some(t) = r.title {
            spans.push(Span::styled(t, Style::new().fg(META_IN).bg(DAY_PILL)));
            used += t.width();
        }
        for (j, e) in r.items.iter().enumerate() {
            let i = r.start + j;
            let pad = " ".repeat(2usize.saturating_sub(e.width()));
            if i == p.sel {
                let cap = Style::new().fg(HOVER).bg(DAY_PILL);
                spans.extend([Span::styled(CAP_L, cap), Span::styled(*e, Style::new().bg(HOVER)), Span::styled(pad, Style::new().bg(HOVER)), Span::styled(CAP_R, cap)]);
            } else {
                spans.extend([Span::styled(" ", bg), Span::styled(*e, bg), Span::styled(pad, bg), Span::styled(" ", bg)]);
            }
            cells.push((Rect::new(area.x + (lead + j * CELL as usize) as u16, y, CELL, 1), i));
            used += CELL as usize;
        }
        spans.push(Span::styled(" ".repeat((w as usize).saturating_sub(used)), bg));
        lines.push(Line::from(spans));
    }
    if total == 0 {
        lines.push(Line::from(Span::styled(format!("{}Nenhum emoji encontrado", " ".repeat(lead)), Style::new().fg(META_IN).bg(DAY_PILL))));
    }
    while lines.len() < h as usize - 1 {
        lines.push(blank(bg));
    }
    lines.push(blank(edge));

    p.top = top;
    p.max_top = max_top;
    p.area = area;
    p.tabs = tab_rects;
    p.cells = cells;
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines), area);
    f.set_cursor_position(caret);
    if rounded {
        for (row, col, d) in box_corners(0, h as usize, 0, w as usize, DAY_PILL, 0) {
            draw_deco(f, app, d, area.x + col as u16, area.y + row as u16);
        }
    }
}
