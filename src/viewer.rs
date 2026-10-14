//! The media viewer, as WhatsApp Web's: a photo or video big over the whole
//! app, on a dark backdrop; who sent it and when on top, the caption under
//! it. A video plays there through mpv (`video`), which then has the screen:
//! the viewer is drawn first, mpv goes over its picture's cells, and once it
//! ends the picture shows again with its play button, until it's closed.

use crate::app::App;
use crate::ui::{avatar_cells, day_label, display_name, draw_avatar, local, phone, truncate, wrap, CAP_L, CAP_R, META_IN, TEXT};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui_image::{Image, Resize};
use serde_json::json;
use unicode_width::UnicodeWidthStr;

/// WhatsApp Web's viewer, near black.
pub(crate) const BACKDROP: Color = Color::Rgb(0x0B, 0x0E, 0x0F);
/// A key's chip, as in "Atalhos de teclado".
pub(crate) const KEY: Color = Color::Rgb(0x3B, 0x3E, 0x3E);
/// The most a video is blown up past its own pixels; photos and stickers
/// show at most at their real size.
const MOST_ZOOM: u32 = 2;
/// Caption rows at most.
const CAPTION_ROWS: usize = 3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    /// Downloading; a video plays once it's here.
    Loading,
    /// The picture shows; a video's with its play button.
    Idle,
    /// Drawn with its picture's cells blank: mpv starts right after.
    Starting,
    /// mpv has the screen.
    Playing,
}

pub struct View {
    pub chat: String,
    pub id: String,
    pub video: bool,
    pub gif: bool,
    pub state: State,
    // Where things were on the last draw, for mpv and the mouse.
    media: Rect,
    bar: Rect,
    /// The top bar's hints, as `HINTS` has them.
    btns: [Rect; 4],
    /// A sticker's place among your favorites, once the bridge says; its
    /// hint (`f`) goes before the others.
    fav: Option<bool>,
    fav_btn: Rect,
}

/// The top bar's keys, as slick's viewer has them.
const HINTS: [(&str, &str); 4] = [("b", "baixar"), ("p", "mostrar na pasta"), ("o", "abrir fora"), ("Esc", "fecha")];

impl App {
    /// Opens the message's photo or video big; a video plays at once,
    /// downloaded first if it has to be.
    pub(crate) fn open_viewer(&mut self, chat: &str, id: &str, video: bool, gif: bool) {
        let file = self.media_file(chat, id);
        let state = match (&file, video) {
            (None, _) => State::Loading,
            (Some(_), true) => State::Starting,
            (Some(_), false) => State::Idle,
        };
        if file.is_none() {
            self.cmd(json!({"c": "download", "chat": chat, "id": id}));
        }
        let sticker = self.msgs.get(chat).and_then(|l| l.iter().find(|m| m.id == id)).and_then(|m| m.media.as_ref()).is_some_and(|md| md.kind == "sticker");
        if sticker {
            self.cmd(json!({"c": "sticker_fav", "chat": chat, "id": id}));
        }
        let none = Rect::default();
        self.viewer = Some(View { chat: chat.into(), id: id.into(), video, gif, state, media: none, bar: none, btns: [none; 4], fav: None, fav_btn: none });
    }

    pub(crate) fn close_viewer(&mut self) {
        self.video.stop();
        self.viewer = None;
        // Big pictures, not wanted again soon.
        self.protocols.retain(|(k, _, _), _| !k.starts_with("view:"));
    }

    /// The viewer's message downloaded (or not).
    pub(crate) fn viewer_media(&mut self, chat: &str, id: &str, error: &str) {
        let Some(v) = self.viewer.as_mut().filter(|v| v.chat == chat && v.id == id && v.state == State::Loading) else { return };
        if !error.is_empty() {
            v.state = State::Idle;
            return self.note(error);
        }
        v.state = if v.video { State::Starting } else { State::Idle };
    }

    /// The bridge's word on the viewer's sticker: a favorite or not.
    pub(crate) fn viewer_fav_state(&mut self, chat: &str, id: &str, fav: bool) {
        if let Some(v) = self.viewer.as_mut().filter(|v| v.chat == chat && v.id == id) {
            v.fav = Some(fav);
        }
    }

    /// `f`: the sticker added to your favorites, or taken out, as WhatsApp
    /// Web's "Adicionar às favoritas"; the other devices follow.
    fn viewer_fav(&mut self) {
        let Some(v) = self.viewer.as_mut() else { return };
        let Some(on) = v.fav else { return };
        v.fav = Some(!on);
        let action = if on { "unfavorite" } else { "favorite" };
        let (chat, id) = (v.chat.clone(), v.id.clone());
        self.cmd(json!({"c": "sticker_fav", "chat": chat, "id": id, "action": action}));
    }

    /// Play, or stop back to its picture.
    fn viewer_play(&mut self) {
        let Some(v) = self.viewer.as_ref().filter(|v| v.video) else { return };
        match v.state {
            State::Playing => self.video.stop(),
            State::Idle => {
                let (chat, id) = (v.chat.clone(), v.id.clone());
                let next = if self.media_file(&chat, &id).is_some() {
                    State::Starting
                } else {
                    self.cmd(json!({"c": "download", "chat": chat, "id": id}));
                    State::Loading
                };
                if let Some(v) = self.viewer.as_mut() {
                    v.state = next;
                }
            }
            State::Loading | State::Starting => {}
        }
    }

    /// `o`: outside the app instead, once it's downloaded.
    fn viewer_outside(&mut self) {
        let Some(v) = self.viewer.as_ref() else { return };
        let (chat, id, loading) = (v.chat.clone(), v.id.clone(), v.state == State::Loading);
        self.close_viewer();
        match self.media_file(&chat, &id) {
            Some(f) => self.open_outside(&chat, &id, &f),
            None => {
                if !loading {
                    self.cmd(json!({"c": "download", "chat": chat, "id": id}));
                }
                self.open_when_ready = Some((chat, id));
            }
        }
    }

    /// `b`, or with `reveal` (`p`) also shown in the file manager: the
    /// viewer's media saved to the downloads folder, once it's here.
    fn viewer_save(&mut self, reveal: bool) {
        let Some(v) = self.viewer.as_ref() else { return };
        let (chat, id) = (v.chat.clone(), v.id.clone());
        if self.media_file(&chat, &id).is_some() {
            return self.save_media(&chat, &id, reveal);
        }
        if self.save_when_ready.is_none() && v.state != State::Loading {
            self.cmd(json!({"c": "download", "chat": chat, "id": id}));
        }
        self.save_when_ready = Some((chat, id, reveal));
        self.note("Baixando…");
    }

    /// Copies the message's file to the downloads folder, named as WhatsApp
    /// Web names it, never over another; once per message. `reveal` opens
    /// the folder.
    pub(crate) fn save_media(&mut self, chat: &str, id: &str, reveal: bool) {
        let key = (chat.to_string(), id.to_string());
        let path = match self.saved.get(&key).filter(|p| p.exists()) {
            Some(p) => p.clone(),
            None => {
                let Some(file) = self.media_file(chat, id) else { return };
                let Some(m) = self.msgs.get(chat).and_then(|l| l.iter().find(|m| m.id == id)) else { return };
                let kind = match m.media.as_ref().map(|md| md.kind.as_str()) {
                    Some("video") => "Video",
                    Some("sticker") => "Sticker",
                    Some("audio") => "Audio",
                    Some("document") => "Document",
                    _ => "Image",
                };
                let ext = std::path::Path::new(&file).extension().and_then(|e| e.to_str()).unwrap_or("bin").to_string();
                let name = format!("WhatsApp {kind} {}", local(m.ts).format("%Y-%m-%d at %H.%M.%S"));
                match save_copy(&file, &name, &ext) {
                    Ok(p) => {
                        self.saved.insert(key, p.clone());
                        p
                    }
                    Err(e) => return self.note(format!("não salvou: {e}")),
                }
            }
        };
        if reveal {
            if let Some(dir) = path.parent() {
                crate::links::open(&dir.to_string_lossy());
            }
        }
        self.note(format!("Salvo em {}", pretty(&path)));
    }

    /// Right after a draw that left the video's cells blank: mpv takes them.
    pub fn after_draw(&mut self) {
        let Some(v) = self.viewer.as_ref().filter(|v| v.state == State::Starting) else { return };
        let (chat, id, gif, rect) = (v.chat.clone(), v.id.clone(), v.gif, v.media);
        let cell = self.picker.as_ref().map(|p| p.font_size()).map(|s| (s.width as u32, s.height as u32));
        let path = self.media_file(&chat, &id);
        let next = match (path, cell) {
            (Some(path), Some(cell)) if rect.width > 0 && rect.height > 0 => {
                // Not both at once.
                self.player.stop();
                match self.video.play(&path, gif, crate::video::Spot { rect, cell }) {
                    Ok(()) => State::Playing,
                    // Without mpv, whatever opens videos.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        self.close_viewer();
                        return crate::links::open(&path);
                    }
                    Err(e) => {
                        self.note(format!("não tocou o vídeo: {e}"));
                        State::Idle
                    }
                }
            }
            _ => State::Idle,
        };
        if let Some(v) = self.viewer.as_mut() {
            v.state = next;
        }
    }

    /// The viewer's keys: all of them while it's open. While its video has
    /// the screen, they only stop it (Esc closes too).
    pub(crate) fn viewer_key(&mut self, k: KeyEvent) -> bool {
        if self.video.covers() {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => self.close_viewer(),
                KeyCode::Char(' ' | 'v') | KeyCode::Enter => self.video.stop(),
                KeyCode::Char('o') => {
                    self.video.stop();
                    self.viewer_outside();
                }
                _ => {}
            }
            return true;
        }
        if self.viewer.is_none() {
            return false;
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_viewer(),
            KeyCode::Char(' ' | 'v') | KeyCode::Enter => self.viewer_play(),
            KeyCode::Char('o') => self.viewer_outside(),
            KeyCode::Char('b' | 'd') => self.viewer_save(false),
            KeyCode::Char('p') => self.viewer_save(true),
            KeyCode::Char('f') => self.viewer_fav(),
            _ => {}
        }
        true
    }

    /// A click on the picture plays or stops a video; on the top bar's
    /// hints does what they say; anywhere else closes it, as on WhatsApp Web.
    pub(crate) fn viewer_mouse(&mut self, m: MouseEvent) -> bool {
        let Some(v) = self.viewer.as_ref() else {
            // Closed while its video was still on its way out.
            return self.video.covers();
        };
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return true;
        }
        let at = Position::new(m.column, m.row);
        let hit = v.btns.iter().position(|r| r.contains(at));
        if v.fav_btn.contains(at) {
            self.viewer_fav();
        } else if let Some(i) = hit {
            match i {
                0 => self.viewer_save(false),
                1 => self.viewer_save(true),
                2 => self.viewer_outside(),
                _ => self.close_viewer(),
            }
        } else if v.media.contains(at) {
            self.viewer_play();
        } else if !v.bar.contains(at) {
            self.close_viewer();
        }
        true
    }
}

/// Draws the viewer over the whole screen; false when there's nothing to
/// show (the message is gone), and it's closed.
pub fn draw(f: &mut Frame, app: &mut App) -> bool {
    let Some(v) = app.viewer.as_ref() else { return false };
    let (chat, id, video, gif, state, fav) = (v.chat.clone(), v.id.clone(), v.video, v.gif, v.state, v.fav);
    let found = app.msgs.get(&chat).and_then(|l| l.iter().find(|m| m.id == id)).filter(|m| m.media.is_some()).cloned();
    let cell = app.picker.as_ref().map(|p| p.font_size()).map(|s| (s.width as u32, s.height as u32));
    let (Some(m), Some(cell)) = (found, cell) else {
        app.close_viewer();
        return false;
    };
    let md = m.media.clone().unwrap_or_default();
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BACKDROP)), area);

    // The top bar: who sent it and when on the left, the keys on the right.
    let bar = Rect { height: 2.min(area.height), ..area };
    // A sticker's favorite hint first, once the bridge has said which.
    let fav = fav.map(|on| ("f", if on { "desfavoritar" } else { "favoritar" }));
    let hints: Vec<(&str, &str)> = fav.into_iter().chain(HINTS).collect();
    let hint_w = |(k, l): (&str, &str)| (k.width() + 3 + l.width()) as u16;
    let mut x = area.right().saturating_sub(2 + hints.iter().map(|&h| hint_w(h) + 3).sum::<u16>() - 3);
    let first = x;
    let mut btns = [Rect::default(); 4];
    let mut fav_btn = Rect::default();
    for (n, &(k, l)) in hints.iter().enumerate() {
        let w = hint_w((k, l));
        let r = Rect::new(x, bar.y, w, 1).intersection(area);
        let spans = vec![
            Span::styled(CAP_L, Style::new().fg(KEY)),
            Span::styled(k, Style::new().fg(TEXT).bg(KEY)),
            Span::styled(CAP_R, Style::new().fg(KEY)),
            Span::styled(format!(" {l}"), Style::new().fg(META_IN)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        match n.checked_sub(fav.is_some() as usize) {
            Some(i) => btns[i] = r,
            None => fav_btn = r,
        }
        x += w + 3;
    }
    let group = chat.ends_with("@g.us");
    let who = if m.from_me {
        app.my_jid().unwrap_or_default()
    } else if !m.sender.is_empty() {
        m.sender.clone()
    } else {
        chat.clone()
    };
    let name = match () {
        _ if m.from_me => "Você".to_string(),
        _ if group && !m.sender_name.is_empty() => display_name(&m.sender_name, &m.sender),
        _ if group => phone(&m.sender),
        _ => display_name(&app.name_of(&chat).unwrap_or_else(|| m.sender_name.clone()), &chat),
    };
    let t = local(m.ts);
    let mut when = format!("{} às {}", capital(&day_label(t)), t.format("%H:%M"));
    if group {
        if let Some(g) = app.name_of(&chat) {
            when = format!("{when} · {g}");
        }
    }
    let cells = avatar_cells(app);
    let text_x = area.x + 2 + cells.map_or(0, |(w, _)| w + 2);
    let text_w = first.saturating_sub(text_x + 2) as usize;
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(truncate(&name, text_w), Style::new().fg(TEXT).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(truncate(&when, text_w), Style::new().fg(META_IN))),
        ]),
        Rect::new(text_x, bar.y, text_w as u16, bar.height).intersection(area),
    );
    if let Some((w, h)) = cells.filter(|_| !who.is_empty() && area.height >= 2) {
        draw_avatar(f, app, &who, false, Rect::new(area.x + 2, bar.y, w, h));
    }

    // Under the picture: what's going on (or the last error), then the caption.
    let caption = crate::media::caption(&m).trim().to_string();
    let cap_w = area.width.saturating_sub(8).min(80) as usize;
    let mut cap = if caption.is_empty() { Vec::new() } else { wrap(&caption, cap_w.max(1)) };
    if cap.len() > CAPTION_ROWS {
        cap.truncate(CAPTION_ROWS);
        if let Some(last) = cap.last_mut() {
            *last = truncate(&format!("{last} …"), cap_w);
        }
    }
    let status = match app.notice.as_ref() {
        // Red for what went wrong; a save is good news.
        Some((n, _)) if n.starts_with("Salvo") || n.starts_with("Baixando") => Span::styled(n.clone(), Style::new().fg(crate::ui::green())),
        Some((n, _)) => Span::styled(n.clone(), Style::new().fg(Color::Red)),
        None => Span::styled(
            match state {
                State::Loading => "Baixando…".to_string(),
                State::Starting | State::Playing => "Espaço para · Esc fecha".into(),
                State::Idle if video && gif => "GIF · Espaço ou clique toca".into(),
                State::Idle if video => format!("{} · Espaço ou clique toca", crate::media::clock(md.seconds)),
                State::Idle => String::new(),
            },
            Style::new().fg(META_IN),
        ),
    };
    // Status row, the caption with a row before it, a row of margin.
    let foot = 1 + if cap.is_empty() { 0 } else { 1 + cap.len() as u16 } + 1;
    let top = area.y + 3;
    let bottom = area.bottom().saturating_sub(foot + 1);
    let (box_w, box_h) = (area.width.saturating_sub(4), bottom.saturating_sub(top));

    // The best picture there is: a photo's file, then the thumbnail.
    let sources: Vec<String> = [(!video).then(|| md.file.clone()), Some(md.thumb.clone())].into_iter().flatten().filter(|p| !p.is_empty()).collect();
    let mut src = None;
    for p in &sources {
        if app.picture(p).is_some() {
            src = Some(p.clone());
            break;
        }
    }
    let size = match (md.width, md.height) {
        (w, h) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    };
    let shape = match (size, src.as_ref().and_then(|p| app.picture(p))) {
        (Some((w, h)), _) => w as f64 / h as f64,
        (None, Some(i)) if i.height() > 0 => i.width() as f64 / i.height() as f64,
        _ if video => 16.0 / 9.0,
        _ => 4.0 / 3.0,
    };
    let media = if box_w >= 8 && box_h >= 3 {
        let zoom = if video { MOST_ZOOM } else { 1 };
        let most = size.map_or(box_w, |(w, _)| ((w * zoom).div_ceil(cell.0) as u16).clamp(1, box_w));
        let (cols, rows) = fit(shape, cell, most, box_h);
        Rect::new(area.x + (area.width - cols) / 2, top + (box_h - rows) / 2, cols, rows)
    } else {
        Rect::default()
    };

    if media.width > 0 {
        if video && matches!(state, State::Starting | State::Playing) {
            // Blank for mpv, no picture of ours under its frames.
            f.render_widget(Block::new().style(Style::new().bg(Color::Black)), media);
        } else {
            let busy = state == State::Loading;
            let overlay = (video || busy).then_some(busy);
            let key = (format!("view:{}:{overlay:?}", src.as_deref().unwrap_or("-")), media.width, media.height);
            if !app.protocols.contains_key(&key) {
                let img = src.as_ref().and_then(|p| match app.pictures.get(p) {
                    Some(crate::app::Picture::Ready(i)) => Some(i),
                    _ => None,
                });
                let (w, h) = (media.width as u32 * cell.0, media.height as u32 * cell.1);
                let center = crate::media::middle(media, cell);
                let pic = crate::media::compose(img, w, h, cell, center, overlay.map(|b| ("", b)), true);
                let Some(picker) = app.picker.as_ref() else { return true };
                if let Ok(p) = picker.new_protocol(pic, Rect::new(0, 0, media.width, media.height).into(), Resize::Fit(None)) {
                    app.protocols.insert(key.clone(), p);
                }
            }
            if let Some(p) = app.protocols.get(&key) {
                f.render_widget(Image::new(p), media);
            }
            if busy {
                crate::media::spinner(f, media);
            }
        }
    }

    let y = if media.height > 0 { media.bottom() + 1 } else { area.y + 3 };
    let mut lines = vec![Line::from(status)];
    if !cap.is_empty() {
        lines.push(Line::default());
        lines.extend(cap.into_iter().map(|l| Line::from(Span::styled(l, Style::new().fg(TEXT)))));
    }
    let r = Rect::new(area.x + 2, y, area.width.saturating_sub(4), lines.len() as u16).intersection(area);
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), r);

    if let Some(v) = app.viewer.as_mut() {
        (v.media, v.bar, v.btns, v.fav_btn) = (media, bar, btns, fav_btn);
    }
    true
}

/// `$XDG_DOWNLOAD_DIR`, as `xdg-user-dirs` records it, else `~/Downloads`,
/// as slick finds it.
fn downloads_dir() -> std::path::PathBuf {
    use std::path::{Path, PathBuf};
    let home = std::env::var("HOME").unwrap_or_default();
    let config = std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| Path::new(&home).join(".config"));
    let from_file = std::fs::read_to_string(config.join("user-dirs.dirs")).ok().and_then(|text| {
        text.lines().find_map(|l| {
            let v = l.trim().strip_prefix("XDG_DOWNLOAD_DIR=")?.trim_matches('"');
            Some(v.replace("$HOME", &home))
        })
    });
    std::env::var("XDG_DOWNLOAD_DIR")
        .ok()
        .or(from_file)
        .filter(|d| !d.is_empty() && d.trim_end_matches('/') != home.trim_end_matches('/'))
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(&home).join("Downloads"))
}

/// `name.ext` in the downloads folder, else `name (1).ext`…: never over
/// another file. A hard link when it's the same disk, a copy when not.
fn save_copy(from: &str, name: &str, ext: &str) -> std::io::Result<std::path::PathBuf> {
    let dir = downloads_dir();
    std::fs::create_dir_all(&dir)?;
    for n in 0.. {
        let path = dir.join(if n == 0 { format!("{name}.{ext}") } else { format!("{name} ({n}).{ext}") });
        if path.exists() {
            continue;
        }
        if std::fs::hard_link(from, &path).is_err() {
            std::fs::copy(from, &path)?;
        }
        return Ok(path);
    }
    unreachable!()
}

/// `~/Downloads/a.jpg`, for the note.
fn pretty(p: &std::path::Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match p.strip_prefix(&home) {
        Ok(rest) if !home.is_empty() => format!("~/{}", rest.display()),
        _ => p.display().to_string(),
    }
}

/// As big as fits in `most_cols`×`most_rows` cells, kept to its shape.
pub(crate) fn fit(shape: f64, (cw, ch): (u32, u32), most_cols: u16, most_rows: u16) -> (u16, u16) {
    let (cw, ch) = (cw.max(1) as f64, ch.max(1) as f64);
    let mut cols = most_cols as f64;
    let mut rows = cols * cw / shape / ch;
    if rows > most_rows as f64 {
        rows = most_rows as f64;
        cols = rows * ch * shape / cw;
    }
    ((cols.round() as u16).clamp(1, most_cols), (rows.round() as u16).clamp(1, most_rows))
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}
