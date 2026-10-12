//! Photos and videos inside their bubbles, as WhatsApp Web shows them: the
//! picture across the bubble, its corners round; a video's with a play
//! button in the middle and how long it lasts at the bottom-left. The
//! message's own thumbnail shows at once, a photo's file (or a video's
//! frame) once it's downloaded. A click opens it in the viewer (`viewer`).

use crate::app::{App, MediaHit, Picture};
use crate::bridge::{Media, Msg};
use image::{imageops::FilterType, DynamicImage, Rgba, RgbaImage};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Frame;
use ratatui_image::{Image, Resize};

/// The most a picture takes, in cells: about WhatsApp Web's 330 px across.
const MOST_COLS: usize = 36;
const MOST_ROWS: usize = 18;
const LEAST_COLS: usize = 16;
const LEAST_ROWS: usize = 4;

/// The play button's disc while the video downloads: the spinner's cell
/// sits on it in this color.
pub(crate) const DISC: [u8; 3] = [0x2A, 0x2F, 0x32];
const SPIN: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// A photo or video in a bubble: where its picture goes, in the
/// conversation's rows and columns, and what it shows.
pub struct PicSpot {
    pub row: usize,
    pub col: usize,
    pub w: usize,
    pub h: usize,
    pub chat: String,
    pub id: String,
    pub video: bool,
    pub gif: bool,
    pub secs: u32,
    pub thumb: String,
    pub file: String,
}

impl PicSpot {
    pub fn new(m: &Msg, md: &Media, row: usize, col: usize, (w, h): (usize, usize)) -> PicSpot {
        PicSpot {
            row,
            col,
            w,
            h,
            chat: m.chat.clone(),
            id: m.id.clone(),
            video: md.kind == "video",
            gif: md.gif,
            secs: md.seconds,
            thumb: md.thumb.clone(),
            file: md.file.clone(),
        }
    }
}

/// The message's photo or video, when its bubble shows one: a video always
/// does (dark until there's a picture), a photo once there's something.
pub fn shown(m: &Msg) -> Option<&Media> {
    m.media.as_ref().filter(|md| md.kind == "video" || (md.kind == "image" && (!md.thumb.is_empty() || !md.file.is_empty())))
}

/// The picture's size in cells for `room` columns, kept to its shape.
pub fn cells(md: &Media, (cw, ch): (u32, u32), room: usize) -> (usize, usize) {
    let (cw, ch) = (cw.max(1) as f64, ch.max(1) as f64);
    let shape = match (md.width, md.height) {
        (w, h) if w > 0 && h > 0 => w as f64 / h as f64,
        _ if md.kind == "video" => 16.0 / 9.0,
        _ => 4.0 / 3.0,
    };
    let most = room.min(MOST_COLS);
    let mut cols = most;
    let mut rows = (cols as f64 * cw / shape / ch).round() as usize;
    if rows > MOST_ROWS {
        rows = MOST_ROWS;
        cols = (rows as f64 * ch * shape / cw).round() as usize;
    }
    (cols.clamp(LEAST_COLS.min(most), most), rows.max(LEAST_ROWS))
}

/// The text under the picture: the caption, without the "🎥 Vídeo" the
/// chat list reads.
pub fn caption(m: &Msg) -> &str {
    for label in ["🎥 Vídeo", "📷 Foto"] {
        if let Some(rest) = m.text.strip_prefix(label) {
            if rest.is_empty() || rest.starts_with(" · ") {
                return rest.trim_start_matches(" · ");
            }
        }
    }
    &m.text
}

pub(crate) fn clock(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Draws the picture at `rect` (the spot, on screen and whole), and keeps
/// it for clicks; a photo not downloaded yet is asked for.
pub fn draw(f: &mut Frame, app: &mut App, s: &PicSpot, rect: Rect) {
    let Some(picker) = app.picker.as_ref() else { return };
    let font = picker.font_size();
    let cell = (font.width as u32, font.height as u32);
    if !s.video && s.file.is_empty() {
        app.want_photo(&s.chat, &s.id);
    }
    // Downloading to open outside the app.
    let loading = app.opening(&s.chat, &s.id);
    // The best there is: a photo's file, then the thumbnail.
    let sources: Vec<String> = [(!s.video).then(|| s.file.clone()), Some(s.thumb.clone())].into_iter().flatten().filter(|p| !p.is_empty()).collect();
    let mut src = None;
    for p in &sources {
        if app.picture(p).is_some() {
            src = Some(p.clone());
            break;
        }
    }
    app.media_hits.push(MediaHit { rect, chat: s.chat.clone(), id: s.id.clone(), video: s.video, gif: s.gif });
    // A photo waits for its picture on the bubble's color.
    if src.is_none() && !s.video {
        return;
    }
    let label = match (s.video, s.gif) {
        (false, _) => None,
        (true, true) => Some("GIF".to_string()),
        (true, false) => (s.secs > 0).then(|| clock(s.secs)),
    };
    let overlay = s.video.then(|| (label.clone().unwrap_or_default(), loading));
    let key = (format!("pic:{}:{overlay:?}", src.as_deref().unwrap_or("-")), rect.width, rect.height);
    if !app.protocols.contains_key(&key) {
        let img = src.as_ref().and_then(|p| match app.pictures.get(p) {
            Some(Picture::Ready(i)) => Some(i),
            _ => None,
        });
        let (w, h) = (rect.width as u32 * cell.0, rect.height as u32 * cell.1);
        let center = middle(rect, cell);
        let pic = compose(img, w, h, cell, center, overlay.as_ref().map(|(l, busy)| (l.as_str(), *busy)), false);
        let Some(picker) = app.picker.as_ref() else { return };
        let Ok(p) = picker.new_protocol(pic, Rect::new(0, 0, rect.width, rect.height).into(), Resize::Fit(None)) else { return };
        app.protocols.insert(key.clone(), p);
    }
    f.render_widget(Image::new(&app.protocols[&key]), rect);
    if loading {
        spinner(f, rect);
    }
}

/// The spinner over the disc in the middle of `rect`, in a cell of its color.
pub(crate) fn spinner(f: &mut Frame, rect: Rect) {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    let disc = Color::Rgb(DISC[0], DISC[1], DISC[2]);
    if let Some(c) = f.buffer_mut().cell_mut((x, y)) {
        c.set_symbol(SPIN[(ms / 100) as usize % SPIN.len()]).set_style(Style::new().fg(Color::White).bg(disc));
    }
}

/// The middle of `rect`'s middle cell, in pixels: where `spinner` goes.
pub(crate) fn middle(rect: Rect, (cw, ch): (u32, u32)) -> (u32, u32) {
    ((rect.width as u32 / 2) * cw + cw / 2, (rect.height as u32 / 2) * ch + ch / 2)
}

/// The picture filling `w`×`h` pixels, its corners round; a video's with
/// the play button on `center` (a disc and no triangle while it
/// downloads) and its label at the bottom-left. `big`, for the viewer: a
/// bigger button and square corners.
pub(crate) fn compose(
    img: Option<&DynamicImage>,
    w: u32,
    h: u32,
    (cw, ch): (u32, u32),
    center: (u32, u32),
    video: Option<(&str, bool)>,
    big: bool,
) -> DynamicImage {
    let mut out = match img {
        Some(i) => i.resize_to_fill(w, h, if i.width() as u64 * i.height() as u64 > 1_000_000 { FilterType::Triangle } else { FilterType::CatmullRom }).to_rgba8(),
        None => RgbaImage::from_pixel(w, h, Rgba([0x11, 0x14, 0x15, 255])),
    };
    let (wf, hf) = (w as f32, h as f32);
    if let Some((label, busy)) = video {
        // A shade up from the bottom, for the label to read on.
        let shade = (ch as f32 * 1.6).min(hf / 2.0);
        if !label.is_empty() {
            for (_, y, p) in out.enumerate_pixels_mut() {
                let t = (y as f32 + 0.5 - (hf - shade)) / shade;
                if t > 0.0 {
                    darken(p, 0.55 * t);
                }
            }
            let scale = ((ch as f32 * 0.42) / 7.0).round().max(1.0) as u32;
            let y0 = h.saturating_sub(ch) + ch.saturating_sub(7 * scale) / 2;
            text(&mut out, label, cw * 6 / 10, y0, scale);
        }
        let (cx, cy) = (center.0 as f32, center.1 as f32);
        let r = (ch as f32 * if big { 1.6 } else { 0.95 }).min(wf.min(hf) * 0.32);
        let tri = r * 0.75;
        for (x, y, p) in out.enumerate_pixels_mut() {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let disc = (r - ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
            if disc <= 0.0 {
                continue;
            }
            if busy {
                blend(p, DISC, disc);
                continue;
            }
            darken(p, 0.5 * disc);
            // The triangle pointing right, its middle a little right of center.
            let (tx, ty) = (px - (cx - tri * 0.4), (py - cy).abs());
            let inside = tx.min(tri * 0.5 * (1.0 - tx / tri) - ty);
            blend(p, [255, 255, 255], (inside + 0.5).clamp(0.0, 1.0));
        }
    }
    if big {
        return DynamicImage::ImageRgba8(out);
    }
    // Round corners, as the bubble's.
    let r = (ch as f32 * 0.3).max(2.0);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let (dx, dy) = ((r - px).max(px - (wf - r)), (r - py).max(py - (hf - r)));
        if dx > 0.0 && dy > 0.0 {
            let a = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            p[3] = (p[3] as f32 * a) as u8;
        }
    }
    DynamicImage::ImageRgba8(out)
}

fn darken(p: &mut Rgba<u8>, by: f32) {
    for c in 0..3 {
        p[c] = (p[c] as f32 * (1.0 - by)) as u8;
    }
}

fn blend(p: &mut Rgba<u8>, color: [u8; 3], a: f32) {
    for c in 0..3 {
        p[c] = (p[c] as f32 + (color[c] as f32 - p[c] as f32) * a) as u8;
    }
}

/// Rows of 5 pixels, the top one first: what a video's label needs.
fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        _ => return None,
    })
}

/// White text from (x, y), each font pixel `scale` across.
fn text(img: &mut RgbaImage, s: &str, x: u32, y: u32, scale: u32) {
    let mut at = x;
    for c in s.chars() {
        let Some(rows) = glyph(c) else { continue };
        // The colon is narrower.
        let (skip, cols) = if c == ':' { (1, 3) } else { (0, 5) };
        for (ry, bits) in rows.iter().enumerate() {
            for rx in 0..cols {
                if bits & (0x10 >> (rx + skip)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (at + rx * scale + dx, y + ry as u32 * scale + dy);
                        if px < img.width() && py < img.height() {
                            img.put_pixel(px, py, Rgba([255, 255, 255, 255]));
                        }
                    }
                }
            }
        }
        at += (cols + 1) * scale;
    }
}
