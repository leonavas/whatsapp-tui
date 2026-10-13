//! Drawing: the QR to pair, then the chat list on the left and the open
//! conversation, in WhatsApp's bubbles, on the right.

use crate::app::{audio_of, App, Audio, AudioHit, Avatar, Filter, Focus, Item, Phase};
use crate::player::{Now, State};
use crate::bridge::{Chat, Hours, Msg};
use chrono::{DateTime, Datelike, FixedOffset, Local, TimeZone, Timelike, Utc};
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use image::{DynamicImage, Rgba, RgbaImage};
use ratatui_image::{Image, Resize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// WhatsApp Web's dark theme; the greens follow the terminal's (`theme`).
pub(crate) use crate::theme::{bubble_out, chip_on, chip_on_text, code_out, green, link_out, meta_out, rule_out};
const BUBBLE_IN: Color = Color::Rgb(0x24, 0x26, 0x26);
/// Monospace and code: a shade off each bubble, as on WhatsApp Web.
const CODE_IN: Color = Color::Rgb(0x33, 0x36, 0x36);
pub(crate) const TEXT: Color = Color::Rgb(0xFA, 0xFA, 0xFA);
/// 60% white over each bubble.
pub(crate) const META_IN: Color = Color::Rgb(0xA7, 0xA8, 0xA8);
const READ_BLUE: Color = Color::Rgb(0x53, 0xBD, 0xEB);
/// Links: WhatsApp's blue on the grey bubbles, a light green on our own.
pub(crate) const LINK_IN: Color = Color::Rgb(0x53, 0xBD, 0xEB);
pub(crate) const DAY_PILL: Color = Color::Rgb(0x1D, 0x1F, 0x1F);
pub(crate) const FIELD: Color = Color::Rgb(0x24, 0x26, 0x26);
const SELECTED: Color = Color::Rgb(0x2E, 0x2F, 0x2F);
const NAMES: [Color; 8] = [
    Color::Rgb(0xE5, 0x7A, 0x5F),
    Color::Rgb(0x53, 0xBD, 0xEB),
    Color::Rgb(0xA5, 0xB3, 0x37),
    Color::Rgb(0xE5, 0x8E, 0xE9),
    Color::Rgb(0xF5, 0xB0, 0x41),
    Color::Rgb(0x06, 0xCF, 0x9C),
    Color::Rgb(0xA7, 0x91, 0xFF),
    Color::Rgb(0xFF, 0x72, 0x8E),
];

// Nerd Font half circles: the rounded ends of a one-row pill.
pub(crate) const CAP_L: &str = "\u{e0b6}";
pub(crate) const CAP_R: &str = "\u{e0b4}";
pub(crate) const SEARCH: &str = "\u{f002}";
const CLIP: &str = "\u{f0c6}";
const SMILEY: &str = "\u{f118}";
/// The clip and the smiley, each with two cells after it.
const ICONS_W: usize = 6;
const INFO: &str = "\u{f05a}";
const PIN: &str = "\u{f041}";
const DOWN: &str = "\u{f107}";
const UP: &str = "\u{f106}";
/// WhatsApp's check by a verified business's name, and by its own.
const CHECK: &str = "\u{f0791}";
const CHECK_BLUE: Color = Color::Rgb(0x1D, 0x9B, 0xF0);
/// The rule over a business's buttons, on each bubble.
const RULE_IN: Color = Color::Rgb(0x3A, 0x3D, 0x3E);
const LINK_ICON: &str = "\u{f08e}";
const REPLY_ICON: &str = "\u{f112}";
const CALL_ICON: &str = "\u{f095}";
const COPY_ICON: &str = "\u{f0c5}";
const LIST_ICON: &str = "\u{f03a}";
/// WhatsApp's red, for a business that's closed.
const CLOSED: Color = Color::Rgb(0xF1, 0x5C, 0x6D);

/// The info panel's narrowest beside the conversation, slick's profile's.
const PANEL: u16 = 44;

pub(crate) fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

fn pill(text: String, fg: Color, bg: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(CAP_L, Style::new().fg(bg)),
        Span::styled(text, Style::new().fg(fg).bg(bg)),
        Span::styled(CAP_R, Style::new().fg(bg)),
    ]
}

/// A picture over one cell that rounds a box: its corners, or a bubble's
/// tail; or over `ticks::CELLS`, a message's status.
#[derive(Clone, Copy)]
pub(crate) enum Deco {
    /// Which corners of the cell to round: bits TL, TR, BL, BR.
    Corners(u8, Color),
    /// WhatsApp's tail, outside the bubble's top corner; true on the left.
    Tail(bool, Color),
    /// The status ticks, as `ticks()` numbers them.
    Ticks(u8, Color),
    /// An audio's button over two cells: ▶, or ⏸ when true.
    Play(bool, Color),
    /// Half a row of a box, across that many cells, its two corners round:
    /// the upper half (the box ends there) when true, else the lower one.
    Edge(bool, u16, Color),
}

const TL: u8 = 1;
const TR: u8 = 2;
const BL: u8 = 4;
const BR: u8 = 8;

/// One cell's size in pixels, when the terminal shows pictures.
fn cell_px(app: &App) -> Option<(u32, u32)> {
    let s = app.picker.as_ref()?.font_size();
    Some((s.width as u32, s.height as u32))
}

fn rgb(c: Color) -> [u8; 3] {
    match c {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => [0, 0, 0],
    }
}

fn deco_image(d: Deco, w: u32, h: u32) -> DynamicImage {
    let (wf, hf) = (w as f32, h as f32);
    let (color, cover): ([u8; 3], Box<dyn Fn(f32, f32) -> f32>) = match d {
        Deco::Ticks(s, c) => return crate::ticks::picture(s, rgb(c), w * crate::ticks::CELLS as u32, h),
        Deco::Play(pause, c) => return play_picture(pause, rgb(c), w * 2, h),
        Deco::Edge(upper, cols, c) => return edge_picture(upper, rgb(c), w * cols as u32, h),
        Deco::Corners(mask, c) => {
            // As round as WhatsApp's 7.5px, within one cell.
            let tall = (mask & (TL | BL) == TL | BL) || (mask & (TR | BR) == TR | BR);
            let r = wf.min(if tall { hf / 2.0 } else { hf });
            // (center, and which side of it rounds: left, top).
            let corners: Vec<(f32, f32, bool, bool)> =
                [(TL, r, r, true, true), (TR, wf - r, r, false, true), (BL, r, hf - r, true, false), (BR, wf - r, hf - r, false, false)]
                    .into_iter()
                    .filter(|c| mask & c.0 != 0)
                    .map(|(_, x, y, l, t)| (x, y, l, t))
                    .collect();
            (
                rgb(c),
                Box::new(move |x, y| {
                    corners.iter().fold(1.0f32, |acc, &(cx, cy, left, top)| {
                        // Only the quarter beyond the center rounds.
                        let out_x = if left { x < cx } else { x > cx };
                        let out_y = if top { y < cy } else { y > cy };
                        if out_x && out_y {
                            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                            acc.min((r - d + 0.5).clamp(0.0, 1.0))
                        } else {
                            acc
                        }
                    })
                }),
            )
        }
        Deco::Tail(left, c) => {
            // A triangle: the top edge carries on out, the slant comes back
            // to the bubble a little over half a row down.
            let th = hf * 0.55;
            (
                rgb(c),
                Box::new(move |x, y| {
                    let edge = wf * (y / th);
                    let inside = if left { x - edge } else { (wf - edge) - x };
                    if y > th { 0.0 } else { (inside + 0.5).clamp(0.0, 1.0) }
                }),
            )
        }
    };
    let mut img = RgbaImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let a = cover(x as f32 + 0.5, y as f32 + 0.5);
        *p = Rgba([color[0], color[1], color[2], (255.0 * a) as u8]);
    }
    DynamicImage::ImageRgba8(img)
}

/// Half a row filled, the upper or the lower half, its ends rounded as the
/// cells' corners are.
fn edge_picture(upper: bool, color: [u8; 3], w: u32, h: u32) -> DynamicImage {
    let (wf, hh) = (w as f32, h as f32 / 2.0);
    let r = hh.min(wf / 2.0);
    let mut img = RgbaImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        // How far into the box from where it ends, mid-row.
        let depth = if upper { hh - py } else { py - hh };
        let cx = px.clamp(r, wf - r);
        let a = if depth >= r || cx == px {
            (depth + 0.5).clamp(0.0, 1.0)
        } else {
            let d = ((px - cx).powi(2) + (depth - r).powi(2)).sqrt();
            (r - d + 0.5).clamp(0.0, 1.0)
        };
        *p = Rgba([color[0], color[1], color[2], (255.0 * a) as u8]);
    }
    DynamicImage::ImageRgba8(img)
}

/// A pill over three rows of `h`, from the middle of the first to the
/// middle of the last, its ends half circles.
fn field_picture(color: [u8; 3], w: u32, h: u32) -> RgbaImage {
    let (wf, hf) = (w as f32, h as f32);
    let r = hf.min(wf / 2.0);
    let mut img = RgbaImage::new(w, h * 3);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        // How far from its spine, a segment across the middle row.
        let cx = px.clamp(r, wf - r);
        let d = ((px - cx).powi(2) + (py - hf * 1.5).powi(2)).sqrt();
        *p = Rgba([color[0], color[1], color[2], (255.0 * (r - d + 0.5).clamp(0.0, 1.0)) as u8]);
    }
    img
}

/// The search field's pill with pictures, `cols` cells from `x`: the half
/// rows over and under its text's row `y`, and the `cap` cells of each round
/// end in that row; the text's own cells fill the rest in its color.
fn draw_tall_field(f: &mut Frame, app: &mut App, x: u16, y: u16, cols: u16, cap: u16) {
    let (Some((w, h)), Some(picker)) = (cell_px(app), app.picker.as_ref()) else { return };
    if y == 0 || cols < 2 * cap {
        return;
    }
    // (row of the picture, first cell, cells across, row on screen).
    let pieces = [(0u32, 0u16, cols, y - 1), (1, 0, cap, y), (1, cols - cap, cap, y), (2, 0, cols, y + 1)];
    for (row, col, n, at) in pieces {
        let key = (format!("field-{row}-{col}-{cols}-{:?}", rgb(FIELD)), n, 1);
        if !app.protocols.contains_key(&key) {
            let pill = field_picture(rgb(FIELD), cols as u32 * w, h);
            let piece = image::imageops::crop_imm(&pill, col as u32 * w, row * h, n as u32 * w, h).to_image();
            let Ok(p) = picker.new_protocol(DynamicImage::ImageRgba8(piece), Rect::new(0, 0, n, 1).into(), Resize::Fit(None)) else { continue };
            app.protocols.insert(key.clone(), p);
        }
        f.render_widget(Image::new(&app.protocols[&key]), Rect::new(x + col, at, n, 1));
    }
}

/// WhatsApp's play triangle, or the two bars of pause, half a row tall.
fn play_picture(pause: bool, color: [u8; 3], w: u32, h: u32) -> DynamicImage {
    let (wf, hf) = (w as f32, h as f32);
    let th = (hf * 0.5).min(wf * 0.8);
    let tw = th * 0.85;
    let (x0, cy) = ((wf - tw) / 2.0, hf / 2.0);
    let mut img = RgbaImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (px, py) = (x as f32 + 0.5 - x0, y as f32 + 0.5);
        let dy = (py - cy).abs();
        let inside = if pause {
            // Each bar a third of the width, a gap between.
            let bar = tw * 0.34;
            let dx = px.min(bar - px).max((px - (tw - bar)).min(tw - px));
            dx.min(th / 2.0 - dy)
        } else {
            px.min(th / 2.0 * (1.0 - px / tw) - dy)
        };
        *p = Rgba([color[0], color[1], color[2], (255.0 * (inside + 0.5).clamp(0.0, 1.0)) as u8]);
    }
    DynamicImage::ImageRgba8(img)
}

/// The button down to the newest message, as WhatsApp Web's: a round
/// one with a chevron, over `r`. False without pictures.
fn draw_newest_btn(f: &mut Frame, app: &mut App, r: Rect) -> bool {
    let (Some((w, h)), Some(picker)) = (cell_px(app), app.picker.as_ref()) else { return false };
    let key = ("newest".to_string(), r.width, r.height);
    if !app.protocols.contains_key(&key) {
        let (pw, ph) = (w * r.width as u32, h * r.height as u32);
        let (wf, hf) = (pw as f32, ph as f32);
        let (cx, cy) = (wf / 2.0, hf / 2.0);
        let rad = wf.min(hf) / 2.0 - 1.0;
        // The chevron: two strokes meeting below the middle.
        let (a, b, t) = (rad * 0.32, rad * 0.16, (rad * 0.075).max(1.0));
        let seg = |px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32| {
            let (dx, dy) = (x1 - x0, y1 - y0);
            let k = (((px - x0) * dx + (py - y0) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            ((px - x0 - k * dx).powi(2) + (py - y0 - k * dy).powi(2)).sqrt()
        };
        let (bg, fg) = (rgb(RULE_IN), rgb(META_IN));
        let mut img = RgbaImage::new(pw, ph);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let disc = (rad - ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
            let d = seg(px, py, cx - a, cy - b, cx, cy + b).min(seg(px, py, cx, cy + b, cx + a, cy - b));
            let mark = (t - d + 0.5).clamp(0.0, 1.0);
            let mix = |i: usize| (bg[i] as f32 + (fg[i] as f32 - bg[i] as f32) * mark) as u8;
            *p = Rgba([mix(0), mix(1), mix(2), (255.0 * disc) as u8]);
        }
        let Ok(p) = picker.new_protocol(DynamicImage::ImageRgba8(img), Rect::new(0, 0, r.width, r.height).into(), Resize::Fit(None)) else {
            return false;
        };
        app.protocols.insert(key.clone(), p);
    }
    // No text under it shows around the circle; the cells keep their
    // background, a bubble's included.
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            f.buffer_mut()[(x, y)].set_symbol(" ");
        }
    }
    f.render_widget(Image::new(&app.protocols[&key]), r);
    true
}

pub(crate) fn draw_deco(f: &mut Frame, app: &mut App, d: Deco, x: u16, y: u16) {
    let (Some((w, h)), Some(picker)) = (cell_px(app), app.picker.as_ref()) else { return };
    let (key, cols) = match d {
        Deco::Corners(m, c) => (format!("corner-{m}-{:?}", rgb(c)), 1),
        Deco::Tail(l, c) => (format!("tail-{l}-{:?}", rgb(c)), 1),
        Deco::Ticks(s, c) => (format!("ticks-{s}-{:?}", rgb(c)), crate::ticks::CELLS),
        Deco::Play(p, c) => (format!("play-{p}-{:?}", rgb(c)), 2),
        Deco::Edge(u, n, c) => (format!("edge-{u}-{:?}", rgb(c)), n),
    };
    let key = (key, cols, 1);
    if !app.protocols.contains_key(&key) {
        let Ok(p) = picker.new_protocol(deco_image(d, w, h), Rect::new(0, 0, cols, 1).into(), Resize::Fit(None)) else { return };
        app.protocols.insert(key.clone(), p);
    }
    f.render_widget(Image::new(&app.protocols[&key]), Rect::new(x, y, cols, 1));
}

/// The corners of a box of `rows` rows whose first and last cells are at
/// `x` and `x + w - 1` of `top`: where each picture goes, as (row, col, deco).
pub(crate) fn box_corners(top: usize, rows: usize, x: usize, w: usize, color: Color, square: u8) -> Vec<(usize, usize, Deco)> {
    let last = top + rows - 1;
    let mut cells: Vec<(usize, usize, u8)> = Vec::new();
    let mut add = |row: usize, col: usize, bit: u8| {
        if square & bit != 0 {
            return;
        }
        match cells.iter_mut().find(|(r, c, _)| *r == row && *c == col) {
            Some(cell) => cell.2 |= bit,
            None => cells.push((row, col, bit)),
        }
    };
    add(top, x, TL);
    add(top, x + w - 1, TR);
    add(last, x, BL);
    add(last, x + w - 1, BR);
    cells.into_iter().map(|(r, c, m)| (r, c, Deco::Corners(m, color))).collect()
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let cached = !app.chats.is_empty() && matches!(app.phase, Phase::Starting | Phase::Connecting);
    if app.phase == Phase::Ready || cached {
        // The media viewer covers it all.
        if crate::viewer::draw(f, app) {
            return;
        }
        return main_view(f, app, area);
    }
    match &app.phase {
        Phase::Qr(code) => {
            let code = code.clone();
            qr_view(f, app, &code, area)
        }
        Phase::LoggedOut if app.account.signing_out => center(f, area, vec![Line::from("Aparelho desconectado. Gerando um QR novo…")]),
        Phase::LoggedOut => center(f, area, vec![Line::from("Sessão encerrada no celular. Gerando um QR novo…")]),
        Phase::Dead(why) => center(
            f,
            area,
            vec![Line::from(why.clone()).red(), Line::default(), Line::from("r reinicia · q sai").style(dim())],
        ),
        _ => center(f, area, vec![Line::from(vec![Span::styled("WhatsApp TUI", Style::new().fg(green()).bold()), Span::raw("  conectando ao WhatsApp…")])]),
    }
}

fn center(f: &mut Frame, area: Rect, lines: Vec<Line>) {
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), Rect { y, height: h.min(area.height), ..area });
}

fn qr_view(f: &mut Frame, app: &mut App, code: &str, area: Rect) {
    let Ok(qr) = qrcode::QrCode::new(code.as_bytes()) else {
        return center(f, area, vec![Line::from("QR inválido")]);
    };
    const QUIET: usize = 2;
    let w = qr.width();
    let colors = qr.to_colors();
    let total = w + 2 * QUIET;
    let dark = |x: usize, y: usize| {
        x >= QUIET && y >= QUIET && x < w + QUIET && y < w + QUIET && colors[(y - QUIET) * w + (x - QUIET)] == qrcode::Color::Dark
    };
    let text = [
        Line::from("Escaneie com o WhatsApp do celular").bold(),
        Line::from("Configurações › Aparelhos conectados › Conectar um aparelho").style(dim()),
        Line::from("q sai").style(dim()),
    ];
    if qr_picture(f, app, code, total, &dark, area, &text) {
        return;
    }
    let rows = total.div_ceil(2);
    let text = [
        Line::from("Escaneie com o WhatsApp do celular").bold(),
        Line::from("Configurações › Aparelhos conectados › Conectar um aparelho").style(dim()),
        Line::from("q sai").style(dim()),
    ];
    if (area.width as usize) < total || (area.height as usize) < rows + text.len() + 1 {
        return center(f, area, vec![Line::from(format!("Aumente o terminal para mostrar o QR (precisa de {total}×{}).", rows + 4))]);
    }
    // Two modules per cell, in the terminal's own colors: its text's for the
    // light ones, its background for the dark.
    let glyph = |up: bool, down: bool| match (!up, !down) {
        (true, true) => "█",
        (true, false) => "▀",
        (false, true) => "▄",
        (false, false) => " ",
    };
    let mut lines: Vec<Line> = (0..total)
        .step_by(2)
        .map(|y| Line::from((0..total).map(|x| Span::raw(glyph(dark(x, y), dark(x, y + 1)))).collect::<Vec<_>>()))
        .collect();
    lines.push(Line::default());
    lines.extend(text);
    center(f, area, lines);
}

/// The QR code as a picture, small and sharp: a few pixels a module, in the
/// theme's text color (light) and background (dark), the text under it.
/// False without pictures in the terminal.
fn qr_picture(f: &mut Frame, app: &mut App, code: &str, total: usize, dark: &dyn Fn(usize, usize) -> bool, area: Rect, text: &[Line<'static>]) -> bool {
    let Some((cw, ch)) = cell_px(app) else { return false };
    let (light, back) = crate::theme::fg_bg().unwrap_or(([0xFF; 3], [0; 3]));
    // About a fifth of a row a module: small, and still well over what a
    // phone's camera needs.
    let m = (ch / 5).max(3);
    let side = total as u32 * m;
    let (cols, rows) = (side.div_ceil(cw) as u16, side.div_ceil(ch) as u16);
    let h = rows + 1 + text.len() as u16;
    if area.width < cols || area.height < h {
        return false;
    }
    let key = (format!("qr-{code}-{light:?}-{back:?}"), cols, rows);
    if !app.protocols.contains_key(&key) {
        // The cells' whole size, the code in the middle, the rest see-through.
        let (w, hp) = (cols as u32 * cw, rows as u32 * ch);
        let (ox, oy) = ((w - side) / 2, (hp - side) / 2);
        let mut img = RgbaImage::new(w, hp);
        for y in 0..side {
            for x in 0..side {
                let c = if dark((x / m) as usize, (y / m) as usize) { back } else { light };
                img.put_pixel(ox + x, oy + y, Rgba([c[0], c[1], c[2], 255]));
            }
        }
        let Some(picker) = app.picker.as_ref() else { return false };
        let Ok(p) = picker.new_protocol(DynamicImage::ImageRgba8(img), Rect::new(0, 0, cols, rows).into(), Resize::Fit(None)) else { return false };
        app.protocols.insert(key.clone(), p);
    }
    let y = area.y + (area.height - h) / 2;
    let x = area.x + (area.width - cols) / 2;
    f.render_widget(Image::new(&app.protocols[&key]), Rect::new(x, y, cols, rows));
    let r = Rect { y: y + rows + 1, height: text.len() as u16, ..area };
    f.render_widget(Paragraph::new(text.to_vec()).alignment(Alignment::Center), r);
    true
}

/// The avatar's size in cells, two rows tall and square in pixels; None
/// without a picture-capable terminal.
pub(crate) fn avatar_cells(app: &App) -> Option<(u16, u16)> {
    let p = app.picker.as_ref()?;
    let s = p.font_size();
    let cols = ((2.0 * s.height as f64 / s.width as f64).round() as u16).clamp(3, 6);
    Some((cols, 2))
}

/// Shrinking with a smooth filter (the default, nearest, is jagged); a
/// cheaper one for big pictures, so encoding them doesn't stall a frame.
pub(crate) fn fit(img: &DynamicImage) -> Resize {
    let big = img.width() as u64 * img.height() as u64 > 1_000_000;
    Resize::Fit(Some(if big { ratatui_image::FilterType::Triangle } else { ratatui_image::FilterType::CatmullRom }))
}

pub(crate) fn draw_avatar(f: &mut Frame, app: &mut App, jid: &str, group: bool, rect: Rect) {
    let Some(picker) = app.picker.as_ref() else { return };
    let key = (jid.to_string(), rect.width, rect.height);
    if !app.protocols.contains_key(&key) {
        let s = picker.font_size();
        let side = (rect.width as u32 * s.width as u32).min(rect.height as u32 * s.height as u32);
        let mut img = match app.avatars.get(jid) {
            _ if jid == "0@s.whatsapp.net" => crate::avatar::official(side),
            Some(Avatar::Ready(img)) => crate::avatar::round(img, side),
            _ => crate::avatar::silhouette(side, group),
        };
        // In a taller box it sits in the middle, the rest see-through.
        let tall = rect.height as u32 * s.height as u32;
        if tall > side {
            let mut canvas = RgbaImage::new(side, tall);
            image::imageops::overlay(&mut canvas, &img.to_rgba8(), 0, ((tall - side) / 2) as i64);
            img = DynamicImage::ImageRgba8(canvas);
        }
        let fit = crate::ui::fit(&img);
        let Ok(p) = picker.new_protocol(img, Rect::new(0, 0, rect.width, rect.height).into(), fit) else { return };
        app.protocols.insert(key.clone(), p);
    }
    f.render_widget(Image::new(&app.protocols[&key]), rect);
    app.want_avatar(jid);
}

fn main_view(f: &mut Frame, app: &mut App, area: Rect) {
    let body = area;
    let side_w = (body.width * 3 / 10).clamp(32, 48).min(body.width / 2);
    let [side, sep, conv] = Layout::horizontal([Constraint::Length(side_w), Constraint::Length(1), Constraint::Min(1)]).areas(body);
    // Your account at the foot of the chat list, as in slick.
    let [side, account] = Layout::vertical([Constraint::Min(1), Constraint::Length(crate::account::BUTTON_H)]).areas(side);
    sidebar(f, app, side);
    crate::account::button(f, app, account);
    let bar: Vec<Line> = (0..sep.height).map(|_| Line::from("│").style(dim())).collect();
    f.render_widget(Paragraph::new(bar), sep);
    // The info panel takes the right, as slick's profile does, and the
    // conversation stays beside it; too narrow for both, it covers it.
    app.panel_area = Rect::default();
    app.panel_close = Rect::default();
    app.panel_hits.clear();
    app.panel_week = Rect::default();
    app.panel_maps = Rect::default();
    app.msg_links.clear();
    app.audio_hits.clear();
    app.media_hits.clear();
    match app.profile.as_ref().map(|_| panel_width(conv.width)) {
        None => conversation(f, app, conv),
        Some(Some(pw)) => {
            let [main, panel] = Layout::horizontal([Constraint::Min(1), Constraint::Length(pw)]).areas(conv);
            conversation(f, app, main);
            profile_panel(f, app, panel, true);
        }
        Some(None) => {
            app.msgs_area = Rect::default();
            app.input_area = Rect::default();
            app.attach_btn = Rect::default();
            app.emoji_btn = Rect::default();
            app.header_area = Rect::default();
            profile_panel(f, app, conv, false);
        }
    }
    crate::react::draw(f, app);
    crate::msgmenu::draw(f, app);
    crate::emoji::draw(f, app);
    crate::emoji_panel::draw(f, app);
    crate::attach::draw_menu(f, app);
    crate::chatmenu::draw(f, app);
    crate::account::draw(f, app);
    // Last, so only what's still a link on screen becomes one for the terminal.
    let is_link = |c: &ratatui::buffer::Cell| c.modifier.contains(Modifier::UNDERLINED) && (c.fg == LINK_IN || c.fg == link_out());
    for (r, url) in &app.msg_links {
        crate::links::osc8(f.buffer_mut(), *r, url, is_link);
    }
    notice(f, app, conv);
}

/// The info panel's width: WhatsApp Web's share of the screen, while the
/// conversation keeps 40 columns; None when that leaves it too narrow.
fn panel_width(total: u16) -> Option<u16> {
    let w = (total * 2 / 5).clamp(PANEL, 64).min(total.saturating_sub(40));
    (w >= 36).then_some(w)
}

fn sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    // The search field, a pill across the column. With pictures it's a row
    // taller, half a row of it over the text and half under.
    let field_w = (area.width as usize).saturating_sub(4);
    let tall = cell_px(app).filter(|&(w, h)| field_w + 2 >= 2 * h.div_ceil(w) as usize + 8);
    // Each round end's cells in the text's row: a glyph, or a picture as
    // wide as the pill is round (a row, from its middle to its edge).
    let cap = tall.map_or(1, |(w, h)| h.div_ceil(w) as usize);
    let lift = u16::from(tall.is_some());
    let inside = (field_w + 2).saturating_sub(2 * cap);
    let searching = app.focus == Focus::Search;
    let (text, fg) = if app.search.is_empty() && !searching {
        ("Pesquisar ou começar uma nova conversa".to_string(), META_IN)
    } else {
        (app.search.clone(), TEXT)
    };
    let icon_fg = if searching { green() } else { META_IN };
    let text = truncate(&text, inside.saturating_sub(4));
    let pad = inside.saturating_sub(3 + text.width());
    let end = |glyph| if tall.is_some() { Span::raw(" ".repeat(cap)) } else { Span::styled(glyph, Style::new().fg(FIELD)) };
    let mut search = vec![Span::raw(" "), end(CAP_L)];
    search.push(Span::styled(format!("{SEARCH}  "), Style::new().fg(icon_fg).bg(FIELD)));
    search.push(Span::styled(text, Style::new().fg(fg).bg(FIELD)));
    search.push(Span::styled(" ".repeat(pad), Style::new().bg(FIELD)));
    search.push(end(CAP_R));

    // The chips: All, Unread, Favorites, Groups. Unread counts the list shown.
    let unread = app.chats.iter().filter(|c| c.unread > 0 && c.archived == app.archived).count();
    let chips = [
        (Filter::All, "Tudo".to_string()),
        (Filter::Unread, if unread > 0 { format!("Não lidas {unread}") } else { "Não lidas".into() }),
        (Filter::Favorites, "Favoritas".into()),
        (Filter::Groups, "Grupos".into()),
    ];
    let mut chip_spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    app.chips.clear();
    // Four of them: without the space inside the pills when the column is narrow.
    let tight = 1 + chips.iter().map(|(_, l)| l.width() + 5).sum::<usize>() > area.width as usize;
    for (filter, label) in chips {
        let (fg, bg) = if app.filter == filter { (chip_on_text(), chip_on()) } else { (META_IN, FIELD) };
        let text = if tight { label } else { format!(" {label} ") };
        let w = text.width() as u16 + 2;
        if x + w <= area.right() {
            app.chips.push((Rect::new(x, area.y + 2 + 2 * lift, w, 1), filter));
        }
        chip_spans.extend(pill(text, fg, bg));
        chip_spans.push(Span::raw(" "));
        x += w + 1;
    }

    // The search field on top, then a blank row before the chips; the tall
    // field's rows above and below its text are pictures.
    let text_y = area.y + lift;
    app.search_area = Rect::new(area.x + 1, area.y, field_w as u16, 1 + 2 * lift);
    let mut top = vec![Line::from(search), Line::default(), Line::from(chip_spans)];
    if tall.is_some() {
        top.insert(0, Line::default());
        top.insert(2, Line::default());
    }
    f.render_widget(Paragraph::new(top), area);
    if tall.is_some() {
        draw_tall_field(f, app, area.x + 1, text_y, field_w as u16 + 2, cap as u16);
    }
    if searching {
        let x = area.x + 1 + cap as u16 + 3 + app.search.width() as u16;
        f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(1 + cap as u16)), text_y));
    }

    let list = Rect { y: area.y + 4 + 2 * lift, height: area.height.saturating_sub(4 + 2 * lift), ..area };
    app.side_list = list;
    app.side_hits.clear();
    app.chat_menu.chevron = None;
    let cells = avatar_cells(app).filter(|_| app.settings.on(crate::config::PHOTOS));
    let items = app.items();
    if items.is_empty() {
        let msg = if app.filter == Filter::All && !app.archived { "Nenhuma conversa ainda" } else { "Nada encontrado" };
        f.render_widget(Paragraph::new(format!(" {msg}")).style(Style::new().fg(META_IN)), list);
        return;
    }
    let cur = app.cursor_index(&items);
    let rows = |from: usize, to: usize| items[from..to].iter().map(|i| i.height()).sum::<u16>();
    // Scrolled by rows, the cursor's kept on screen and the end not left short.
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

    // The highlight follows the keyboard: the cursor while choosing, the open
    // chat while writing.
    // Nothing's lit on the start screen until the keys move, as on WhatsApp Web;
    // a search lights its first result, the one Enter opens.
    let idle = app.cursor.is_none() && app.open.is_none() && app.search.trim().is_empty();
    let lit = if app.focus == Focus::Input || idle && app.focus == Focus::Sidebar {
        app.open.as_ref().map(|j| format!("c:{j}"))
    } else {
        items.get(cur).and_then(Item::key)
    };
    // A cell in from each side: the highlight's a rounded box, as on WhatsApp Web.
    let inner = Rect { x: list.x + 1, width: list.width.saturating_sub(2), ..list };
    let width = inner.width as usize;
    let avatar_w = cells.map(|(w, _)| w);
    let query = app.search.trim().to_string();
    let mut lines: Vec<Line> = Vec::new();
    let mut pictures: Vec<(u16, String, bool)> = Vec::new();
    let mut statuses: Vec<(u16, u16, u8)> = Vec::new();
    let mut lit_at = None;
    let mut y = list.y;
    for item in items.iter().skip(offset) {
        let h = item.height();
        if y + h > list.bottom() {
            break;
        }
        let key = item.key();
        let on = key.is_some() && key == lit;
        match item {
            Item::Header(t) => lines.push(Line::from(Span::styled(format!(" {t}"), Style::new().fg(green())))),
            Item::Info(t) => lines.push(Line::from(Span::styled(format!(" {t}"), Style::new().fg(META_IN)))),
            Item::Archived => {
                // How many archived chats wait, green while any does.
                let n = app.chats.iter().filter(|c| c.archived && c.unread > 0).count();
                let count = Span::styled(n.to_string(), Style::new().fg(if n > 0 { green() } else { META_IN }));
                lines.push(lead_row(crate::chatmenu::ARCHIVE, Span::styled("Arquivadas", Style::new().fg(TEXT)), Some(count), width, avatar_w, on));
            }
            Item::Back => lines.push(lead_row("←", Span::styled("Arquivadas", Style::new().fg(TEXT).bold()), None, width, avatar_w, on)),
            Item::Chat(c) => {
                lines.extend(chat_item(c, width, avatar_w, on, app.typing.contains_key(&c.jid), app.draft_line(&c.jid)));
                pictures.push((y, c.jid.clone(), c.group));
                statuses.push((y, h, c.status));
                // The chevron chat_item put by the time opens its menu.
                if on {
                    app.chat_menu.chevron = Some((Rect::new(inner.x + width.saturating_sub(3) as u16, y, 3, 1), c.jid.clone()));
                }
            }
            Item::Contact(c) => {
                lines.extend(contact_item(&c.name, &phone(&c.jid), Style::new().fg(META_IN), width, avatar_w, on));
                pictures.push((y, c.jid.clone(), false));
            }
            Item::Number(n) => {
                let (sub, style) = if n.exists {
                    ("Começar uma conversa", Style::new().fg(green()))
                } else {
                    ("Este número não está no WhatsApp", Style::new().fg(META_IN))
                };
                lines.extend(contact_item(&phone(&n.jid), sub, style, width, avatar_w, on));
                pictures.push((y, n.jid.clone(), false));
            }
            Item::Message(m) => {
                let chat = app.name_of(&m.chat).unwrap_or_else(|| phone(&m.chat));
                lines.extend(message_item(m, &display_name(&chat, &m.chat), &query, width, on));
            }
        }
        // The rest of its rows, air under the two lines.
        let bg = if on { Style::new().bg(SELECTED) } else { Style::new() };
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
    // Where chat_item kept cells for the ticks' picture.
    let mut tick_cells = Vec::new();
    for &(top, h, status) in &statuses {
        for (row, line) in lines.iter().enumerate().skip((top - list.y) as usize).take(h as usize) {
            if let Some(i) = line.spans.iter().position(|s| s.content == TICK_SLOT) {
                let col = row_width(&line.spans[..i]) as u16;
                tick_cells.push((inner.x + col, inner.y + row as u16, status));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
    // Pictures at hand: half a row of air above the lines and half below,
    // as WhatsApp pads the selected chat. The row of air under it gives its
    // upper half, the one over it (the chat above's) its lower half.
    if let Some((top, h)) = lit_at.filter(|_| app.picker.is_some() && width > 1) {
        let (x0, cols) = (inner.x, width as u16);
        let blank = |f: &mut Frame, y: u16| (x0..x0 + cols).all(|x| f.buffer_mut()[(x, y)].symbol() == " ");
        let below = top + h - 1;
        for x in x0..x0 + cols {
            f.buffer_mut()[(x, below)].set_bg(Color::Reset);
        }
        draw_deco(f, app, Deco::Edge(true, cols, SELECTED), x0, below);
        if top > inner.y && blank(f, top - 1) {
            draw_deco(f, app, Deco::Edge(false, cols, SELECTED), x0, top - 1);
        }
    }
    if let Some((w, _)) = cells {
        for (y, jid, group) in pictures {
            // On the two lines of text, centered with them; the air row stays below.
            draw_avatar(f, app, &jid, group, Rect::new(inner.x + 1, y, w, 2));
        }
    }
    for (x, y, status) in tick_cells {
        draw_deco(f, app, Deco::Ticks(status, ticks(status).1), x, y);
    }
}

/// A chat named by its number reads like WhatsApp writes numbers.
pub(crate) fn display_name(name: &str, jid: &str) -> String {
    let digits = name.strip_prefix('+').is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
    if digits {
        phone(jid)
    } else {
        name.to_string()
    }
}

/// A one-line row with an icon where the pictures go, its text and, at the
/// right, `right`: "Arquivadas" and its count, "← Arquivadas".
fn lead_row(icon: &str, text: Span<'static>, right: Option<Span<'static>>, width: usize, avatar: Option<u16>, lit: bool) -> Line<'static> {
    let bg = if lit { Style::new().bg(SELECTED) } else { Style::new() };
    // The icon in the middle of the picture's column.
    let (before, after) = match avatar {
        Some(w) => (1 + (w as usize).saturating_sub(1) / 2, (w as usize + 2).saturating_sub(2 + (w as usize).saturating_sub(1) / 2)),
        None => (1, 2),
    };
    let mut spans = vec![Span::raw(" ".repeat(before)), Span::styled(icon.to_string(), Style::new().fg(META_IN)), Span::raw(" ".repeat(after.max(1))), text];
    let right_w = right.as_ref().map_or(0, |r| r.content.width());
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used + right_w + 1))));
    spans.extend(right);
    spans.push(Span::raw(" "));
    Line::from(spans).style(bg)
}

/// Room on the left for the picture, drawn over these cells afterwards.
fn lead(avatar: Option<u16>) -> String {
    match avatar {
        Some(w) => " ".repeat(w as usize + 2),
        None => " ".into(),
    }
}

/// A contact (or a typed number) found by the search: a name and a line under it.
fn contact_item(title: &str, sub: &str, sub_style: Style, width: usize, avatar: Option<u16>, lit: bool) -> [Line<'static>; 2] {
    let bg = if lit { Style::new().bg(SELECTED) } else { Style::new() };
    let lead = lead(avatar);
    let inner = width.saturating_sub(lead.width() + 1);
    let pad = |s: &str| " ".repeat(inner.saturating_sub(s.width()) + 1);
    let title = truncate(title, inner);
    let sub = truncate(sub, inner);
    [
        Line::from(vec![Span::raw(lead.clone()), Span::styled(title.clone(), Style::new().fg(TEXT)), Span::raw(pad(&title))]).style(bg),
        Line::from(vec![Span::raw(lead), Span::styled(sub.clone(), sub_style), Span::raw(pad(&sub))]).style(bg),
    ]
}

/// A message the search found: its chat and date, then the text around the
/// match, the match in green.
fn message_item(m: &Msg, chat: &str, query: &str, width: usize, lit: bool) -> [Line<'static>; 2] {
    let bg = if lit { Style::new().bg(SELECTED) } else { Style::new() };
    let inner = width.saturating_sub(3);
    let date = short_time(m.ts);
    let name = truncate(chat, inner.saturating_sub(date.width() + 1));
    let gap = inner.saturating_sub(name.width() + date.width());
    let top = Line::from(vec![
        Span::raw("  "),
        Span::styled(name, Style::new().fg(TEXT)),
        Span::raw(" ".repeat(gap)),
        Span::styled(date, Style::new().fg(META_IN)),
        Span::raw(" "),
    ])
    .style(bg);

    let mut spans = vec![Span::raw("  ")];
    let mut room = inner;
    if m.from_me {
        let (t, color) = ticks(m.status);
        spans.push(Span::styled(format!("{t} "), Style::new().fg(color)));
        room = room.saturating_sub(t.width() + 1);
    }
    // From a bit before the match, so it shows.
    let text: Vec<char> = m.text.replace('\n', " ").chars().collect();
    let lower: Vec<char> = text.iter().flat_map(|c| c.to_lowercase()).collect();
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let at = (lower.len() == text.len() && !q.is_empty()).then(|| lower.windows(q.len()).position(|w| w == q.as_slice())).flatten();
    let start = at.map_or(0, |i| i.saturating_sub(12));
    let mut shown: String = text[start..].iter().collect();
    if start > 0 {
        shown.insert(0, '…');
    }
    let shown = truncate(&shown, room);
    let plain = Style::new().fg(META_IN);
    match at {
        Some(i) => {
            let rel = i - start + usize::from(start > 0);
            let chars: Vec<char> = shown.chars().collect();
            let end = (rel + q.len()).min(chars.len());
            let rel = rel.min(end);
            spans.push(Span::styled(chars[..rel].iter().collect::<String>(), plain));
            spans.push(Span::styled(chars[rel..end].iter().collect::<String>(), Style::new().fg(green()).bold()));
            spans.push(Span::styled(chars[end..].iter().collect::<String>(), plain));
        }
        None => spans.push(Span::styled(shown, plain)),
    }
    let used: usize = spans.iter().map(|s| s.content.width()).sum();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    [top, Line::from(spans).style(bg)]
}

/// Blank cells kept for the ticks' picture, which goes over them; braille's
/// blank so the sidebar can find them in a chat's preview.
const TICK_SLOT: &str = "\u{2800}\u{2800}";

fn ticks(status: u8) -> (&'static str, Color) {
    match status {
        0 => ("◷", META_IN),
        1 => ("✓", META_IN),
        2 => ("✓✓", META_IN),
        _ => ("✓✓", READ_BLUE),
    }
}

fn chat_item(c: &Chat, width: usize, avatar: Option<u16>, lit: bool, typing: bool, draft: Option<String>) -> [Line<'static>; 2] {
    let bg = if lit { Style::new().bg(SELECTED) } else { Style::new() };
    let lead = lead(avatar);
    let unread = c.unread > 0;
    let time = short_time(c.last_ts);
    let inner = width.saturating_sub(lead.width() + 1);

    let name_style = if unread { Style::new().fg(TEXT).bold() } else { Style::new().fg(TEXT) };
    // The lit row's chevron after the time, as WhatsApp Web's on hover: its menu.
    let chevron = if lit { format!(" {DOWN}") } else { String::new() };
    let name = truncate(&display_name(&c.name, &c.jid), inner.saturating_sub(time.width() + chevron.width() + 1));
    let gap = inner.saturating_sub(name.width() + time.width() + chevron.width());
    let time_style = if unread && !c.muted { Style::new().fg(green()) } else { Style::new().fg(META_IN) };
    let top = Line::from(vec![
        Span::raw(lead.clone()),
        Span::styled(name, name_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(time, time_style),
        Span::styled(chevron, Style::new().fg(META_IN)),
        Span::raw(" "),
    ])
    .style(bg);

    // At the preview's end, as on WhatsApp Web: muted, pinned, the count.
    let mut right: Vec<Span> = Vec::new();
    let icon = Style::new().fg(META_IN);
    for (on, glyph) in [(c.muted, crate::chatmenu::MUTED), (c.pinned, crate::chatmenu::PINNED)] {
        if on {
            if !right.is_empty() {
                right.push(Span::raw(" "));
            }
            right.push(Span::styled(glyph, icon));
        }
    }
    if unread {
        if !right.is_empty() {
            right.push(Span::raw(" "));
        }
        let badge = if c.muted { META_IN } else { green() };
        right.extend(pill(c.unread.to_string(), Color::Black, badge));
    }
    let right_w: usize = right.iter().map(|s| s.content.width()).sum();
    let mut left: Vec<Span> = vec![Span::raw(lead)];
    let room = inner.saturating_sub(right_w + 1);
    if typing {
        left.push(Span::styled(truncate("digitando…", room), Style::new().fg(green())));
    } else if let Some(d) = draft {
        // What was left unsent, in place of the last message, as on the Web.
        let label = "Rascunho: ";
        left.push(Span::styled(truncate(label, room), Style::new().fg(green()).bold()));
        left.push(Span::styled(truncate(&d, room.saturating_sub(label.width())), Style::new().fg(META_IN)));
    } else {
        let mut room = room;
        if c.from_me && !c.preview.is_empty() {
            // An avatar means pictures: the ticks are drawn over blanks.
            let (t, color) = if avatar.is_some() { (TICK_SLOT, META_IN) } else { ticks(c.status) };
            left.push(Span::styled(t, Style::new().fg(color)));
            left.push(Span::raw(" "));
            room = room.saturating_sub(t.width() + 1);
        }
        let style = if unread { Style::new().fg(TEXT) } else { Style::new().fg(META_IN) };
        left.extend(crate::markup::line(&c.preview, room, style));
    }
    let used: usize = left.iter().map(|s| s.content.width()).sum();
    left.push(Span::raw(" ".repeat((width.saturating_sub(1)).saturating_sub(used + right_w))));
    left.extend(right);
    left.push(Span::raw(" "));
    [top, Line::from(left).style(bg)]
}

/// A sticker's height in rows: about WhatsApp Web's 190 px.
const STICKER_ROWS: u16 = 7;

/// How long a message gone to stays lit, the last of it fading.
const FLASH_SECS: f32 = 2.5;
const FADE_SECS: f32 = 1.0;

fn conversation(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(jid) = app.open.clone() else {
        app.msgs_area = Rect::default();
        app.input_area = Rect::default();
        app.attach_btn = Rect::default();
        app.emoji_btn = Rect::default();
        return center(
            f,
            area,
            vec![
                Line::from("WhatsApp TUI").fg(green()).bold(),
                Line::default(),
                Line::from("Escolha uma conversa à esquerda").style(dim()),
                Line::from("↑↓ navega · Enter abre · / busca · ? atalhos").style(dim()),
            ],
        );
    };
    let chat = app.chat(&jid).cloned();
    let group = jid.ends_with("@g.us");
    let read_only = chat.as_ref().map(|c| c.read_only.clone()).unwrap_or_default();

    let selecting = app.msg_menu.selecting(&jid);
    let editing = if app.msg_menu.editing(&jid) { 2 } else { 0 };
    let input_h = if selecting {
        3
    } else if read_only.is_empty() {
        input_lines(&app.input, area.width.saturating_sub(8) as usize).len().clamp(1, 6) as u16 + 4 + crate::attach::rows(app) + editing
    } else {
        3
    };
    let [header, msgs_area, input] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1), Constraint::Length(input_h)]).areas(area);
    app.msgs_area = msgs_area;
    // Selecting, the bubbles move over for a column of checkboxes.
    let full_msgs = msgs_area;
    let msgs_area = if selecting {
        let cw = crate::msgmenu::CHECK_W.min(msgs_area.width / 2);
        Rect { x: msgs_area.x + cw, width: msgs_area.width - cw, ..msgs_area }
    } else {
        msgs_area
    };
    app.input_area = input;
    app.attach_btn = Rect::default();
    app.emoji_btn = Rect::default();
    app.header_area = Rect { height: 2, ..header };

    // Header: the picture, the name, and who's typing or what it is; a click
    // opens the info panel.
    let cells = avatar_cells(app);
    let indent = cells.map_or(1, |(w, _)| w + 3);
    let name = display_name(&app.name_of(&jid).unwrap_or_else(|| phone(&jid)), &jid);
    let number = phone(&jid);
    let sub = match app.typing.get(&jid) {
        Some((who, _)) if group => Span::styled(format!("{who} está digitando…"), Style::new().fg(green())),
        Some(_) => Span::styled("digitando…", Style::new().fg(green())),
        None if jid == "0@s.whatsapp.net" => Span::styled("Conta oficial do WhatsApp", Style::new().fg(META_IN)),
        None if group => Span::styled("clique para ver os dados do grupo", Style::new().fg(META_IN)),
        None if name == number => Span::styled("clique para ver os dados do contato", Style::new().fg(META_IN)),
        None => Span::styled(number, Style::new().fg(META_IN)),
    };
    // The check after the name, as WhatsApp Web marks itself and verified businesses.
    let verified = jid == "0@s.whatsapp.net" || chat.as_ref().is_some_and(|c| c.verified);
    let check = if verified { Span::styled(format!(" {CHECK}"), Style::new().fg(CHECK_BLUE)) } else { Span::raw("") };
    let pad = " ".repeat(indent as usize);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::raw(pad.clone()), Span::styled(name, Style::new().fg(TEXT).bold()), check]),
            Line::from(vec![Span::raw(pad), sub]),
            Line::from("─".repeat(header.width as usize)).style(dim()),
        ]),
        header,
    );
    if let Some((w, h)) = cells {
        draw_avatar(f, app, &jid, group, Rect::new(header.x + 1, header.y, w, h));
    }

    // Messages, bottom-aligned, `scroll` lines up from the newest.
    let empty = Vec::new();
    let msgs = app.msgs.get(&jid).unwrap_or(&empty);
    let loaded = app.msgs.contains_key(&jid);
    let now = app.player.now.as_ref().filter(|n| n.chat == jid);
    // In a group, the sender's picture by their first bubble, as WhatsApp Web shows it.
    let faces = cells.filter(|_| group && app.settings.on(crate::config::PHOTOS)).map(|(w, _)| w);
    let (lines, decos, starts, bigs, links, audios, pics, senders, spots) = message_lines(msgs, msgs_area.width as usize, group, faces, cell_px(app), now);
    let h = msgs_area.height as usize;
    // From a search result: that message in the middle of the screen.
    if let Some(id) = app.jump.clone().filter(|_| loaded) {
        if let Some(&(_, row)) = starts.iter().find(|(m, _)| *m == id) {
            app.scroll = lines.len().saturating_sub(row + h / 2);
        }
        app.jump = None;
    }
    let max_scroll = lines.len().saturating_sub(h);
    app.scroll = app.scroll.min(max_scroll);
    let end = lines.len() - app.scroll;
    let start = end.saturating_sub(h);
    let shown: Vec<Line> = lines[start..end].to_vec();
    let top_pad = h.saturating_sub(shown.len()) as u16;
    f.render_widget(Paragraph::new(shown), Rect { y: msgs_area.y + top_pad, height: msgs_area.height - top_pad, ..msgs_area });
    for (row, col, d) in decos {
        if row >= start && row < end && col < msgs_area.width as usize {
            draw_deco(f, app, d, msgs_area.x + col as u16, msgs_area.y + top_pad + (row - start) as u16);
        }
    }
    for (row, col, w, url) in links {
        if row >= start && row < end && col < msgs_area.width as usize {
            let w = w.min(msgs_area.width as usize - col) as u16;
            app.msg_links.push((Rect::new(msgs_area.x + col as u16, msgs_area.y + top_pad + (row - start) as u16, w, 1), url));
        }
    }
    // An audio's icon only whole, as the big emojis; its button and track
    // take clicks on any row shown.
    let y_of = |row: usize| msgs_area.y + top_pad + (row - start) as u16;
    for a in audios {
        if a.row < start || a.row >= end || a.track + a.track_w > msgs_area.width as usize {
            continue;
        }
        let y = y_of(a.row);
        if a.row + 1 < end && cell_px(app).is_some() {
            let rect = Rect::new(msgs_area.x + a.col as u16, y, a.icon_w as u16, 2);
            if a.ptt {
                draw_avatar(f, app, &a.sender, false, rect);
            } else {
                draw_headphones(f, app, rect);
            }
        }
        if a.playable {
            app.audio_hits.push(AudioHit {
                play: Rect::new(msgs_area.x + (a.col + a.icon_w + 1) as u16, y, 2, 1),
                track: Rect::new(msgs_area.x + a.track as u16, y, a.track_w as u16, 1),
                chat: jid.clone(),
                id: a.id,
                secs: a.secs,
            });
        }
    }
    // Photos and videos only whole, as the big emojis.
    for p in &pics {
        if p.row >= start && p.row + p.h <= end && p.col + p.w <= msgs_area.width as usize {
            let rect = Rect::new(msgs_area.x + p.col as u16, y_of(p.row), p.w as u16, p.h as u16);
            crate::media::draw(f, app, p, rect);
        }
    }
    // Where the bubbles are, for the reactions and the clicks on quotes.
    app.bubbles.clear();
    app.sender_hits.clear();
    for s in spots {
        if let Some((row, col, w)) = s.name.filter(|&(row, col, _)| row >= start && row < end && col < msgs_area.width as usize) {
            let w = w.min(msgs_area.width as usize - col) as u16;
            app.sender_hits.push((Rect::new(msgs_area.x + col as u16, y_of(row), w, 1), s.sender.clone()));
        }
        let (a, b) = (s.top.max(start), (s.top + s.rows).min(end));
        if a >= b || s.x >= msgs_area.width as usize {
            continue;
        }
        let w = s.w.min(msgs_area.width as usize - s.x) as u16;
        let rect = Rect::new(msgs_area.x + s.x as u16, y_of(a), w, (b - a) as u16);
        let quote = s.quote.and_then(|(q, n)| {
            let (qa, qb) = (q.max(start), (q + n).min(end));
            (qa < qb).then(|| Rect::new(rect.x + 1, y_of(qa), w.saturating_sub(3), (qb - qa) as u16))
        });
        app.bubbles.push(crate::react::Hit { rect, id: s.id, from_me: s.from_me, quote });
    }
    // The message gone to from a quote: its rows lit behind it, as WhatsApp
    // Web does, fading after a couple of seconds.
    if let Some((id, at)) = app.flash.clone() {
        let t = at.elapsed().as_secs_f32();
        match app.bubbles.iter().find(|h| h.id == id).map(|h| h.rect) {
            _ if t >= FLASH_SECS => app.flash = None,
            Some(r) => {
                let glow = crate::theme::glow(((FLASH_SECS - t) / FADE_SECS).min(1.0));
                for y in r.y..r.bottom() {
                    for x in msgs_area.left()..msgs_area.right() {
                        let cell = &mut f.buffer_mut()[(x, y)];
                        if cell.bg == Color::Reset {
                            cell.set_bg(glow);
                        }
                    }
                }
            }
            None => {}
        }
    }
    crate::msgmenu::draw_checks(f, app, full_msgs);
    crate::react::draw_quick(f, app);
    crate::msgmenu::draw_chevron(f, app);
    // The senders' pictures only whole, as the big emojis.
    if let Some(w) = faces {
        for (row, col, sender) in senders {
            if row >= start && row + 2 <= end {
                let rect = Rect::new(msgs_area.x + col as u16, y_of(row), w, 2);
                draw_avatar(f, app, &sender, false, rect);
                app.sender_hits.push((rect, sender));
            }
        }
    }
    // Big emojis only whole: a terminal picture can't be cut.
    for (row, col, (w, eh), text) in bigs {
        if row >= start && row + eh as usize <= end {
            crate::bigemoji::draw(f, app, &text, Rect::new(msgs_area.x + col as u16, msgs_area.y + top_pad + (row - start) as u16, w, eh));
        }
    }
    // Scrolled up: WhatsApp Web's round button with a chevron, down to the newest.
    app.newest_btn = Rect::default();
    if app.scroll > 0 {
        let r = Rect { x: msgs_area.right().saturating_sub(6), y: msgs_area.bottom().saturating_sub(3), width: 4, height: 2 };
        if !draw_newest_btn(f, app, r) {
            let label = format!(" {DOWN} ");
            let r = Rect { y: r.y + 1, width: label.width() as u16 + 2, height: 1, ..r };
            f.render_widget(Paragraph::new(Line::from(pill(label, META_IN, RULE_IN))), r);
        }
        app.newest_btn = r;
    }

    if selecting {
        crate::msgmenu::draw_select_bar(f, app, input);
    } else if read_only.is_empty() {
        let input = if editing > 0 {
            crate::msgmenu::draw_edit_strip(f, app, Rect { height: editing, ..input });
            Rect { y: input.y + editing, height: input.height - editing, ..input }
        } else {
            input
        };
        input_box(f, app, input);
    } else {
        // Like WhatsApp: no box, just why.
        let r = Rect { y: input.y + 1, height: 1, ..input };
        f.render_widget(Paragraph::new(read_only).style(Style::new().fg(META_IN)).alignment(Alignment::Center), r);
    }
}

/// The message box: a rounded field, as on WhatsApp Web, with a row of
/// padding inside and of margin outside, above and below.
fn input_box(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Input;
    let rounded = app.picker.is_some();
    let box_w = (area.width as usize).saturating_sub(2);
    // The clip and the smiley come before the text, as on WhatsApp Web.
    let width = box_w.saturating_sub(6 + ICONS_W);
    app.input_width = width;
    let x = area.x + 1;
    let rows: Vec<String> = if app.input.is_empty() { vec![String::new()] } else { input_lines(&app.input, width) };
    let (crow, ccol) = caret_pos(&app.input, app.caret, width);
    // The attached files' cards sit over the text, inside the field.
    let strip = crate::attach::rows(app);
    let h = (area.height as usize).saturating_sub(4 + strip as usize).max(1);
    let first = (crow + 1).saturating_sub(h);

    let field = Style::new().fg(TEXT).bg(FIELD);
    // The padding rows' end cells stay bare: the pictures round them.
    let edge = if rounded { Style::new() } else { field };
    let padding = Line::from(vec![Span::styled(" ", edge), Span::styled(" ".repeat(box_w.saturating_sub(2)), field), Span::styled(" ", edge)]);
    let shown: Vec<&String> = rows.iter().skip(first).take(h).collect();
    // The formatting shows as it's typed; each row's start in the text for
    // that: a row ends at the width, or at a newline it takes.
    let chars: Vec<char> = app.input.chars().collect();
    let looks = crate::markup::scan(&chars);
    let mut starts = Vec::with_capacity(rows.len());
    let mut at = 0;
    for r in &rows {
        starts.push(at);
        at += r.chars().count();
        if chars.get(at) == Some(&'\n') {
            at += 1;
        }
    }
    let n = shown.len();
    let mut lines = vec![Line::default(), padding.clone()];
    lines.extend((0..strip).map(|_| Line::from(Span::styled(" ".repeat(box_w), field))));
    let icon = Style::new().fg(META_IN).bg(FIELD);
    for (i, row) in shown.into_iter().enumerate() {
        let icons = if i == 0 {
            vec![Span::styled(format!("{CLIP}  "), icon), Span::styled(format!("{SMILEY}  "), icon)]
        } else {
            vec![Span::styled(" ".repeat(ICONS_W), field)]
        };
        let text = if app.input.is_empty() && i == 0 {
            let hint = if focused { "Digite uma mensagem" } else { "Digite uma mensagem (Tab)" };
            vec![Span::styled(hint, Style::new().fg(META_IN).bg(FIELD))]
        } else {
            crate::markup::typed_row(row, starts[first + i], &looks, field, CODE_IN, META_IN)
        };
        let pad = width.saturating_sub(row_width(&text));
        let mut spans = vec![Span::styled("   ", field)];
        spans.extend(icons);
        spans.extend(text);
        spans.push(Span::styled(" ".repeat(pad + 3), field));
        lines.push(Line::from(spans));
    }
    lines.push(padding);
    f.render_widget(Paragraph::new(lines), Rect { x, width: box_w as u16, ..area });
    if rounded {
        for (row, col, d) in box_corners(1, n + 2 + strip as usize, 0, box_w, FIELD, 0) {
            draw_deco(f, app, d, x + col as u16, area.y + row as u16);
        }
    }
    if strip > 0 {
        crate::attach::draw_strip(f, app, Rect::new(x + 3, area.y + 2, (box_w as u16).saturating_sub(6), strip - 1));
    }
    let text_y = area.y + 2 + strip;
    app.attach_btn = Rect::new(x + 2, text_y, 3, 1);
    app.emoji_btn = Rect::new(x + 5, text_y, 3, 1);
    if focused {
        f.set_cursor_position(Position::new(x + 3 + ICONS_W as u16 + ccol as u16, text_y + (crow - first) as u16));
    }
}

/// The message box breaks lines by width at any char: easy to put the caret on.
fn input_lines(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = vec![String::new()];
    let mut w = 0;
    for ch in s.chars() {
        if ch == '\n' {
            out.push(String::new());
            w = 0;
            continue;
        }
        let cw = ch.width().unwrap_or(0);
        if w + cw > width {
            out.push(String::new());
            w = 0;
        }
        out.last_mut().unwrap().push(ch);
        w += cw;
    }
    out
}

pub(crate) fn caret_pos(s: &str, caret: usize, width: usize) -> (usize, usize) {
    let width = width.max(1);
    let (mut row, mut col) = (0, 0);
    for ch in s.chars().take(caret) {
        if ch == '\n' {
            row += 1;
            col = 0;
            continue;
        }
        let cw = ch.width().unwrap_or(0);
        if col + cw > width {
            row += 1;
            col = 0;
        }
        col += cw;
    }
    if col >= width {
        (row + 1, 0)
    } else {
        (row, col)
    }
}

/// The color a group gives a member's name, theirs in every message and quote.
fn name_color(jid: &str) -> Color {
    NAMES[jid.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)) % NAMES.len()]
}

/// The conversation as bubbles, a blank row between them. With pictures
/// in the terminal their corners are rounded and the first of each run of
/// one sender's messages has WhatsApp's tail, on its side: the returned
/// (row, col, deco) say where those pictures go. A message of only emojis
/// goes without a bubble, big, as (row, col, size, text).
/// Then the links' pieces, a row each, as (row, col, width, url); then
/// the audio players; the photos and videos; last, in a group, the
/// senders' pictures by their first bubbles, as (row, col, jid); and where
/// each message is.
type Bubbles = (
    Vec<Line<'static>>,
    Vec<(usize, usize, Deco)>,
    Vec<(String, usize)>,
    Vec<(usize, usize, (u16, u16), String)>,
    Vec<(usize, usize, usize, String)>,
    Vec<AudioSpot>,
    Vec<crate::media::PicSpot>,
    Vec<(usize, usize, String)>,
    Vec<Spot>,
);

/// Where a message is, in rows and columns of the conversation: its bubble
/// (or big emojis) and its quote's rows, for the reactions and the clicks.
struct Spot {
    id: String,
    top: usize,
    rows: usize,
    x: usize,
    w: usize,
    from_me: bool,
    quote: Option<(usize, usize)>,
    /// A group member's name over it, as (row, col, width): a click opens their info.
    name: Option<(usize, usize, usize)>,
    sender: String,
}

/// An audio bubble's player: its first row and where its icon, button and
/// track are, for the pictures and the clicks.
struct AudioSpot {
    row: usize,
    /// The icon's first column; the button's two cells come a blank after it.
    col: usize,
    icon_w: usize,
    track: usize,
    track_w: usize,
    id: String,
    sender: String,
    ptt: bool,
    playable: bool,
    secs: u32,
}

const ORANGE: Color = Color::Rgb(0xFF, 0x9F, 0x1C);
const PLAY: Color = Color::Rgb(0xC5, 0xCD, 0xD1);
const MIC: &str = "\u{f130}";
const HEADPHONES: &str = "\u{f025}";
const PLAY_GLYPH: &str = "\u{f04b}";
const PAUSE_GLYPH: &str = "\u{f04c}";
const SPIN: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

fn clock_secs(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// An audio's two rows, as WhatsApp Web lays it out: the icon (a picture
/// over blanks, with pictures), the button and the track; under the track,
/// where it is or how long it lasts. Also the icon's width and the track's
/// column and width.
fn audio_rows(a: &Audio, now: Option<&Now>, from_me: bool, bg: Color, meta_fg: Color, cell: Option<(u32, u32)>, inner: usize) -> ([Vec<Span<'static>>; 2], usize, usize, usize) {
    let w = 38.min(inner.saturating_sub(2));
    // As wide as a two-row avatar.
    let icon_w = cell.map_or(1, |(cw, ch)| ((2.0 * ch as f64 / cw as f64).round() as usize).clamp(3, 6));
    let track_w = w.saturating_sub(icon_w + 4).max(3);
    let on = |c: Color| Style::new().fg(c).bg(bg);
    let blank = |n: usize| Span::styled(" ".repeat(n), Style::new().bg(bg));
    let icon = match cell {
        Some(_) => blank(icon_w),
        None if a.ptt => Span::styled(MIC, on(green())),
        None => Span::styled(HEADPHONES, on(ORANGE)),
    };
    let state = now.map(|n| n.state);
    let button = match state {
        Some(State::Loading) => {
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
            Span::styled(format!("{} ", SPIN[(ms / 100) as usize % SPIN.len()]), on(PLAY))
        }
        _ if cell.is_some() => blank(2),
        Some(State::Playing) => Span::styled(format!("{PAUSE_GLYPH} "), on(PLAY)),
        _ => Span::styled(format!("{PLAY_GLYPH} "), on(if a.playable { PLAY } else { meta_fg })),
    };
    // mpv's length once it's playing: the message's is rounded, or missing.
    let dur = now.map(|n| n.dur).filter(|&d| d > 0.0).unwrap_or(a.secs as f64);
    let pos = now.filter(|n| n.state != State::Loading).map(|n| n.pos);
    let frac = match pos {
        Some(p) if dur > 0.0 => (p / dur).clamp(0.0, 1.0),
        _ => 0.0,
    };
    let knob = (frac * (track_w - 1) as f64).round() as usize;
    let accent = if from_me { link_out() } else { LINK_IN };
    // A cell of padding on each side: a player wants more room than text.
    let mut top = vec![blank(1), icon, blank(1), button, blank(1)];
    top.push(Span::styled("━".repeat(knob), on(accent)));
    top.push(Span::styled("●", on(if a.playable { accent } else { meta_fg })));
    top.push(Span::styled("─".repeat(track_w - 1 - knob), on(meta_fg)));
    top.push(blank(1));
    let time = match pos {
        _ if !a.playable => "indisponível".to_string(),
        Some(p) => clock_secs(p),
        None => clock_secs(dur),
    };
    let under = vec![blank(icon_w + 5), Span::styled(time, on(meta_fg))];
    ([top, under], icon_w, icon_w + 5, track_w)
}

/// Also where each message starts, by id. `cell` is a cell's pixels, when
/// the terminal shows pictures; `now`, the open chat's audio playing.
/// `faces`: the width of the pictures by a group's bubbles, None without them.
fn message_lines(msgs: &[Msg], width: usize, group: bool, faces: Option<u16>, cell: Option<(u32, u32)>, now: Option<&Now>) -> Bubbles {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut decos = Vec::new();
    let mut starts = Vec::new();
    let mut bigs = Vec::new();
    let mut link_cells = Vec::new();
    let mut audios = Vec::new();
    let mut pics = Vec::new();
    let mut senders = Vec::new();
    let mut spots = Vec::new();
    let rounded = cell.is_some();
    // Others' bubbles start after the pictures' column and a blank.
    let face_w = faces.map_or(0, |w| w as usize + 1);
    let left = 2 + face_w;
    if width < 16 + face_w {
        return (out, decos, starts, bigs, link_cells, audios, pics, senders, spots);
    }
    let outer = (width * 3 / 4).clamp(16, 90).min(width - 4 - face_w);
    // A cell of air on the left, two on the right, where the text would
    // otherwise run into the edge.
    let inner = outer - 3;
    let mut prev_day = None;
    let mut prev_sender: Option<&str> = None;
    for m in msgs {
        let t = local(m.ts);
        let day = t.date_naive();
        if prev_day != Some(day) {
            let label = format!(" {} ", day_label(t));
            let pad = width.saturating_sub(label.width() + 2) / 2;
            out.push(Line::default());
            let mut spans = vec![Span::raw(" ".repeat(pad))];
            spans.extend(pill(label, META_IN, DAY_PILL));
            out.push(Line::from(spans));
            prev_day = Some(day);
            prev_sender = None;
        }
        let first = prev_sender != Some(m.sender.as_str());
        out.push(Line::default());
        let who_color = name_color(&m.sender);
        if faces.is_some() && !m.from_me && first {
            senders.push((out.len(), 1, m.sender.clone()));
        }

        // Only emojis: big, the time in a pill under their right end. A
        // sticker goes the same way, its picture without a bubble.
        let big = cell.filter(|_| m.quote.is_empty()).and_then(|c| crate::bigemoji::cells(&m.text, c)).map(|(w, h)| (w as usize, h, None));
        let sticker = cell.and_then(|(cw, ch)| {
            let md = m.media.as_ref().filter(|md| md.kind == "sticker")?;
            let cols = (STICKER_ROWS as f64 * ch as f64 / cw.max(1) as f64).round() as usize;
            Some((cols, STICKER_ROWS, Some(md)))
        });
        if let Some((ew, eh, sticker)) = sticker.or(big).filter(|&(ew, ..)| ew + 4 + face_w <= width) {
            starts.push((m.id.clone(), out.len()));
            let mut name_at = None;
            if group && !m.from_me && first {
                let name = Style::new().fg(who_color).add_modifier(Modifier::BOLD);
                let shown = truncate(&m.sender_name, inner);
                name_at = Some((out.len(), left, shown.width()));
                out.push(Line::from(vec![Span::raw(" ".repeat(left)), Span::styled(shown, name)]));
            }
            let mut stamp = vec![Span::styled(format!(" {}", t.format("%H:%M")), Style::new().fg(META_IN).bg(DAY_PILL))];
            // Big emojis only come with pictures: the ticks are one, over blanks.
            let tick_col = row_width(&stamp);
            if m.from_me {
                stamp.push(Span::styled(" ".repeat(crate::ticks::CELLS as usize), Style::new().bg(DAY_PILL)));
            }
            stamp.push(Span::styled(" ", Style::new().bg(DAY_PILL)));
            let pw = row_width(&stamp) + 2;
            let (ex, px) = if m.from_me { (width - 2 - ew, width.saturating_sub(2 + pw)) } else { (left, left + ew.saturating_sub(pw)) };
            match sticker {
                Some(md) => pics.push(crate::media::PicSpot::new(m, md, out.len(), ex, (ew, eh as usize))),
                None => bigs.push((out.len(), ex, (ew as u16, eh), m.text.clone())),
            }
            out.extend((0..eh).map(|_| Line::default()));
            if m.from_me {
                decos.push((out.len(), px + 1 + tick_col, Deco::Ticks(m.status, ticks(m.status).1)));
            }
            let mut spans = vec![Span::raw(" ".repeat(px)), Span::styled(CAP_L, Style::new().fg(DAY_PILL))];
            spans.extend(stamp);
            spans.push(Span::styled(CAP_R, Style::new().fg(DAY_PILL)));
            out.push(Line::from(spans));
            let (sx, sw) = (ex.min(px), (ex + ew).max(px + pw) - ex.min(px));
            let top = out.len() - 1 - eh as usize;
            spots.push(Spot { id: m.id.clone(), top, rows: eh as usize + 1, x: sx, w: sw, from_me: m.from_me, quote: None, name: name_at, sender: m.sender.clone() });
            out.extend(reactions_line(m, sx, sw, width));
            prev_sender = Some(m.sender.as_str());
            continue;
        }

        let (bg, meta_fg) = if m.from_me { (bubble_out(), meta_out()) } else { (BUBBLE_IN, META_IN) };
        let body = Style::new().fg(TEXT).bg(bg);
        let meta = Style::new().fg(meta_fg).bg(bg);

        // The bubble's content, row by row.
        let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
        let mut name_w = None;
        if group && !m.from_me && first {
            let shown = truncate(&m.sender_name, inner);
            name_w = Some(shown.width());
            rows.push(vec![Span::styled(shown, Style::new().fg(who_color).bg(bg).add_modifier(Modifier::BOLD))]);
        }
        // A quote in a box of its own, as WhatsApp Web's: a bar in its
        // author's color, their name, then up to two lines of it.
        let quote_bg = if m.from_me { code_out() } else { DAY_PILL };
        let mut quote_rows = 0..0;
        if !m.quote.is_empty() {
            let bar_color = if m.quote_from.is_empty() { green() } else { name_color(&m.quote_from) };
            let bar = Span::styled("▎ ", Style::new().fg(bar_color).bg(quote_bg));
            let qw = inner.saturating_sub(3);
            let start = rows.len();
            if !m.quote_name.is_empty() {
                let who = Style::new().fg(bar_color).bg(quote_bg).add_modifier(Modifier::BOLD);
                rows.push(vec![bar.clone(), Span::styled(truncate(&m.quote_name, qw), who)]);
            }
            let mut text = wrap(&m.quote, qw);
            if text.len() > 2 {
                text[1] = truncate(&format!("{}…", text[1].trim_end()), qw);
            }
            for l in text.into_iter().take(2) {
                rows.push(vec![bar.clone(), Span::styled(l, Style::new().fg(meta_fg).bg(quote_bg))]);
            }
            quote_rows = start..rows.len();
        }
        // A photo or video across the bubble, blank rows its picture goes
        // over; the text under it wraps to its width, as WhatsApp does.
        let media = cell.and_then(|c| crate::media::shown(m).map(|md| (md, crate::media::cells(md, c, inner + 1))));
        let wrap = media.map_or(inner, |(_, (pw, _))| pw - 1);
        let pic_row = rows.len();
        if let Some((_, (_, ph))) = media {
            rows.extend((0..ph).map(|_| Vec::new()));
        }
        let card = m.card.as_ref();
        if let Some(title) = card.map(|c| c.title.as_str()).filter(|t| !t.is_empty()) {
            rows.extend(wrap_rows(title, wrap, body.add_modifier(Modifier::BOLD)));
        }
        let text = if media.is_some() { crate::media::caption(m) } else { m.text.as_str() };
        // Links underlined in their color, on every row they wrap onto.
        let link = Style::new().fg(if m.from_me { link_out() } else { LINK_IN }).bg(bg).add_modifier(Modifier::UNDERLINED);
        let mut pieces = Vec::new();
        // An audio is a player, not its text.
        let audio = audio_of(m);
        let mut player = None;
        let n_text = if media.is_some() && text.is_empty() {
            0
        } else if let Some(a) = &audio {
            let now = now.filter(|n| n.id == m.id);
            let (r, icon_w, track, track_w) = audio_rows(a, now, m.from_me, bg, meta_fg, cell, inner);
            let state = now.map(|n| n.state);
            // A blank row over the player, and one under it after the time.
            rows.push(Vec::new());
            player = Some((rows.len(), icon_w, track, track_w, state));
            rows.extend(r);
            2
        } else {
            // WhatsApp's formatting, its markers hidden: the rows' widths are what's shown.
            let mention = Style::new().fg(if m.from_me { link_out() } else { green() }).bg(bg);
            let looks = crate::markup::Looks { base: body, link, mention, shade: if m.from_me { code_out() } else { CODE_IN }, dim: meta_fg };
            let (text, at) = crate::markup::mentions(text, &m.mentions);
            let (text, found) = crate::markup::rows(&text, wrap, &looks, &at);
            pieces.extend(found.into_iter().map(|(r, c, w, url)| (rows.len() + r, c, w, url)));
            let n_text = text.len();
            rows.extend(text);
            n_text
        };
        // A business's footer, dim under the text; the time can follow it.
        let mut n_text = n_text;
        if let Some(foot) = card.map(|c| c.footer.as_str()).filter(|t| !t.is_empty()) {
            let r = wrap_rows(foot, wrap, meta);
            n_text += r.len();
            rows.extend(r);
        }
        // Its buttons, each under a rule, centered: an icon and the label.
        let buttons: Vec<(String, String)> = card
            .map(|c| c.buttons.iter().map(|b| (truncate(&format!("{} {}", button_icon(&b.kind, &b.url), b.text), inner), b.url.clone())).collect())
            .unwrap_or_default();

        let mut footer = vec![Span::styled(t.format("%H:%M").to_string(), meta)];
        let (tick_text, tick_color) = ticks(m.status);
        let tick_color = if tick_color == META_IN { meta_fg } else { tick_color };
        if m.from_me && rounded {
            // Blanks the ticks' picture goes over.
            footer.push(Span::styled(" ".repeat(crate::ticks::CELLS as usize), Style::new().bg(bg)));
        } else if m.from_me {
            footer.push(Span::styled(format!(" {tick_text}"), Style::new().fg(tick_color).bg(bg)));
        }
        let footer_w: usize = footer.iter().map(|s| s.content.width()).sum();

        // The time goes at the end of the last line when it fits, as WhatsApp does.
        let last_w = rows.last().map_or(0, |r| row_width(r));
        let inline = n_text > 0 && last_w + 2 + footer_w <= wrap;
        let content_w = rows
            .iter()
            .map(|r| row_width(r))
            .chain([if inline { last_w + 2 + footer_w } else { footer_w }])
            .chain(media.map(|(_, (pw, _))| pw - 1))
            .chain(buttons.iter().map(|(b, _)| b.width()))
            .max()
            .unwrap_or(0);
        let bw = content_w + 3;
        // A column kept free on the tail's side.
        let x = if m.from_me { width.saturating_sub(bw + 2) } else { left };
        if !inline {
            let mut r = vec![Span::styled(" ".repeat(content_w - footer_w), body)];
            r.extend(footer.clone());
            rows.push(r);
        }

        let footer_row = rows.len() - 1;
        if audio.is_some() {
            rows.push(Vec::new());
        }
        let rule = Style::new().fg(if m.from_me { rule_out() } else { RULE_IN }).bg(bg);
        let button = Style::new().fg(green()).bg(bg);
        for (label, url) in &buttons {
            rows.push(vec![Span::styled("─".repeat(content_w), rule)]);
            let left = (content_w - label.width()) / 2;
            if !url.is_empty() {
                pieces.push((rows.len(), 0, content_w, url.clone()));
            }
            rows.push(vec![Span::styled(" ".repeat(left), body), Span::styled(label.clone(), button)]);
        }
        let top = out.len();
        starts.push((m.id.clone(), top));
        let n = rows.len();
        // Where a picture rounds the corner, the cell is left bare under it.
        let edge = |row: usize, left: bool| {
            let square_side = first && left != m.from_me && row == 0;
            if rounded && (row == 0 || row + 1 == n) && !square_side {
                Style::new()
            } else {
                body
            }
        };
        for (i, mut r) in rows.into_iter().enumerate() {
            let used = row_width(&r);
            let mut spans = vec![Span::raw(" ".repeat(x)), Span::styled(" ", edge(i, true))];
            if i == footer_row && inline {
                r.push(Span::styled(" ".repeat(content_w - used - footer_w), body));
                r.extend(footer.clone());
            } else if quote_rows.contains(&i) {
                // Across the bubble; a rounded corner's cell bare under its picture.
                let corner = rounded && (i == quote_rows.start || i + 1 == quote_rows.end);
                let fill = Style::new().bg(quote_bg);
                r.push(Span::styled(" ".repeat((content_w - used).saturating_sub(usize::from(corner))), fill));
                if corner {
                    r.push(Span::styled(" ", body));
                }
            } else {
                r.push(Span::styled(" ".repeat(content_w - used), body));
            }
            spans.extend(r);
            spans.push(Span::styled(" ", body));
            spans.push(Span::styled(" ", edge(i, false)));
            out.push(Line::from(spans));
        }
        if rounded {
            // On the tail's side the top corner stays square.
            let square = match (first, m.from_me) {
                (true, false) => TL,
                (true, true) => TR,
                _ => 0,
            };
            decos.extend(box_corners(top, n, x, bw, bg, square));
            if !quote_rows.is_empty() {
                // The bar's side stays square.
                decos.extend(box_corners(top + quote_rows.start, quote_rows.len(), x + 1, content_w, quote_bg, TL | BL));
            }
            if m.from_me {
                decos.push((top + footer_row, x + 1 + content_w - crate::ticks::CELLS as usize, Deco::Ticks(m.status, tick_color)));
            }
            if first {
                let col = if m.from_me { x + bw } else { x - 1 };
                decos.push((top, col, Deco::Tail(!m.from_me, bg)));
            }
        }
        link_cells.extend(pieces.into_iter().map(|(r, c, w, url)| (top + r, x + 1 + c, w, url)));
        if let Some((md, (pw, ph))) = media {
            // Centered when the bubble's wider than it (a long name over it).
            let col = x + 1 + (content_w + 1).saturating_sub(pw) / 2;
            pics.push(crate::media::PicSpot::new(m, md, top + pic_row, col, (pw, ph)));
        }
        if let (Some(a), Some((row, icon_w, track, track_w, state))) = (audio, player) {
            let col = x + 2;
            // The button's a picture, but for the spinner while it downloads.
            if rounded && state != Some(State::Loading) {
                let color = if a.playable { PLAY } else { meta_fg };
                decos.push((top + row, col + icon_w + 1, Deco::Play(state == Some(State::Playing), color)));
            }
            audios.push(AudioSpot {
                row: top + row,
                col,
                icon_w,
                track: col - 1 + track,
                track_w,
                id: m.id.clone(),
                sender: m.sender.clone(),
                ptt: a.ptt,
                playable: a.playable,
                secs: a.secs,
            });
        }
        let quote = (!quote_rows.is_empty()).then(|| (top + quote_rows.start, quote_rows.len()));
        let name = name_w.map(|w| (top, x + 1, w));
        spots.push(Spot { id: m.id.clone(), top, rows: n, x, w: bw, from_me: m.from_me, quote, name, sender: m.sender.clone() });
        out.extend(reactions_line(m, x, bw, width));
        prev_sender = Some(m.sender.as_str());
    }
    (out, decos, starts, bigs, link_cells, audios, pics, senders, spots)
}

/// The reactions under a bubble, as WhatsApp Web's: a pill with up to three
/// emojis, the most used first, and how many reacted when more than one;
/// at the bubble's left for others', its right for yours.
fn reactions_line(m: &Msg, x: usize, w: usize, width: usize) -> Option<Line<'static>> {
    if m.reactions.is_empty() {
        return None;
    }
    let total: u32 = m.reactions.iter().map(|r| r.count).sum();
    let mut label = String::from(" ");
    for r in m.reactions.iter().take(3) {
        label.push_str(&r.emoji);
    }
    if total > 1 {
        label.push_str(&format!(" {total}"));
    }
    label.push(' ');
    let pw = label.width() + 2;
    let col = if m.from_me { (x + w).saturating_sub(pw + 1) } else { x + 1 };
    let mut spans = vec![Span::raw(" ".repeat(col.min(width.saturating_sub(pw))))];
    spans.extend(pill(label, TEXT, FIELD));
    Some(Line::from(spans))
}

/// What's drawn over the panel's text: the big picture, or a member's.
enum Pic {
    Big,
    Member(String),
}

/// WhatsApp Web's contact (or group) info, at the right as slick's profile
/// is: the picture, the name and number, then sections under thin rules —
/// the about, a business's details, a group's description and members (a
/// click opens a chat with one). `beside` the conversation it has a line
/// between them; over it, the sidebar's line is enough.
fn profile_panel(f: &mut Frame, app: &mut App, area: Rect, beside: bool) {
    app.panel_area = area;
    let Some(view) = app.profile.as_ref() else { return };
    let chat = view.chat.clone();
    let group = chat.ends_with("@g.us");
    let inner = if beside {
        let block = Block::new().borders(Borders::LEFT).border_style(dim());
        let inner = block.inner(area);
        f.render_widget(block, area);
        inner
    } else {
        area
    };
    if inner.width < 12 || inner.height < 6 {
        return;
    }
    let w = inner.width as usize;

    // The header, as tall as the conversation's so their rules line up.
    let title = if group { "Dados do grupo" } else { "Dados do contato" };
    f.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(" ✕   ", Style::new().fg(META_IN)), Span::styled(title, Style::new().fg(TEXT).bold())]))
            .block(Block::new().borders(Borders::BOTTOM).border_style(dim())),
        Rect { height: 3, ..inner },
    );
    app.panel_close = Rect::new(inner.x, inner.y, 4, 1);
    let body = Rect { y: inner.y + 3, height: inner.height - 3, ..inner };

    let center_line = |text: String, style: Style| Line::from(Span::styled(truncate(&text, w.saturating_sub(4)), style)).alignment(Alignment::Center);
    let meta = Style::new().fg(META_IN);
    let text = Style::new().fg(TEXT);
    // Text sits 2 cells in from both sides.
    let text_w = w.saturating_sub(4);
    let mut lines: Vec<Line> = vec![Line::default()];
    let mut pics: Vec<(usize, u16, u16, u16, Pic)> = Vec::new();
    let mut hits: Vec<(usize, String)> = Vec::new();
    let mut week_row = None;
    let mut maps_rows = None;

    // The picture, round and centered: about half the panel wide, and no
    // more than 2/5 of its height.
    if let Some((fw, fh)) = cell_px(app) {
        let mut cols = (w as u16 / 2).max(12).min(w as u16 - 4);
        let mut rows = ((cols as f64 * fw as f64 / fh as f64).round() as u16).max(1);
        let most = (body.height * 2 / 5).max(4);
        if rows > most {
            rows = most;
            cols = (rows as f64 * fh as f64 / fw as f64).round() as u16;
        }
        pics.push((lines.len(), (w as u16 - cols) / 2, cols, rows, Pic::Big));
        lines.extend((0..rows).map(|_| Line::default()));
        lines.push(Line::default());
    }

    let fallback = app.name_of(&chat).unwrap_or_else(|| phone(&chat));
    match &view.data {
        None => {
            lines.push(center_line(display_name(&fallback, &chat), text.bold()));
            lines.push(Line::default());
            lines.push(center_line("Carregando…".into(), meta));
        }
        Some(d) => {
            let name = display_name(if d.name.is_empty() { &fallback } else { &d.name }, &chat);
            lines.push(center_line(name.clone(), text.bold()));
            let number = phone(&chat);
            if group {
                lines.push(center_line(format!("Grupo · {} membros", d.members.len()), meta));
            } else if !number.is_empty() && number != name {
                lines.push(center_line(number, meta));
            }
            if !d.push_name.is_empty() && d.push_name != name && !d.biz {
                lines.push(center_line(format!("~{}", d.push_name), meta));
            }
            if !d.category.is_empty() {
                lines.push(center_line(d.category.clone(), meta));
            }
            // Open or not, as WhatsApp Web puts it: "Aberta até às 17:00".
            let open = (!d.hours.is_empty()).then(|| opening(&d.hours, d.tz));
            if let Some(o) = &open {
                let (word, color) = if o.open { ("Aberta", green()) } else { ("Fechada", CLOSED) };
                let mut spans = vec![Span::styled(word, Style::new().fg(color))];
                if !o.next.is_empty() {
                    spans.push(Span::styled(format!(" {}", o.next), meta));
                }
                lines.push(Line::from(spans).alignment(Alignment::Center));
            }
            if !d.error.is_empty() {
                lines.push(Line::default());
                lines.push(center_line(format!("Não carregou: {}", d.error), Style::new().fg(Color::Red)));
            }

            if group {
                section(&mut lines, w, "Descrição", "");
                if d.about.trim().is_empty() {
                    lines.push(Line::from(Span::styled("  Sem descrição", meta)));
                } else {
                    paragraph(&mut lines, &d.about, text_w, "  ");
                }
                if d.created > 0 {
                    section(&mut lines, w, "Criado em", "");
                    lines.push(Line::from(Span::styled(format!("  {}", local(d.created).format("%d/%m/%Y às %H:%M")), text)));
                }
            }
            if d.biz {
                section(&mut lines, w, "Conta comercial", INFO);
                let before = lines.len();
                // A blank row between the details, as WhatsApp Web spaces them.
                let gap = |lines: &mut Vec<Line<'static>>| {
                    if lines.len() > before {
                        lines.push(Line::default());
                    }
                };
                let in_green = |lines: &mut Vec<Line<'static>>, t: &str| {
                    for l in wrap(t, text_w) {
                        lines.push(Line::from(Span::styled(format!("  {l}"), Style::new().fg(green()))));
                    }
                };
                if !d.description.trim().is_empty() {
                    paragraph(&mut lines, d.description.trim(), text_w, "  ");
                }
                // Today's hours, a click opens the week.
                if let Some(o) = &open {
                    gap(&mut lines);
                    week_row = Some(lines.len());
                    let (now, color) = match (o.always, o.open) {
                        (true, _) => ("Aberta 24 horas", green()),
                        (_, true) => ("Aberta agora", green()),
                        _ => ("Fechada agora", CLOSED),
                    };
                    let today = if o.always { String::new() } else { day_hours(&d.hours, o.day) };
                    let right = format!("{today}  {}", if view.week { UP } else { DOWN });
                    let pad = text_w.saturating_sub(now.width() + right.width());
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(now, Style::new().fg(color)),
                        Span::raw(" ".repeat(pad)),
                        Span::styled(right, meta),
                    ]));
                    if view.week {
                        for day in 0..7u8 {
                            let mut name = DAYS[day as usize].chars();
                            let name: String = name.next().into_iter().flat_map(char::to_uppercase).chain(name).collect();
                            let hours = day_hours(&d.hours, day);
                            let style = if day == o.day { text.bold() } else { meta };
                            let pad = text_w.saturating_sub(name.width() + hours.width());
                            lines.push(Line::from(vec![Span::raw("  "), Span::styled(name, style), Span::raw(" ".repeat(pad)), Span::styled(hours, style)]));
                        }
                    }
                }
                // The address, and under it a link to Google Maps: a click
                // on either opens it.
                if !d.address.trim().is_empty() || !d.maps.is_empty() {
                    gap(&mut lines);
                    let from = lines.len();
                    for l in wrap(d.address.trim(), text_w).into_iter().filter(|l| !l.is_empty()) {
                        lines.push(Line::from(Span::styled(format!("  {l}"), text)));
                    }
                    if !d.maps.is_empty() {
                        lines.push(Line::from(Span::styled(format!("  {PIN} Abrir no Google Maps"), Style::new().fg(green()))));
                        maps_rows = Some((from, lines.len()));
                    }
                }
                if !d.email.is_empty() {
                    gap(&mut lines);
                    in_green(&mut lines, &d.email);
                }
                for site in &d.website {
                    gap(&mut lines);
                    in_green(&mut lines, site);
                }
                if lines.len() == before {
                    lines.push(Line::from(Span::styled("  Esta é uma conta comercial.", meta)));
                }
            }
            if !group && !d.about.trim().is_empty() {
                section(&mut lines, w, "Recado", "");
                paragraph(&mut lines, &d.about, text_w, "  ");
            }

            if group && !d.members.is_empty() {
                section(&mut lines, w, &format!("{} membros", d.members.len()), "");
                let avatar = avatar_cells(app).map(|(w, _)| w);
                let lead = match avatar {
                    Some(a) => " ".repeat(a as usize + 4),
                    None => "  ".into(),
                };
                let room = w.saturating_sub(lead.width() + 2);
                for m in &d.members {
                    let row = lines.len();
                    let name = if m.me { "Você".to_string() } else { display_name(&m.name, &m.jid) };
                    let number = phone(&m.jid);
                    let sub = if !m.me && number != name { number } else { String::new() };
                    let tag = if m.admin { " Admin do grupo " } else { "" };
                    let tag_w = if m.admin { tag.width() + 2 } else { 0 };
                    let name = truncate(&name, room.saturating_sub(tag_w + 1));
                    let gap = room.saturating_sub(name.width() + tag_w);
                    let mut top = vec![Span::raw(lead.clone()), Span::styled(name, text), Span::raw(" ".repeat(gap))];
                    if m.admin {
                        top.extend(pill(tag.into(), chip_on_text(), chip_on()));
                    }
                    lines.push(Line::from(top));
                    lines.push(Line::from(vec![Span::raw(lead.clone()), Span::styled(truncate(&sub, room), meta)]));
                    // A row between members, as their pictures are two rows tall.
                    lines.push(Line::default());
                    if let Some(a) = avatar {
                        pics.push((row, 2, a, 2, Pic::Member(m.jid.clone())));
                    }
                    if !m.me {
                        hits.push((row, m.jid.clone()));
                    }
                }
            }
        }
    }
    lines.push(Line::default());

    // Scrolled with the wheel, within what there is. Pictures and click
    // targets only where they fit whole: a terminal picture can't be cut.
    let h = body.height as usize;
    let scroll = view.scroll.min(lines.len().saturating_sub(h));
    if let Some(v) = app.profile.as_mut() {
        v.scroll = scroll;
    }
    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(h).collect();
    f.render_widget(Paragraph::new(shown), body);
    let whole = |row: usize, rows: usize| row >= scroll && row + rows <= scroll + h;
    for (row, jid) in hits {
        if whole(row, 2) {
            app.panel_hits.push((Rect::new(body.x, body.y + (row - scroll) as u16, body.width, 2), jid));
        }
    }
    if let Some(row) = week_row.filter(|&r| whole(r, 1)) {
        app.panel_week = Rect::new(body.x, body.y + (row - scroll) as u16, body.width, 1);
    }
    // Only its rows on screen.
    if let Some((from, to)) = maps_rows {
        let (from, to) = (from.max(scroll), to.min(scroll + h));
        if from < to {
            app.panel_maps = Rect::new(body.x, body.y + (from - scroll) as u16, body.width, (to - from) as u16);
        }
    }
    for (row, x, cw, ch, pic) in pics {
        if !whole(row, ch as usize) {
            continue;
        }
        let rect = Rect::new(body.x + x, body.y + (row - scroll) as u16, cw, ch);
        match pic {
            Pic::Member(jid) => draw_avatar(f, app, &jid, false, rect),
            Pic::Big => draw_big_picture(f, app, &chat, group, rect),
        }
    }
}

/// A section of the info panel: a rule across it, then its dim title, with
/// `icon` at the right end.
fn section(lines: &mut Vec<Line<'static>>, w: usize, title: &str, icon: &str) {
    lines.push(Line::default());
    lines.push(Line::from("─".repeat(w)).style(dim()));
    lines.push(Line::default());
    let pad = w.saturating_sub(4 + title.width() + icon.width());
    lines.push(Line::from(Span::styled(format!("  {title}{}{icon}", " ".repeat(pad)), Style::new().fg(META_IN))));
}

/// Where a business's week stands now, on its own clock.
struct Opening {
    open: bool,
    /// Open every hour of every day.
    always: bool,
    /// The next change: "até às 17:00", "abre amanhã às 08:00".
    next: String,
    /// Today, 0 for Monday.
    day: u8,
}

fn clock(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60 % 24, minutes % 60)
}

/// A day's hours as WhatsApp Web lists them.
fn day_hours(hours: &[Hours], day: u8) -> String {
    let spans: Vec<String> = hours
        .iter()
        .filter(|h| h.day == day)
        .map(|h| match h.mode.as_str() {
            "open_24h" => "Aberta 24 horas".into(),
            "appointment_only" => "Com agendamento".into(),
            _ => format!("{} - {}", clock(h.open), clock(h.close)),
        })
        .collect();
    if spans.is_empty() {
        "Fechada".into()
    } else {
        spans.join(", ")
    }
}

fn opening(hours: &[Hours], tz: Option<i32>) -> Opening {
    let now = match tz.and_then(FixedOffset::east_opt) {
        Some(off) => Utc::now().with_timezone(&off).naive_local(),
        None => Local::now().naive_local(),
    };
    let day = now.weekday().num_days_from_monday() as u8;
    let min = (now.hour() * 60 + now.minute()) as u16;
    let all_day = |d: u8| hours.iter().any(|h| h.day == d && h.mode == "open_24h");
    if (0..7).all(all_day) {
        return Opening { open: true, always: true, next: String::new(), day };
    }
    // Hours past midnight close the next day.
    let end = |h: &Hours| if h.close <= h.open { h.close + 1440 } else { h.close };
    let timed = |h: &&Hours| h.mode != "open_24h" && h.mode != "appointment_only";
    if all_day(day) {
        return Opening { open: true, always: false, next: "24 horas hoje".into(), day };
    }
    for h in hours.iter().filter(timed) {
        let today = h.day == day && min >= h.open && min < end(h);
        let from_yesterday = h.day == (day + 6) % 7 && end(h) > 1440 && min < end(h) - 1440;
        if today || from_yesterday {
            return Opening { open: true, always: false, next: format!("até às {}", clock(h.close)), day };
        }
    }
    // Closed: when it opens next, later today or on a coming day.
    for k in 0..8u8 {
        let d = (day + k) % 7;
        let start = hours
            .iter()
            .filter(|h| h.day == d && h.mode != "appointment_only")
            .map(|h| if h.mode == "open_24h" { 0 } else { h.open })
            .filter(|&o| k > 0 || o > min)
            .min();
        if let Some(o) = start {
            let when = match k {
                0 => String::new(),
                1 => "amanhã ".into(),
                _ => format!("{} ", DAYS[d as usize]),
            };
            return Opening { open: false, always: false, next: format!("abre {when}às {}", clock(o)), day };
        }
    }
    Opening { open: false, always: false, next: String::new(), day }
}

/// Text wrapped to `width` after `lead`, with WhatsApp's formatting, its
/// links and e-mails in green.
fn paragraph(lines: &mut Vec<Line<'static>>, text: &str, width: usize, lead: &str) {
    let looks = crate::markup::Looks { base: Style::new().fg(TEXT), link: Style::new().fg(green()), mention: Style::new().fg(green()), shade: CODE_IN, dim: META_IN };
    for row in crate::markup::rows(text, width, &looks, &[]).0 {
        let mut spans = vec![Span::raw(lead.to_string())];
        spans.extend(row);
        lines.push(Line::from(spans));
    }
}

/// The panel's picture: the full-size photo once it's here, the small one
/// until then.
fn draw_big_picture(f: &mut Frame, app: &mut App, jid: &str, group: bool, rect: Rect) {
    let Some(picker) = app.picker.as_ref() else { return };
    let key = (format!("big:{jid}"), rect.width, rect.height);
    if !app.protocols.contains_key(&key) {
        let s = picker.font_size();
        let side = (rect.width as u32 * s.width as u32).min(rect.height as u32 * s.height as u32);
        let photo = app.profile.as_ref().and_then(|p| p.photo.as_ref());
        let img = match (photo, app.avatars.get(jid)) {
            _ if jid == "0@s.whatsapp.net" => crate::avatar::official(side),
            (Some(img), _) | (None, Some(Avatar::Ready(img))) => crate::avatar::round(img, side),
            _ => crate::avatar::silhouette(side, group),
        };
        let fit = crate::ui::fit(&img);
        let Ok(p) = picker.new_protocol(img, Rect::new(0, 0, rect.width, rect.height).into(), fit) else { return };
        app.protocols.insert(key.clone(), p);
    }
    f.render_widget(Image::new(&app.protocols[&key]), rect);
}

/// An audio file's icon: a white headphone on an orange circle.
fn draw_headphones(f: &mut Frame, app: &mut App, rect: Rect) {
    let Some(picker) = app.picker.as_ref() else { return };
    let key = ("headphones".to_string(), rect.width, rect.height);
    if !app.protocols.contains_key(&key) {
        let s = picker.font_size();
        let side = (rect.width as u32 * s.width as u32).min(rect.height as u32 * s.height as u32);
        let Ok(p) = picker.new_protocol(crate::avatar::headphones(side), Rect::new(0, 0, rect.width, rect.height).into(), Resize::Fit(None)) else { return };
        app.protocols.insert(key.clone(), p);
    }
    f.render_widget(Image::new(&app.protocols[&key]), rect);
}

/// Plain text wrapped to `width`, a row of spans each.
fn wrap_rows(text: &str, width: usize, style: Style) -> Vec<Vec<Span<'static>>> {
    wrap(text, width).into_iter().map(|l| vec![Span::styled(l, style)]).collect()
}

/// A business button's icon: a link opens, the rest only show what they are.
fn button_icon(kind: &str, url: &str) -> &'static str {
    match kind {
        _ if !url.is_empty() => LINK_ICON,
        "call" => CALL_ICON,
        "copy" => COPY_ICON,
        "list" => LIST_ICON,
        _ => REPLY_ICON,
    }
}

fn row_width(r: &[Span]) -> usize {
    r.iter().map(|s| s.content.width()).sum()
}

/// A note or an error for a few seconds, over the row of margin under the
/// message box (or the foot of that side): no row of its own, so nothing
/// moves when it comes and goes. The keys are in "Atalhos de teclado".
fn notice(f: &mut Frame, app: &App, conv: Rect) {
    let Some((text, _)) = &app.notice else { return };
    let r = if app.input_area.width > 0 { app.input_area } else { conv };
    let row = Rect { y: r.bottom().saturating_sub(1), height: 1, ..r };
    let text = truncate(text, row.width.saturating_sub(2) as usize);
    f.render_widget(Paragraph::new(text).red().alignment(Alignment::Center), row);
}

pub(crate) fn local(ts: i64) -> DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_else(Local::now)
}

fn short_time(ts: i64) -> String {
    if ts == 0 {
        return String::new();
    }
    let t = local(ts);
    let days = (Local::now().date_naive() - t.date_naive()).num_days();
    match days {
        0 => t.format("%H:%M").to_string(),
        1 => "Ontem".into(),
        2..=6 => weekday(t).into(),
        _ => t.format("%d/%m/%Y").to_string(),
    }
}

pub(crate) fn day_label(t: DateTime<Local>) -> String {
    let days = (Local::now().date_naive() - t.date_naive()).num_days();
    match days {
        0 => "Hoje".into(),
        1 => "Ontem".into(),
        2..=6 => weekday(t).into(),
        _ => t.format("%d/%m/%Y").to_string(),
    }
}

const DAYS: [&str; 7] = ["segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado", "domingo"];

fn weekday(t: DateTime<Local>) -> &'static str {
    DAYS[t.weekday().num_days_from_monday() as usize]
}

/// `5511999990000@s.whatsapp.net` → `+55 11 99999-0000` (Brazilian numbers; others as they come).
pub(crate) fn phone(jid: &str) -> String {
    let user = jid.split('@').next().unwrap_or(jid);
    if !jid.ends_with("@s.whatsapp.net") {
        return String::new();
    }
    if user.len() == 13 && user.starts_with("55") {
        return format!("+55 {} {}-{}", &user[2..4], &user[4..9], &user[9..]);
    }
    if user.len() == 12 && user.starts_with("55") {
        return format!("+55 {} {}-{}", &user[2..4], &user[4..8], &user[8..]);
    }
    format!("+{user}")
}

/// Greedy word wrap by display width; words longer than a line are split.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    wrap_indexed(text, width).into_iter().map(|(l, _)| l).collect()
}

/// `wrap`, with where in `text` each char of a line came from, by char.
pub(crate) fn wrap_indexed(text: &str, width: usize) -> Vec<(String, Vec<usize>)> {
    let mut out = Vec::new();
    // The char at the start of each word.
    let mut i = 0;
    for para in text.split('\n') {
        let mut line = (String::new(), Vec::new());
        let mut w = 0;
        for word in para.split(' ') {
            let ww = word.width();
            if w > 0 && w + 1 + ww > width {
                out.push(std::mem::take(&mut line));
                w = 0;
            }
            if w > 0 {
                line.0.push(' ');
                line.1.push(i - 1);
                w += 1;
            }
            for ch in word.chars() {
                let cw = ch.width().unwrap_or(0);
                if w + cw > width {
                    out.push(std::mem::take(&mut line));
                    w = 0;
                }
                line.0.push(ch);
                line.1.push(i);
                i += 1;
                w += cw;
            }
            // Past the space after it, or the paragraph's newline.
            i += 1;
        }
        out.push(line);
    }
    out
}

pub(crate) fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}
