//! A contact's (or group's) picture big, as WhatsApp Web opens it from the
//! info panel: at its full size over the whole app, on the media viewer's
//! backdrop, whose it is on top. The small picture, with a spinner, until
//! the full one comes.

use crate::app::{App, Avatar};
use crate::ui::{avatar_cells, display_name, draw_avatar, phone, truncate, CAP_L, CAP_R, META_IN, TEXT};
use crate::viewer::{fit, BACKDROP, KEY};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui_image::{Image, Resize};
use unicode_width::UnicodeWidthStr;

/// WhatsApp's pictures are 640 pixels wide: the room kept while it comes.
const FULL: u32 = 640;

/// The top bar's keys.
const HINTS: [(&str, &str); 2] = [("o", "abrir fora"), ("Esc", "fecha")];

/// Where things were on the last draw, for the mouse.
#[derive(Default)]
pub struct View {
    media: Rect,
    bar: Rect,
    btns: [Rect; 2],
}

impl App {
    /// A click on the info panel's picture: big, once there's one. Without
    /// pictures in the terminal, outside the app.
    pub(crate) fn open_photo(&mut self) {
        let Some(p) = self.profile.as_ref() else { return };
        match p.pic.clone() {
            Some(path) if path.is_empty() => self.note("Sem foto do perfil"),
            Some(path) if self.picker.is_none() => crate::links::open(&path),
            None if self.picker.is_none() => self.note("Carregando a foto…"),
            _ => {
                if let Some(p) = self.profile.as_mut() {
                    p.big = Some(View::default());
                }
            }
        }
    }

    /// `I` on the list: the open chat's picture, with its info open beside.
    pub(crate) fn chat_photo(&mut self) {
        let Some(chat) = self.open.clone() else { return };
        self.show_profile(chat);
        self.open_photo();
    }

    fn close_photo(&mut self) {
        if let Some(p) = self.profile.as_mut() {
            p.big = None;
        }
        self.protocols.retain(|(k, _, _), _| !k.starts_with("photo:"));
    }

    /// The full picture came (or there's none): open, it shows, or says so.
    pub(crate) fn photo_arrived(&mut self, path: &str, error: &str) {
        if !path.is_empty() || !self.profile.as_ref().is_some_and(|p| p.big.is_some()) {
            return;
        }
        self.close_photo();
        self.note(if error.is_empty() { "Sem foto do perfil" } else { error });
    }

    /// `o`: outside the app instead.
    fn photo_outside(&mut self) {
        let path = self.profile.as_ref().and_then(|p| p.pic.clone()).filter(|p| !p.is_empty());
        self.close_photo();
        match path {
            Some(p) => crate::links::open(&p),
            None => self.note("Carregando a foto…"),
        }
    }

    /// All the keys while it's open.
    pub(crate) fn photo_key(&mut self, k: KeyEvent) -> bool {
        if !self.profile.as_ref().is_some_and(|p| p.big.is_some()) {
            return false;
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_photo(),
            KeyCode::Char('o') => self.photo_outside(),
            _ => {}
        }
        true
    }

    /// The top bar's hints do what they say; a click off the picture
    /// closes it, as on WhatsApp Web.
    pub(crate) fn photo_mouse(&mut self, m: MouseEvent) -> bool {
        let Some(v) = self.profile.as_ref().and_then(|p| p.big.as_ref()) else { return false };
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return true;
        }
        let at = Position::new(m.column, m.row);
        let off = !v.media.contains(at) && !v.bar.contains(at);
        match v.btns.iter().position(|r| r.contains(at)) {
            Some(0) => self.photo_outside(),
            Some(_) => self.close_photo(),
            None if off => self.close_photo(),
            None => {}
        }
        true
    }
}

/// Draws the picture over the whole screen; false when it's not open.
pub fn draw(f: &mut Frame, app: &mut App) -> bool {
    let Some(p) = app.profile.as_ref().filter(|p| p.big.is_some()) else { return false };
    let Some(cell) = app.picker.as_ref().map(|p| p.font_size()).map(|s| (s.width as u32, s.height as u32)) else { return false };
    let chat = p.chat.clone();
    let group = chat.ends_with("@g.us");
    let loading = p.pic.is_none();
    // A hidden number's (@lid) number, when the bridge knows it.
    let who = match p.data.as_ref().map(|d| d.phone.as_str()) {
        Some(n) if !n.is_empty() => format!("{n}@s.whatsapp.net"),
        _ => chat.clone(),
    };
    let saved = p.data.as_ref().map(|d| d.name.clone()).filter(|n| !n.is_empty());
    let name = display_name(&saved.or_else(|| app.name_of(&chat)).unwrap_or_else(|| phone(&who)), &who);
    let number = phone(&who);
    let sub = if !group && number != name { number } else { String::new() };
    // The full picture's size, once it's here.
    let dims = p.photo.as_ref().map(|i| (i.width(), i.height()));
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BACKDROP)), area);

    // The top bar: whose on the left, the keys on the right.
    let bar = Rect { height: 2.min(area.height), ..area };
    let hint_w = |(k, l): (&str, &str)| (k.width() + 3 + l.width()) as u16;
    let mut x = area.right().saturating_sub(2 + HINTS.iter().map(|&h| hint_w(h) + 3).sum::<u16>() - 3);
    let mut btns = [Rect::default(); 2];
    for (n, &(k, l)) in HINTS.iter().enumerate() {
        let w = hint_w((k, l));
        let r = Rect::new(x, bar.y, w, 1).intersection(area);
        let spans = vec![
            Span::styled(CAP_L, Style::new().fg(KEY)),
            Span::styled(k, Style::new().fg(TEXT).bg(KEY)),
            Span::styled(CAP_R, Style::new().fg(KEY)),
            Span::styled(format!(" {l}"), Style::new().fg(META_IN)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        btns[n] = r;
        x += w + 3;
    }
    let cells = avatar_cells(app);
    let text_x = area.x + 2 + cells.map_or(0, |(w, _)| w + 2);
    let text_w = btns[0].x.saturating_sub(text_x + 2) as usize;
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(truncate(&name, text_w), Style::new().fg(TEXT).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(truncate(&sub, text_w), Style::new().fg(META_IN))),
        ]),
        Rect::new(text_x, bar.y, text_w as u16, bar.height).intersection(area),
    );
    if let Some((w, h)) = cells.filter(|_| area.height >= 2) {
        draw_avatar(f, app, &chat, group, Rect::new(area.x + 2, bar.y, w, h));
    }

    // The picture in the middle, at most at its real size: the small one
    // in the full one's room while it comes.
    let (top, bottom) = (area.y + 3, area.bottom().saturating_sub(2));
    let (box_w, box_h) = (area.width.saturating_sub(4), bottom.saturating_sub(top));
    let media = if box_w >= 4 && box_h >= 2 {
        let (w, h) = dims.filter(|&(w, h)| w > 0 && h > 0).unwrap_or((FULL, FULL));
        let (cols, rows) = fit(w as f64 / h as f64, cell, (w.div_ceil(cell.0) as u16).clamp(1, box_w), box_h);
        Rect::new(area.x + (area.width - cols) / 2, top + (box_h - rows) / 2, cols, rows)
    } else {
        Rect::default()
    };
    if media.width > 0 {
        let key = (format!("photo:{chat}:{loading}:{}", dims.is_some()), media.width, media.height);
        if !app.protocols.contains_key(&key) {
            let small = match app.avatars.get(&chat) {
                Some(Avatar::Ready(img)) => Some(img),
                _ => None,
            };
            let img = app.profile.as_ref().and_then(|p| p.photo.as_ref()).or(small);
            let (w, h) = (media.width as u32 * cell.0, media.height as u32 * cell.1);
            let center = crate::media::middle(media, cell);
            let pic = crate::media::compose(img, w, h, cell, center, loading.then_some(("", true)), true);
            if let Some(Ok(proto)) = app.picker.as_ref().map(|pk| pk.new_protocol(pic, Rect::new(0, 0, media.width, media.height).into(), Resize::Fit(None))) {
                app.protocols.insert(key.clone(), proto);
            }
        }
        if let Some(proto) = app.protocols.get(&key) {
            f.render_widget(Image::new(proto), media);
        }
        if loading {
            crate::media::spinner(f, media);
        }
    }

    if let Some(v) = app.profile.as_mut().and_then(|p| p.big.as_mut()) {
        (v.media, v.bar, v.btns) = (media, bar, btns);
    }
    true
}
