//! Reactions: hovering a message shows its quick reactions by the bubble, as
//! Slack's, six emojis and a + for the emoji panel; the message menu's
//! "Reagir" opens them in a bar, as WhatsApp Web's. Your own reaction picked
//! again takes it back.
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
    if app.msg_menu.select.is_some() || app.react_bar.is_some() {
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
