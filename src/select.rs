//! Selecting the messages' text with the mouse, as in a browser: a drag
//! lights what it goes over, across bubbles, and letting go copies it; a
//! double click takes a word, a triple click the whole message. Copied as
//! written; over several messages, a line each, as WhatsApp Web copies
//! them: "[13:47, 14/10/2026] Name: text".

use crate::app::App;
use crate::bridge::Msg;
use crate::ui::{display_name, local, phone, TEXT};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::Frame;
use std::time::{Duration, Instant};

/// Clicks closer than this on the same cell count as one double or triple click.
const MULTI: Duration = Duration::from_millis(500);
/// Held past the top or bottom, the conversation scrolls a row this often.
const AUTOSCROLL: Duration = Duration::from_millis(60);

/// A message as the last draw laid it out: its rows, and its text's cells
/// by row as (row, (col, width, first char, last char)).
pub struct Laid {
    pub id: String,
    pub top: usize,
    pub rows: usize,
    /// Its text is a photo's or video's caption.
    pub caption: bool,
    pub glyphs: Vec<(usize, Vec<(usize, usize, usize, usize)>)>,
}

/// A point of the conversation, by message, so it stays put as it scrolls
/// and as more history comes in: rows from that message's top, and the column.
#[derive(Clone)]
struct Anchor {
    id: String,
    dy: isize,
    col: usize,
}

#[derive(Default)]
pub struct TextSel {
    chat: String,
    /// Where the left button went down on text, while it's held.
    press: Option<(Position, Anchor)>,
    /// Moved off the cell it went down on: a selection, not a click.
    dragging: bool,
    /// The pointer while dragging: the moving end, kept under it as the wheel scrolls.
    pointer: Position,
    /// What's selected, from where it was pressed to where it was let go.
    range: Option<(Anchor, Anchor)>,
    /// The last click, for double and triple ones.
    last: Option<(Instant, Position, u8)>,
    scrolled: Option<Instant>,
    // Where the last draw put things.
    laid: Vec<Laid>,
    area: Rect,
    /// The first row shown, and the screen row it's on.
    start: usize,
    y0: u16,
}

impl TextSel {
    pub fn clear(&mut self) {
        self.press = None;
        self.dragging = false;
        self.range = None;
    }

    pub fn active(&self) -> bool {
        self.range.is_some()
    }

    /// A cell on screen as a point of the conversation; past the top or the
    /// bottom, the start or the end of the row there.
    fn anchor_at(&self, p: Position) -> Option<Anchor> {
        let a = self.area;
        if a.height == 0 {
            return None;
        }
        let (y, col) = match p.y {
            y if y < a.y => (a.y, 0),
            y if y >= a.bottom() => (a.bottom() - 1, usize::MAX),
            y => (y, p.x.saturating_sub(a.x) as usize),
        };
        let row = self.start as isize + y as isize - self.y0 as isize;
        let i = self.laid.partition_point(|l| l.top as isize <= row);
        let l = self.laid.get(i.saturating_sub(1))?;
        Some(Anchor { id: l.id.clone(), dy: row - l.top as isize, col })
    }

    /// Back to the row and column, while that message is still there.
    fn point(&self, a: &Anchor) -> Option<(isize, usize)> {
        self.laid.iter().find(|l| l.id == a.id).map(|l| (l.top as isize + a.dy, a.col))
    }

    /// The ends of the selection, in reading order.
    fn ends(&self) -> Option<((isize, usize), (isize, usize))> {
        let (a, b) = self.range.as_ref()?;
        let (a, b) = (self.point(a)?, self.point(b)?);
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    /// The point on the cells of the chars from `a` to `b` of one message:
    /// a word, or all of it.
    fn span(&self, l: &Laid, a: usize, b: usize) -> Option<(Anchor, Anchor)> {
        let cells = || l.glyphs.iter().flat_map(|(row, cs)| cs.iter().map(move |c| (*row, *c)));
        let (r0, (c0, ..)) = cells().find(|(_, c)| c.3 >= a)?;
        let (r1, (c1, w1, ..)) = cells().filter(|(_, c)| c.2 <= b).last()?;
        let at = |row: usize, col: usize| Anchor { id: l.id.clone(), dy: row as isize - l.top as isize, col };
        Some((at(r0, c0), at(r1, c1 + w1 - 1)))
    }
}

/// Whether a cell `w` wide at (row, col) is between the ends.
fn inside(((r0, c0), (r1, c1)): ((isize, usize), (isize, usize)), row: isize, col: usize, w: usize) -> bool {
    (row, col + w.max(1) - 1) >= (r0, c0) && (row, col) <= (r1, c1)
}

/// Where the conversation's messages are, from its draw, and the selection
/// lit over their text. `start` is the first row shown, on screen row `y0`.
pub fn draw(f: &mut Frame, app: &mut App, laid: Vec<Laid>, area: Rect, start: usize, y0: u16) {
    let s = &mut app.text_sel;
    if app.open.as_deref() != Some(s.chat.as_str()) {
        s.clear();
    }
    (s.laid, s.area, s.start, s.y0) = (laid, area, start, y0);
    // Its moving end follows the pointer as the conversation scrolls under it.
    if s.dragging {
        if let Some(end) = s.anchor_at(s.pointer) {
            s.range = s.range.take().map(|(a, _)| (a, end));
        }
    }
    let Some(ends) = s.ends() else { return };
    let (bg, end) = (crate::theme::selection(), start + area.bottom().saturating_sub(y0) as usize);
    let buf = f.buffer_mut();
    for l in &s.laid {
        for (row, cells) in l.glyphs.iter().filter(|(row, _)| (start..end).contains(row)) {
            let y = y0 + (row - start) as u16;
            for &(col, w, ..) in cells.iter().filter(|c| inside(ends, *row as isize, c.0, c.1)) {
                for x in (col..col + w).map(|c| area.x as usize + c).filter(|&x| x < area.right() as usize) {
                    buf[(x as u16, y)].set_bg(bg).set_fg(TEXT);
                }
            }
        }
    }
}

/// Who wrote it, as WhatsApp Web names them when it copies: you by your name.
fn author(app: &App, chat: &str, m: &Msg) -> String {
    let group = chat.ends_with("@g.us");
    match () {
        _ if m.from_me => crate::account::my_name(app),
        _ if group && !m.sender_name.is_empty() => display_name(&m.sender_name, &m.sender),
        _ if group => phone(&m.sender),
        _ => display_name(&app.name_of(chat).unwrap_or_else(|| m.sender_name.clone()), chat),
    }
}

/// All of a message's text, as the menu's "Copiar" takes it, but for the
/// names in it; a photo without a caption, its label.
fn whole(m: &Msg) -> String {
    let mut t = crate::msgmenu::plain(m);
    if t.is_empty() && m.media.is_some() {
        t = m.text.split(" · ").next().unwrap_or_default().to_string();
    }
    crate::markup::mentions(&t, &m.mentions).0
}

/// The text its cells came from: what `ui::message_lines` laid out.
fn source(m: &Msg, caption: bool) -> String {
    let t = if caption { crate::media::caption(m) } else { m.text.as_str() };
    crate::markup::mentions(t, &m.mentions).0
}

impl App {
    /// The left button down on the messages, not on a link or a button: a
    /// selection may start; two clicks take a word, three the message.
    pub(crate) fn select_press(&mut self, at: Position) {
        let Some(chat) = self.open.clone() else { return };
        let s = &mut self.text_sel;
        let n = match s.last {
            Some((t, p, n)) if p == at && t.elapsed() < MULTI => n % 3 + 1,
            _ => 1,
        };
        s.last = Some((Instant::now(), at, n));
        s.chat = chat;
        let Some(a) = s.anchor_at(at) else { return };
        if n == 1 {
            s.press = Some((at, a));
            return;
        }
        // The cell clicked, in which message's text.
        let Some((row, col)) = s.point(&a) else { return };
        let hit = s.laid.iter().enumerate().find_map(|(i, l)| {
            let cells = &l.glyphs.iter().find(|(r, _)| *r as isize == row)?.1;
            cells.iter().find(|c| (c.0..c.0 + c.1).contains(&col)).map(|c| (i, c.2))
        });
        let Some((i, k)) = hit else { return };
        let id = s.laid[i].id.clone();
        let Some(m) = self.open.as_ref().and_then(|c| self.msgs.get(c)).and_then(|l| l.iter().find(|m| m.id == id)).cloned() else { return };
        let s = &self.text_sel;
        let l = &s.laid[i];
        if n == 3 {
            let last = l.glyphs.iter().flat_map(|(_, cs)| cs).map(|c| c.3).max().unwrap_or(0);
            self.text_sel.range = s.span(l, 0, last);
            self.copy_text(whole(&m));
            return;
        }
        // A word, or the one char clicked when it's no word's.
        let c: Vec<char> = source(&m, l.caption).chars().collect();
        let word = |k: usize| c.get(k).is_some_and(|x| x.is_alphanumeric() || *x == '_');
        let (mut a, mut b) = (k, k);
        if word(k) {
            while a > 0 && word(a - 1) {
                a -= 1;
            }
            while word(b + 1) {
                b += 1;
            }
        }
        self.text_sel.range = s.span(l, a, b);
        let text = self.selected_text();
        self.copy_text(text);
    }

    /// The mouse while the button's held after `select_press`: a drag past
    /// the cell it went down on selects, letting go copies. True when taken.
    pub(crate) fn select_held(&mut self, m: MouseEvent) -> bool {
        let s = &mut self.text_sel;
        let Some((p, a)) = s.press.clone() else { return false };
        let at = Position::new(m.column, m.row);
        match m.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                if !s.dragging && at == p {
                    return true;
                }
                s.dragging = true;
                s.pointer = at;
                if let Some(end) = s.anchor_at(at) {
                    s.range = Some((a, end));
                }
                true
            }
            // Let go, or (no button) let go where it wasn't seen.
            MouseEventKind::Up(MouseButton::Left) | MouseEventKind::Moved => {
                let dragged = s.dragging;
                s.press = None;
                s.dragging = false;
                if !dragged {
                    return false;
                }
                if let Some(end) = s.anchor_at(at).filter(|_| m.kind != MouseEventKind::Moved) {
                    s.range = Some((a, end));
                }
                let text = self.selected_text();
                if text.is_empty() {
                    self.text_sel.range = None;
                } else {
                    self.copy_text(text);
                }
                m.kind != MouseEventKind::Moved
            }
            // The wheel scrolls on, the selection following the pointer.
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => false,
            _ => {
                s.press = None;
                s.dragging = false;
                false
            }
        }
    }

    /// Held past the top or bottom of the conversation, it scrolls.
    pub(crate) fn select_tick(&mut self) {
        let s = &mut self.text_sel;
        if !s.dragging || s.scrolled.is_some_and(|t| t.elapsed() < AUTOSCROLL) {
            return;
        }
        if s.pointer.y < s.area.y {
            self.scroll += 1;
        } else if s.pointer.y >= s.area.bottom() && self.scroll > 0 {
            self.scroll -= 1;
        } else {
            return;
        }
        s.scrolled = Some(Instant::now());
    }

    fn copy_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        crate::msgmenu::copy(&text);
        self.note("Copiado");
    }

    /// What's selected, as written. In one message, just that; over
    /// several, each whole but for the ends' parts, a line each after its
    /// time and author.
    fn selected_text(&self) -> String {
        let s = &self.text_sel;
        let Some(ends) = s.ends() else { return String::new() };
        let Some(msgs) = self.msgs.get(&s.chat) else { return String::new() };
        let ((r0, _), (r1, _)) = ends;
        let mut parts = Vec::new();
        for l in &s.laid {
            let Some(m) = msgs.iter().find(|m| m.id == l.id) else { continue };
            let picked = l.glyphs.iter().flat_map(|(row, cs)| cs.iter().filter(move |c| inside(ends, *row as isize, c.0, c.1)));
            let span = picked.fold(None, |acc: Option<(usize, usize)>, c| Some(acc.map_or((c.2, c.3), |(a, b)| (a.min(c.2), b.max(c.3)))));
            let text = match span {
                Some((a, b)) => crate::markup::as_written(&source(m, l.caption), a, b),
                // Gone over whole, with no text of its own lit: a photo, an audio.
                None if l.top as isize > r0 && ((l.top + l.rows) as isize) <= r1 => whole(m),
                None => continue,
            };
            if !text.trim().is_empty() {
                parts.push((m, text));
            }
        }
        match parts.as_slice() {
            [(_, text)] => text.clone(),
            _ => parts
                .iter()
                .map(|(m, text)| format!("[{}] {}: {text}", local(m.ts).format("%H:%M, %d/%m/%Y"), author(self, &s.chat, m)))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}
