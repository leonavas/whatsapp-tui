//! Files sent with a message, as WhatsApp Web attaches them: pasted with
//! Ctrl+V or picked through the clip's menu, shown over the text in the
//! box, and sent one message each, the caption on the first. Terminals
//! don't forward pasted images, so the clipboard is read directly
//! (wl-paste, else xclip), as slick does.

use crate::app::{App, Focus, Phase};
use crate::ui::{box_corners, draw_deco, FIELD, META_IN, TEXT};
use crate::Ev as AppEv;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use image::DynamicImage;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;
use ratatui_image::{Image, Resize};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

/// A card: three rows inside a rounded border.
const CARD_ROWS: u16 = 5;
const MENU_BG: Color = Color::Rgb(0x23, 0x31, 0x38);
const MENU_HOVER: Color = Color::Rgb(0x18, 0x22, 0x29);
const CARD_EDGE: Color = Color::Rgb(0x55, 0x58, 0x58);

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Video,
    Audio,
    Document,
}

// What WhatsApp shows or plays inline (the bridge's list); the rest goes as
// a document.
const IMAGES: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
const VIDEOS: [&str; 3] = ["mp4", "m4v", "3gp"];
const AUDIOS: [&str; 7] = ["mp3", "m4a", "aac", "ogg", "oga", "opus", "amr"];

impl Kind {
    fn of(path: &Path) -> Kind {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let ext = ext.as_str();
        if IMAGES.contains(&ext) {
            Kind::Image
        } else if VIDEOS.contains(&ext) {
            Kind::Video
        } else if AUDIOS.contains(&ext) {
            Kind::Audio
        } else {
            Kind::Document
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Document => "document",
        }
    }

    fn icon(self) -> (&'static str, Color) {
        match self {
            Kind::Image => (PICKS[1].icon, PICKS[1].color),
            Kind::Video => ("\u{f03d}", PICKS[1].color),
            Kind::Audio => (PICKS[2].icon, PICKS[2].color),
            Kind::Document => (PICKS[0].icon, PICKS[0].color),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Image => "Foto",
            Kind::Video => "Vídeo",
            Kind::Audio => "Áudio",
            Kind::Document => "Documento",
        }
    }
}

pub struct Attachment {
    /// Its pictures' key in `App::protocols`; given when it arrives.
    pub key: String,
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub kind: Kind,
    pub thumb: Option<DynamicImage>,
    /// Made by whatsapp-tui (a pasted picture): removed once sent or dropped.
    pub temp: bool,
}

impl Attachment {
    /// How a draft of only this reads in the chat list.
    pub fn draft_label(&self) -> String {
        match self.kind {
            Kind::Image => "📷 Foto".into(),
            Kind::Video => "🎥 Vídeo".into(),
            Kind::Audio => format!("🎧 {}", self.name),
            Kind::Document => format!("📄 {}", self.name),
        }
    }
}

/// One choice of the clip's menu.
struct Pick {
    icon: &'static str,
    label: &'static str,
    color: Color,
    /// The chooser's filter; empty takes any file.
    exts: &'static [&'static str],
}

const PICKS: [Pick; 3] = [
    Pick { icon: "\u{f15b}", label: "Documento", color: Color::Rgb(0x7F, 0x66, 0xFF), exts: &[] },
    Pick { icon: "\u{f03e}", label: "Fotos e vídeos", color: Color::Rgb(0x00, 0x7B, 0xFC), exts: &["jpg", "jpeg", "png", "webp", "mp4", "m4v", "3gp"] },
    Pick { icon: "\u{f025}", label: "Áudio", color: Color::Rgb(0xFA, 0x65, 0x33), exts: &AUDIOS },
];

#[derive(Default)]
pub struct Attach {
    /// Per chat, so they wait there like the typed draft.
    pub files: HashMap<String, Vec<Attachment>>,
    /// The clip's menu, open with its highlighted row.
    pub menu: Option<usize>,
    menu_rows: Vec<(Rect, usize)>,
    menu_area: Rect,
    /// Each card's ✕ on screen and the attachment it removes.
    close: Vec<(Rect, String)>,
    reading: bool,
    picking: bool,
    next: u64,
}

pub enum Clip {
    Files(Vec<Attachment>),
    Text(String),
    Empty,
}

pub enum Ev {
    Clip { chat: String, res: Result<Clip, String> },
    Picked { chat: String, res: Result<Vec<Attachment>, String> },
    /// The drafts kept from the last run: text and files by chat.
    Restored { texts: HashMap<String, String>, files: HashMap<String, Vec<Attachment>> },
}

impl App {
    fn can_attach(&self) -> Option<String> {
        let chat = self.open.clone()?;
        self.chat(&chat).is_none_or(|c| c.read_only.is_empty()).then_some(chat)
    }

    /// The open chat's attachments.
    pub fn attached(&self) -> &[Attachment] {
        self.open.as_ref().and_then(|j| self.attach.files.get(j)).map_or(&[], Vec::as_slice)
    }

    fn spawn_ev(&self, f: impl FnOnce() -> Ev + Send + 'static) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(AppEv::Attach(f()));
        });
    }

    /// Ctrl+V, or a paste with nothing in it: files or a picture on the
    /// clipboard are attached, text is typed in. Read off the UI thread.
    pub fn paste_clipboard(&mut self) {
        let Some(chat) = self.can_attach() else { return };
        if std::mem::replace(&mut self.attach.reading, true) {
            return;
        }
        self.spawn_ev(move || Ev::Clip { chat, res: read_clipboard() });
    }

    fn open_menu(&mut self) {
        if self.can_attach().is_some() {
            self.attach.menu = Some(0);
            // One popup at a time over the box.
            self.emoji_panel.open = false;
        }
    }

    /// A choice of the menu: the desktop's file chooser, filtered to it.
    fn pick(&mut self, i: usize) {
        self.attach.menu = None;
        let Some(chat) = self.can_attach() else { return };
        if std::mem::replace(&mut self.attach.picking, true) {
            return;
        }
        self.spawn_ev(move || {
            let res = choose(&PICKS[i]).map(|paths| {
                paths
                    .iter()
                    .filter_map(|p| prepare(p, i))
                    .collect()
            });
            Ev::Picked { chat, res }
        });
    }

    fn add_files(&mut self, chat: String, files: Vec<Attachment>) {
        let list = self.attach.files.entry(chat).or_default();
        for mut a in files {
            self.attach.next += 1;
            a.key = format!("att:{}", self.attach.next);
            list.push(a);
        }
    }

    fn forget(&mut self, key: &str) {
        for list in self.attach.files.values_mut() {
            list.retain(|a| {
                if a.key != key {
                    return true;
                }
                if a.temp {
                    let _ = std::fs::remove_file(&a.path);
                }
                false
            });
        }
        self.attach.files.retain(|_, l| !l.is_empty());
        self.protocols.retain(|(k, _, _), _| k != key);
    }

    /// Enter with files attached: each goes as its own message, the typed
    /// text as the first one's caption.
    fn send_files(&mut self) {
        let Some(chat) = self.open.clone() else { return };
        if self.phase != Phase::Ready {
            return self.note("Ainda conectando; tente de novo em instantes.");
        }
        let Some(files) = self.attach.files.remove(&chat) else { return };
        let mut caption = self.input.trim().to_string();
        for a in files {
            self.cmd(json!({
                "c": "send_file", "chat": chat, "path": a.path, "name": a.name,
                "caption": std::mem::take(&mut caption), "kind": a.kind.wire(), "temp": a.temp,
            }));
            self.protocols.retain(|(k, _, _), _| *k != a.key);
        }
        self.input.clear();
        self.caret = 0;
        self.scroll = 0;
    }

    /// Keys for the menu and the attachments, ahead of the box's own: the
    /// menu's arrows, Alt+A, Ctrl+V, Backspace on an empty box, and Enter
    /// with files. True when taken.
    pub fn attach_key(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if let Some(sel) = self.attach.menu {
            let n = PICKS.len();
            match k.code {
                KeyCode::Up => self.attach.menu = Some((sel + n - 1) % n),
                KeyCode::Down | KeyCode::Tab => self.attach.menu = Some((sel + 1) % n),
                KeyCode::Enter => self.pick(sel),
                KeyCode::Esc => self.attach.menu = None,
                KeyCode::Char('a') if alt => self.attach.menu = None,
                _ => {
                    self.attach.menu = None;
                    return false;
                }
            }
            return true;
        }
        if alt && matches!(k.code, KeyCode::Char('a') | KeyCode::Char('A')) {
            self.open_menu();
            return self.attach.menu.is_some();
        }
        if self.focus != Focus::Input {
            return false;
        }
        match k.code {
            KeyCode::Char('v') if ctrl => self.paste_clipboard(),
            KeyCode::Backspace if self.input.is_empty() && !self.attached().is_empty() => {
                let key = self.attached().last().map(|a| a.key.clone()).unwrap_or_default();
                self.forget(&key);
            }
            KeyCode::Enter if self.sends(k) && !self.attached().is_empty() => self.send_files(),
            _ => return false,
        }
        true
    }

    /// The clip, the menu's rows and the cards' ✕. True when handled.
    pub fn attach_mouse(&mut self, m: MouseEvent) -> bool {
        let at = Position::new(m.column, m.row);
        let row = self.attach.menu_rows.iter().find(|(r, _)| r.contains(at)).map(|&(_, i)| i);
        match m.kind {
            MouseEventKind::Moved if self.attach.menu.is_some() => {
                if let Some(i) = row {
                    self.attach.menu = Some(i);
                }
                false
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(i) = row.filter(|_| self.attach.menu.is_some()) {
                    self.pick(i);
                } else if self.attach_btn.contains(at) {
                    match self.attach.menu {
                        Some(_) => self.attach.menu = None,
                        None => self.open_menu(),
                    }
                } else if self.attach.menu.is_some() {
                    // A click away closes it, as on the Web, and does nothing else.
                    if !self.attach.menu_area.contains(at) {
                        self.attach.menu = None;
                    }
                } else if let Some(key) = self.attach.close.iter().find(|(r, _)| r.contains(at)).map(|(_, k)| k.clone()) {
                    self.forget(&key);
                } else {
                    return false;
                }
                true
            }
            _ => false,
        }
    }

    pub fn on_attach(&mut self, ev: Ev) {
        match ev {
            Ev::Clip { chat, res } => {
                self.attach.reading = false;
                match res {
                    Ok(Clip::Files(v)) => self.add_files(chat, v),
                    Ok(Clip::Text(s)) if self.open.as_ref() == Some(&chat) && self.focus == Focus::Input => self.insert(&s),
                    Ok(Clip::Text(_)) => {}
                    Ok(Clip::Empty) => self.note("Nada para colar."),
                    Err(e) => self.note(e),
                }
            }
            Ev::Picked { chat, res } => {
                self.attach.picking = false;
                match res {
                    Ok(v) => self.add_files(chat, v),
                    Err(e) => self.note(e),
                }
            }
            Ev::Restored { texts, files } => {
                // Signed out meanwhile: they were the last account's.
                if matches!(self.phase, crate::app::Phase::Qr(_) | crate::app::Phase::LoggedOut) {
                    self.drafts.mark_restored();
                    self.save_drafts();
                    return;
                }
                for (chat, v) in files {
                    self.add_files(chat, v);
                }
                self.restore_drafts(texts);
            }
        }
    }
}

/// The rows the message box grows by for the cards, with a blank one under them.
pub fn rows(app: &App) -> u16 {
    if app.attached().is_empty() { 0 } else { CARD_ROWS + 1 }
}

fn size(n: u64) -> String {
    let (v, unit) = match n {
        0..1_000 => return format!("{n} B"),
        1_000..1_000_000 => (n as f64 / 1e3, "kB"),
        1_000_000..1_000_000_000 => (n as f64 / 1e6, "MB"),
        _ => (n as f64 / 1e9, "GB"),
    };
    let s = if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") };
    format!("{} {unit}", s.replace('.', ","))
}

fn cut(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        if out.width() + ch.to_string().width() + 1 > width {
            break;
        }
        out.push(ch);
    }
    out + "…"
}

/// The cards in a row over the text: a picture's thumbnail, or a file's
/// icon, name and size; each with a ✕ on its corner.
pub fn draw_strip(f: &mut Frame, app: &mut App, area: Rect) {
    app.attach.close.clear();
    if area.height < CARD_ROWS {
        return;
    }
    let (fw, fh) = app.picker.as_ref().map(|p| p.font_size()).map(|s| (s.width as u64, s.height as u64)).unwrap_or((1, 2));
    let pictures = app.picker.is_some();
    let cards: Vec<(String, String, u64, Kind, Option<(u32, u32)>)> = app
        .attached()
        .iter()
        .map(|a| (a.key.clone(), a.name.clone(), a.size, a.kind, a.thumb.as_ref().map(|t| (t.width(), t.height()))))
        .collect();
    let inner_rows = CARD_ROWS - 2;
    let field = Style::new().bg(FIELD);
    let mut x = area.x;
    for (i, (key, name, bytes, kind, dims)) in cards.iter().enumerate() {
        let picture = dims.filter(|_| pictures);
        let cols = match picture {
            Some((w, h)) => (w as u64 * fh * inner_rows as u64 / (h.max(1) as u64 * fw.max(1))).clamp(6, 24) as u16,
            None => (name.width().max(12) as u16 + 2).min(26),
        };
        let card = Rect::new(x, area.y, cols + 2, CARD_ROWS);
        if card.right() > area.right() {
            let more = format!("+{}", cards.len() - i);
            let w = more.width() as u16;
            if x + w <= area.right() {
                f.render_widget(Paragraph::new(more).style(Style::new().fg(META_IN).bg(FIELD)), Rect::new(x, area.y + CARD_ROWS / 2, w, 1));
            }
            break;
        }
        let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(CARD_EDGE)).style(field);
        let inside = block.inner(card);
        f.render_widget(block, card);
        if picture.is_some() {
            let pkey = (key.clone(), inside.width, inside.height);
            if !app.protocols.contains_key(&pkey) {
                let thumb = app.attached().iter().find(|a| a.key == *key).and_then(|a| a.thumb.clone());
                if let (Some(img), Some(picker)) = (thumb, app.picker.as_ref()) {
                    if let Ok(p) = picker.new_protocol(img, Rect::new(0, 0, inside.width, inside.height).into(), Resize::Fit(None)) {
                        app.protocols.insert(pkey.clone(), p);
                    }
                }
            }
            if let Some(p) = app.protocols.get(&pkey) {
                f.render_widget(Image::new(p), inside);
            }
        } else {
            let (icon, color) = kind.icon();
            let w = inside.width as usize;
            let lines = vec![
                Line::from(vec![Span::styled(format!("{icon} "), Style::new().fg(color)), Span::styled(kind.label(), Style::new().fg(META_IN))]),
                Line::from(cut(name, w)).fg(TEXT),
                Line::from(size(*bytes)).fg(META_IN),
            ];
            f.render_widget(Paragraph::new(lines).style(field), inside);
        }
        let close = Rect::new(card.right() - 4, card.y, 3, 1);
        f.render_widget(Paragraph::new(" ✕ ").style(Style::new().fg(TEXT).bg(FIELD).bold()), close);
        app.attach.close.push((close, key.clone()));
        x = card.right() + 1;
    }
}

/// The clip's menu, over the message box at its left, as on WhatsApp Web.
pub fn draw_menu(f: &mut Frame, app: &mut App) {
    app.attach.menu_rows.clear();
    app.attach.menu_area = Rect::default();
    let Some(sel) = app.attach.menu else { return };
    let btn = app.attach_btn;
    let h = PICKS.len() as u16 + 2;
    let w = PICKS.iter().map(|p| p.label.width()).max().unwrap_or(0) as u16 + 10;
    let Some(y) = app.input_area.y.checked_sub(h).filter(|&y| y >= app.msgs_area.y && btn.width > 0) else {
        app.attach.menu = None;
        return;
    };
    let x = btn.x.saturating_sub(1);
    let rounded = app.picker.is_some();
    let bg = Style::new().bg(MENU_BG);
    // The corner cells stay bare: the pictures round them.
    let edge = if rounded { Style::new() } else { bg };
    let blank = Line::from(vec![Span::styled(" ", edge), Span::styled(" ".repeat(w as usize - 2), bg), Span::styled(" ", edge)]);
    let mut lines = vec![blank.clone()];
    for (i, p) in PICKS.iter().enumerate() {
        let row = if i == sel { Style::new().bg(MENU_HOVER) } else { bg };
        let pad = w as usize - 8 - p.label.width();
        lines.push(Line::from(vec![
            Span::styled(" ", bg),
            Span::styled("  ", row),
            Span::styled(p.icon, row.fg(p.color)),
            Span::styled("  ", row),
            Span::styled(p.label, row.fg(TEXT)),
            Span::styled(" ".repeat(pad + 1), row),
            Span::styled(" ", bg),
        ]));
        app.attach.menu_rows.push((Rect::new(x, y + 1 + i as u16, w, 1), i));
    }
    lines.push(blank);
    let area = Rect::new(x, y, w, h);
    app.attach.menu_area = area;
    f.render_widget(Paragraph::new(lines), area);
    if rounded {
        for (row, col, d) in box_corners(0, h as usize, 0, w as usize, MENU_BG, 0) {
            draw_deco(f, app, d, x + col as u16, y + row as u16);
        }
    }
}

// ---- files -----------------------------------------------------------------

/// Where pasted pictures wait to be sent.
pub fn paste_dir() -> PathBuf {
    crate::bridge::cache_dir().join("paste")
}

/// A file as an attachment, for the menu's choice `pick` (None: by its
/// type). A picture gets its thumbnail; one WhatsApp won't show as sent
/// (WebP) goes as a PNG; one that can't be read goes as a document.
pub fn prepare(path: &Path, pick: impl Into<Option<usize>>) -> Option<Attachment> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let mut kind = Kind::of(path);
    match pick.into() {
        Some(0) => kind = Kind::Document,
        Some(1) if !matches!(kind, Kind::Image | Kind::Video) => kind = Kind::Document,
        Some(2) if kind != Kind::Audio => kind = Kind::Document,
        _ => {}
    }
    let mut a = Attachment { key: String::new(), path: path.to_path_buf(), name, size: meta.len(), kind, thumb: None, temp: false };
    if kind == Kind::Image {
        match image::open(path) {
            Ok(img) => {
                let png_or_jpeg = matches!(image::ImageFormat::from_path(path), Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg));
                if !png_or_jpeg {
                    let to = temp_path("png");
                    img.save_with_format(&to, image::ImageFormat::Png).ok()?;
                    a.size = std::fs::metadata(&to).map_or(a.size, |m| m.len());
                    a.path = to;
                    a.temp = true;
                }
                a.thumb = Some(img.thumbnail(320, 320));
            }
            Err(_) => a.kind = Kind::Document,
        }
    }
    Some(a)
}

fn temp_path(ext: &str) -> PathBuf {
    let dir = paste_dir();
    let _ = std::fs::create_dir_all(&dir);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    dir.join(format!("{}-{nanos}.{ext}", std::process::id()))
}

/// The desktop's file chooser, several files at once: zenity, yad or
/// kdialog, whichever is there. Empty when it's cancelled.
fn choose(p: &Pick) -> Result<Vec<PathBuf>, String> {
    use std::process::{Command, Stdio};
    let patterns: Vec<String> = p.exts.iter().flat_map(|e| [format!("*.{e}"), format!("*.{}", e.to_uppercase())]).collect();
    let title = format!("WhatsApp: {}", p.label);
    let gtk_filter = (!patterns.is_empty()).then(|| format!("--file-filter={} | {}", p.label, patterns.join(" ")));
    let tries: [(&str, Vec<String>); 3] = [
        ("zenity", [vec!["--file-selection".into(), "--multiple".into(), "--separator=\n".into(), format!("--title={title}")], gtk_filter.clone().into_iter().collect()].concat()),
        ("yad", [vec!["--file".into(), "--multiple".into(), "--separator=\n".into(), format!("--title={title}")], gtk_filter.into_iter().collect()].concat()),
        (
            "kdialog",
            vec![
                "--getopenfilename".into(),
                "--multiple".into(),
                "--separate-output".into(),
                std::env::var("HOME").unwrap_or_default(),
                if patterns.is_empty() { "*".into() } else { format!("{}|{}", patterns.join(" "), p.label) },
                "--title".into(),
                title.clone(),
            ],
        ),
    ];
    for (cmd, args) in tries {
        let out = Command::new(cmd).args(&args).stdin(Stdio::null()).stderr(Stdio::null()).output();
        match out {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{cmd}: {e}")),
            // Cancelled, or closed.
            Ok(o) if !o.status.success() => return Ok(Vec::new()),
            Ok(o) => return Ok(String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect()),
        }
    }
    Err("Sem seletor de arquivos: instale o zenity, ou cole com Ctrl+V.".into())
}

// ---- the clipboard ---------------------------------------------------------

/// Picture types we can read, the usual first.
const TYPES: [(&str, &str); 3] = [("image/png", "png"), ("image/jpeg", "jpg"), ("image/webp", "webp")];

fn read_clipboard() -> Result<Clip, String> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (tool, package) = if wayland { ("wl-paste", "wl-clipboard") } else { ("xclip", "xclip") };
    let get = |args: &[&str]| run(tool, package, args);
    let types = if wayland { get(&["--list-types"])? } else { get(&["-selection", "clipboard", "-t", "TARGETS", "-o"])? };
    // Nothing copied: both tools just fail.
    let Some(types) = types else { return Ok(Clip::Empty) };
    let types = String::from_utf8_lossy(&types);
    let has = |t: &str| types.lines().any(|l| l.trim() == t);
    let fetch = |mime: &str| -> Result<Option<Vec<u8>>, String> {
        let args: &[&str] = if wayland { &["--no-newline", "--type", mime] } else { &["-selection", "clipboard", "-t", mime, "-o"] };
        get(args)
    };
    // Files copied in a file manager: those files.
    if has("text/uri-list") {
        let list = fetch("text/uri-list")?.unwrap_or_default();
        let files: Vec<Attachment> = String::from_utf8_lossy(&list)
            .lines()
            .filter_map(|l| l.trim().strip_prefix("file://"))
            .filter_map(|p| prepare(Path::new(&unescape(p)), None))
            .collect();
        if !files.is_empty() {
            return Ok(Clip::Files(files));
        }
    }
    if let Some((mime, ext)) = TYPES.iter().copied().find(|(m, _)| has(m)) {
        let Some(bytes) = fetch(mime)?.filter(|b| !b.is_empty()) else { return Ok(Clip::Empty) };
        let to = temp_path(if ext == "webp" { "png" } else { ext });
        let written = match ext {
            "webp" => image::load_from_memory(&bytes).map_err(|e| e.to_string()).and_then(|i| i.save_with_format(&to, image::ImageFormat::Png).map_err(|e| e.to_string())),
            _ => std::fs::write(&to, &bytes).map_err(|e| e.to_string()),
        };
        written.map_err(|e| format!("imagem colada: {e}"))?;
        let mut a = prepare(&to, None).ok_or("imagem colada: ilegível")?;
        a.temp = true;
        a.name = format!("imagem.{}", to.extension().unwrap_or_default().to_string_lossy());
        return Ok(Clip::Files(vec![a]));
    }
    let text = types.lines().any(|l| l.starts_with("text/") || matches!(l.trim(), "UTF8_STRING" | "STRING" | "TEXT"));
    if !text {
        return Ok(Clip::Empty);
    }
    let args: &[&str] = if wayland { &["--no-newline", "--type", "text"] } else { &["-selection", "clipboard", "-t", "UTF8_STRING", "-o"] };
    Ok(get(args)?.map(|b| String::from_utf8_lossy(&b).into_owned()).filter(|s| !s.is_empty()).map_or(Clip::Empty, Clip::Text))
}

/// `%20` and friends in a file URI.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).and_then(|&c| hex(c)), b.get(i + 2).and_then(|&c| hex(c))) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the tool printed, `None` when it failed (an empty clipboard). A tool
/// that hangs, waiting on an app that owns the clipboard, is given up on.
fn run(cmd: &str, package: &str, args: &[&str]) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let spawned = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(format!("Colar precisa do {cmd}: instale o {package}.")),
        Err(e) => return Err(format!("{cmd}: {e}")),
    };
    let mut out = child.stdout.take().ok_or_else(|| format!("{cmd}: sem saída"))?;
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success().then(|| reader.join().unwrap_or_default())),
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("o {cmd} não respondeu"));
            }
        }
    }
}
