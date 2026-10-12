//! Reactions, as on WhatsApp Web: hovering a message shows a smiley by its
//! bubble (and the message menu has "Reagir"), whose bar has six emojis and a
//! + for the emoji panel; your own reaction picked again takes it back.
//! Also a click on a reply's quote, which goes to the message it quotes.

use crate::app::{App, Phase};
use crate::ui::{draw_deco, Deco, CAP_L, CAP_R, DAY_PILL, META_IN, TEXT};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;
use serde_json::json;

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
        match QUICK.get(i) {
            Some(e) => self.react(&bar.chat, &bar.id, e),
            None => self.emoji_panel_react((bar.chat, bar.id)),
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

    /// While the bar's open: ← → choose, Enter picks, Esc closes; any other
    /// key closes it and goes on.
    pub fn react_key(&mut self, k: KeyEvent) -> bool {
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

    /// The bar's clicks and hovering, the smiley, a click on a quote; and which message the pointer's over.
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
        match m.kind {
            MouseEventKind::Moved => {
                self.hover_msg = self.bubbles.iter().find(on_row).map(|h| h.id.clone());
                false
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((r, id)) = self.react_btn.clone().filter(|(r, _)| r.contains(at)) {
                    self.open_react_bar(id, r);
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

/// The smiley by the hovered (or reacted-to) message, outside its bubble:
/// to the right of others', to the left of yours.
pub fn draw_button(f: &mut Frame, app: &mut App) {
    app.react_btn = None;
    if app.msg_menu.select.is_some() {
        return;
    }
    let id = app.react_bar.as_ref().map(|b| b.id.clone()).or_else(|| app.hover_msg.clone());
    let Some(h) = id.and_then(|id| app.bubbles.iter().find(|h| h.id == id)) else { return };
    let area = app.msgs_area;
    let x = if h.from_me { h.rect.x.checked_sub(3) } else { Some(h.rect.right() + 1).filter(|x| x + 2 <= area.right()) };
    let Some(x) = x.filter(|&x| x >= area.x) else { return };
    let r = Rect::new(x, h.rect.y + h.rect.height.saturating_sub(1) / 2, 2, 1);
    f.render_widget(Paragraph::new(Span::styled(SMILEY, Style::new().fg(META_IN))), r);
    app.react_btn = Some((r, h.id.clone()));
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
            for col in x..x + w {
                f.buffer_mut()[(col, row)].set_symbol(" ");
            }
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
