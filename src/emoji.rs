//! The `:` autocomplete in the message box, like WhatsApp Web's: a row of
//! emoji over the field, found by their English names and shortcodes (the
//! `emojis` crate) and by their Portuguese ones (CLDR's, in emoji_pt.txt).

use crate::app::{App, Focus};
use crate::ui::{box_corners, draw_deco, DAY_PILL, META_IN};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::HashMap;
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Matches offered at most.
const LIMIT: usize = 30;
/// The selected emoji's name, at the box's right end, cut at this.
const NAME_W: usize = 24;
/// Around the selected emoji.
const RING: Color = Color::Rgb(0xD1, 0xD7, 0xDB);
/// WhatsApp's most used first, among matches equally good.
const FREQUENT: [&str; 40] = [
    "❤", "😂", "👍", "🙏", "😍", "😊", "🥰", "😘", "🔥", "🎉", "😭", "🤣", "😅", "😁", "👏", "💪", "😉", "🤔", "😢", "😎",
    "🙌", "✨", "💚", "💙", "😆", "🤗", "😴", "😡", "👀", "✅", "👌", "🤝", "😬", "🙄", "😳", "💯", "🎂", "🌹", "😋", "🤩",
];
/// Words a prefix doesn't count in: ":co" isn't every "rosto com…".
const STOP: [&str; 16] = ["a", "o", "e", "de", "do", "da", "dos", "das", "em", "com", "para", "of", "with", "the", "and", "in"];

/// The popup's state; whether it shows follows from what's typed before the caret.
#[derive(Default)]
pub struct Popup {
    /// The fragment `hits` are for.
    query: String,
    /// (emoji, the name it matched by).
    hits: Vec<(&'static str, &'static str)>,
    sel: usize,
    /// The first match on screen, when they don't all fit.
    off: usize,
    /// Esc hides it until the fragment changes.
    dismissed: Option<String>,
    // Where it was on the last draw, for the mouse.
    area: Rect,
    slots: Vec<(Rect, usize)>,
}

struct Term {
    /// Lowercase, without accents.
    norm: String,
    shown: &'static str,
    /// A name or shortcode, rather than one of the other keywords.
    main: bool,
}

struct Entry {
    emoji: &'static str,
    terms: Vec<Term>,
    /// Frequent ones first, then CLDR's order (faces, people, nature…).
    rank: usize,
}

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

/// CLDR leaves out the U+FE0F that the `emojis` crate keeps.
fn bare(e: &str) -> String {
    e.replace('\u{fe0f}', "")
}

impl Term {
    fn new(s: &'static str, main: bool) -> Term {
        Term { norm: fold(s), shown: s, main }
    }

    /// Exact, then a name's word starting with `q`, then another keyword's,
    /// then `q` anywhere.
    fn tier(&self, q: &str) -> Option<u8> {
        if self.norm == q {
            return Some(0);
        }
        let word = self.norm.starts_with(q) || self.norm.split([' ', '_', '-']).any(|w| !STOP.contains(&w) && w.starts_with(q));
        if word {
            Some(if self.main { 1 } else { 2 })
        } else {
            self.norm.contains(q).then_some(3)
        }
    }
}

fn index() -> &'static [Entry] {
    static INDEX: OnceLock<Vec<Entry>> = OnceLock::new();
    INDEX.get_or_init(|| {
        // "emoji<TAB>name | keyword | keyword", the name first.
        let pt: HashMap<&str, &str> = include_str!("emoji_pt.txt").lines().filter_map(|l| l.split_once('\t')).collect();
        emojis::iter()
            .enumerate()
            .map(|(i, e)| {
                let key = bare(e.as_str());
                let mut terms: Vec<Term> = e.shortcodes().chain([e.name()]).map(|s| Term::new(s, true)).collect();
                if let Some(words) = pt.get(key.as_str()) {
                    terms.extend(words.split(" | ").enumerate().map(|(j, w)| Term::new(w, j == 0)));
                }
                let rank = FREQUENT.iter().position(|f| *f == key).unwrap_or(FREQUENT.len() + i);
                Entry { emoji: e.as_str(), terms, rank }
            })
            .collect()
    })
}

/// Every emoji that matches `q`, as (emoji, the name it matched by), best
/// first; among equally good ones, the recently used, then the frequent.
pub(crate) fn matches(q: &str, recent: &[&str]) -> Vec<(&'static str, &'static str)> {
    let q = fold(q);
    let used = |e: &str| recent.iter().position(|r| *r == e).unwrap_or(recent.len());
    let mut hits: Vec<((u8, usize, usize), &'static str, &'static str)> = index()
        .iter()
        .filter_map(|e| {
            let (tier, t) = e.terms.iter().filter_map(|t| Some((t.tier(&q)?, t))).min_by_key(|(tier, _)| *tier)?;
            Some(((tier, used(e.emoji), e.rank), e.emoji, t.shown))
        })
        .collect();
    hits.sort_by_key(|h| h.0);
    hits.into_iter().map(|(_, e, name)| (e, name)).collect()
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '+'
}

/// The `:fragment` right before the caret: the colon at the start or after
/// a space (not "10:30"), then at least two characters. The colon's index
/// (chars) and the text after it.
fn fragment(text: &str, caret: usize) -> Option<(usize, String)> {
    let before: Vec<char> = text.chars().take(caret).collect();
    let mut i = before.len();
    while i > 0 && is_name_char(before[i - 1]) {
        i -= 1;
    }
    if i == 0 || before[i - 1] != ':' || (i >= 2 && !before[i - 2].is_whitespace()) {
        return None;
    }
    let frag: String = before[i..].iter().collect();
    (frag.chars().count() >= 2).then_some((i - 1, frag))
}

fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    for g in s.graphemes(true) {
        if out.width() + g.width() + 1 > width {
            break;
        }
        out.push_str(g);
    }
    out + "…"
}

impl App {
    /// Brings the matches up to what's typed; true while the popup shows.
    fn emoji_open(&mut self) -> bool {
        let p = &mut self.emoji;
        // The emoji panel, open, takes the keys: no popup under it.
        let shown = self.focus == Focus::Input && self.input_area.height > 0 && !self.emoji_panel.open;
        let Some((_, frag)) = fragment(&self.input, self.caret).filter(|_| shown) else {
            p.dismissed = None;
            return false;
        };
        if p.dismissed.as_deref() == Some(frag.as_str()) {
            return false;
        }
        p.dismissed = None;
        if p.query != frag {
            p.hits = matches(&frag, &self.emoji_panel.recent);
            p.hits.truncate(LIMIT);
            p.query = frag;
            p.sel = 0;
            p.off = 0;
        }
        !p.hits.is_empty()
    }

    /// Replaces the `:fragment` with the emoji.
    fn emoji_pick(&mut self, i: usize) {
        let Some((start, _)) = fragment(&self.input, self.caret) else { return };
        let Some(&(e, _)) = self.emoji.hits.get(i) else { return };
        let chars: Vec<char> = self.input.chars().collect();
        let after: String = chars[self.caret..].iter().collect();
        self.input = chars[..start].iter().collect::<String>() + e + &after;
        self.caret = start + e.chars().count();
        self.emoji_panel.used(e);
    }

    /// Keys for the popup, ahead of everything else (Esc above all); true
    /// when it took the key.
    pub fn emoji_key(&mut self, k: KeyEvent) -> bool {
        if !self.emoji_open() {
            return false;
        }
        let n = self.emoji.hits.len();
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let plain = !k.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);
        let p = &mut self.emoji;
        match k.code {
            KeyCode::Left | KeyCode::Up | KeyCode::BackTab => p.sel = (p.sel + n - 1) % n,
            KeyCode::Tab if shift => p.sel = (p.sel + n - 1) % n,
            KeyCode::Right | KeyCode::Down => p.sel = (p.sel + 1) % n,
            KeyCode::Enter | KeyCode::Tab if plain => self.emoji_pick(self.emoji.sel),
            KeyCode::Esc => p.dismissed = Some(p.query.clone()),
            _ => return false,
        }
        true
    }

    /// A click on the popup: an emoji goes in; true when it took the click.
    pub fn emoji_click(&mut self, m: MouseEvent) -> bool {
        let at = Position::new(m.column, m.row);
        if m.kind != MouseEventKind::Down(MouseButton::Left) || !self.emoji.area.contains(at) || !self.emoji_open() {
            return false;
        }
        if let Some(&(_, i)) = self.emoji.slots.iter().find(|(r, _)| r.contains(at)) {
            self.emoji_pick(i);
        }
        true
    }
}

/// The popup: a rounded box over the message box, left-aligned with it, the
/// matches in a row and the selected one ringed.
pub fn draw(f: &mut Frame, app: &mut App) {
    app.emoji.area = Rect::default();
    app.emoji.slots.clear();
    let input = app.input_area;
    // Over the conversation's last rows; the field's margin row stays between.
    let Some(y) = input.y.checked_sub(3).filter(|&y| y >= app.msgs_area.y) else { return };
    if input.height == 0 || !app.emoji_open() {
        return;
    }
    let x = input.x + 1;
    let avail = input.width.saturating_sub(2) as usize;
    let p = &mut app.emoji;
    let n = p.hits.len();
    let name_w = p.hits.iter().map(|h| h.1.width()).max().unwrap_or(0).min(NAME_W);
    // Each emoji takes 3 columns, its ring's sides shared with its neighbours'.
    let fit = |extra: usize| avail.saturating_sub(3 + extra) / 3;
    let extra = if fit(name_w + 2) >= n.min(6) { name_w + 2 } else { 0 };
    let k = n.min(fit(extra));
    if k == 0 {
        return;
    }
    p.off = p.off.min(p.sel).max((p.sel + 1).saturating_sub(k)).min(n - k);
    let w = 3 * k + 3 + extra;

    let rounded = app.picker.is_some();
    let bg = Style::new().bg(DAY_PILL);
    // The corner cells stay bare: the pictures round them.
    let edge = if rounded { Style::new() } else { bg };
    let ring = Style::new().fg(RING).bg(DAY_PILL);
    let s = p.sel - p.off;
    let mut rows: [Vec<Span>; 3] = [vec![Span::styled(" ", edge)], vec![Span::styled(" ", bg)], vec![Span::styled(" ", edge)]];
    for b in 0..=k {
        let side = match b {
            _ if b == s => ["╭", "│", "╰"],
            _ if b == s + 1 => ["╮", "│", "╯"],
            _ => [" "; 3],
        };
        for (row, c) in rows.iter_mut().zip(side) {
            row.push(Span::styled(c, ring));
        }
        if b < k {
            let e = p.hits[p.off + b].0;
            let line = if b == s { "──" } else { "  " };
            rows[0].push(Span::styled(line, ring));
            rows[1].push(Span::styled(e, bg));
            rows[1].push(Span::styled(" ".repeat(2usize.saturating_sub(e.width())), bg));
            rows[2].push(Span::styled(line, ring));
        }
    }
    if extra > 0 {
        let name = truncate(p.hits[p.sel].1, name_w);
        let pad = extra - name.width();
        rows[0].push(Span::styled(" ".repeat(extra), bg));
        rows[1].push(Span::styled(format!("  {name}"), Style::new().fg(META_IN).bg(DAY_PILL)));
        rows[1].push(Span::styled(" ".repeat(pad - 2), bg));
        rows[2].push(Span::styled(" ".repeat(extra), bg));
    }
    rows[0].push(Span::styled(" ", edge));
    rows[1].push(Span::styled(" ", bg));
    rows[2].push(Span::styled(" ", edge));

    let area = Rect::new(x, y, w as u16, 3);
    p.area = area;
    p.slots = (0..k).map(|j| (Rect::new(x + 1 + 3 * j as u16, y, 3, 3), p.off + j)).collect();
    f.render_widget(Paragraph::new(rows.map(Line::from).to_vec()), area);
    if rounded {
        for (row, col, d) in box_corners(0, 3, 0, w, DAY_PILL, 0) {
            draw_deco(f, app, d, x + col as u16, y + row as u16);
        }
    }
}
