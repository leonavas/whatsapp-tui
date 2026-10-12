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

// WhatsApp Web's dark theme.
pub(crate) const GREEN: Color = Color::Rgb(0x25, 0xD3, 0x66);
const BUBBLE_IN: Color = Color::Rgb(0x24, 0x26, 0x26);
const BUBBLE_OUT: Color = Color::Rgb(0x14, 0x4D, 0x37);
/// Monospace and code: a shade off each bubble, as on WhatsApp Web.
const CODE_IN: Color = Color::Rgb(0x33, 0x36, 0x36);
const CODE_OUT: Color = Color::Rgb(0x0E, 0x3A, 0x29);
pub(crate) const TEXT: Color = Color::Rgb(0xFA, 0xFA, 0xFA);
/// 60% white over each bubble.
pub(crate) const META_IN: Color = Color::Rgb(0xA7, 0xA8, 0xA8);
const META_OUT: Color = Color::Rgb(0xA1, 0xB8, 0xAF);
const READ_BLUE: Color = Color::Rgb(0x53, 0xBD, 0xEB);
/// Links: WhatsApp's blue on the grey bubbles, a light green on our own.
pub(crate) const LINK_IN: Color = Color::Rgb(0x53, 0xBD, 0xEB);
const LINK_OUT: Color = Color::Rgb(0x7F, 0xD8, 0xA8);
pub(crate) const DAY_PILL: Color = Color::Rgb(0x1D, 0x1F, 0x1F);
pub(crate) const FIELD: Color = Color::Rgb(0x24, 0x26, 0x26);
const SELECTED: Color = Color::Rgb(0x2E, 0x2F, 0x2F);
const CHIP_ON: Color = Color::Rgb(0x10, 0x35, 0x29);
const CHIP_ON_TEXT: Color = Color::Rgb(0xD9, 0xFD, 0xD3);
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

pub(crate) fn draw_deco(f: &mut Frame, app: &mut App, d: Deco, x: u16, y: u16) {
    let (Some((w, h)), Some(picker)) = (cell_px(app), app.picker.as_ref()) else { return };
    let (key, cols) = match d {
        Deco::Corners(m, c) => (format!("corner-{m}-{:?}", rgb(c)), 1),
        Deco::Tail(l, c) => (format!("tail-{l}-{:?}", rgb(c)), 1),
        Deco::Ticks(s, c) => (format!("ticks-{s}-{:?}", rgb(c)), crate::ticks::CELLS),
        Deco::Play(p, c) => (format!("play-{p}-{:?}", rgb(c)), 2),
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
        return main_view(f, app, area);
    }
    match &app.phase {
        Phase::Qr(code) => qr_view(f, code, area),
        Phase::LoggedOut if app.account.signing_out => center(f, area, vec![Line::from("Aparelho desconectado. Gerando um QR novo…")]),
        Phase::LoggedOut => center(f, area, vec![Line::from("Sessão encerrada no celular. Gerando um QR novo…")]),
        Phase::Dead(why) => center(
            f,
            area,
            vec![Line::from(why.clone()).red(), Line::default(), Line::from("r reinicia · q sai").style(dim())],
        ),
        _ => center(f, area, vec![Line::from(vec![Span::styled("WhatsApp TUI", Style::new().fg(GREEN).bold()), Span::raw("  conectando ao WhatsApp…")])]),
    }
}

fn center(f: &mut Frame, area: Rect, lines: Vec<Line>) {
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), Rect { y, height: h.min(area.height), ..area });
}

fn qr_view(f: &mut Frame, code: &str, area: Rect) {
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
    let rows = total.div_ceil(2);
    let text = [
        Line::from("Escaneie com o WhatsApp do celular").bold(),
        Line::from("Configurações › Aparelhos conectados › Conectar um aparelho").style(dim()),
        Line::from("q sai").style(dim()),
    ];
    if (area.width as usize) < total || (area.height as usize) < rows + text.len() + 1 {
        return center(f, area, vec![Line::from(format!("Aumente o terminal para mostrar o QR (precisa de {total}×{}).", rows + 4))]);
    }
    // Two modules per cell: the upper one in the foreground, the lower in the
    // background, in fixed black and white so any theme reads it.
    let paint = |d: bool| if d { Color::Rgb(0, 0, 0) } else { Color::Rgb(255, 255, 255) };
    let mut lines: Vec<Line> = (0..total)
        .step_by(2)
        .map(|y| Line::from((0..total).map(|x| Span::styled("▀", Style::new().fg(paint(dark(x, y))).bg(paint(dark(x, y + 1))))).collect::<Vec<_>>()))
        .collect();
    lines.push(Line::default());
    lines.extend(text);
    center(f, area, lines);
}

/// The avatar's size in cells, two rows tall and square in pixels; None
/// without a picture-capable terminal.
pub(crate) fn avatar_cells(app: &App) -> Option<(u16, u16)> {
    let p = app.picker.as_ref()?;
    let s = p.font_size();
    let cols = ((2.0 * s.height as f64 / s.width as f64).round() as u16).clamp(3, 6);
    Some((cols, 2))
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
        let Ok(p) = picker.new_protocol(img, Rect::new(0, 0, rect.width, rect.height).into(), Resize::Fit(None)) else { return };
        app.protocols.insert(key.clone(), p);
    }
    f.render_widget(Image::new(&app.protocols[&key]), rect);
    app.want_avatar(jid);
}

fn main_view(f: &mut Frame, app: &mut App, area: Rect) {
    let [body, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
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
    crate::emoji::draw(f, app);
    crate::emoji_panel::draw(f, app);
    crate::attach::draw_menu(f, app);
    crate::account::draw(f, app);
    // Last, so only what's still a link on screen becomes one for the terminal.
    let is_link = |c: &ratatui::buffer::Cell| c.modifier.contains(Modifier::UNDERLINED) && (c.fg == LINK_IN || c.fg == LINK_OUT);
    for (r, url) in &app.msg_links {
        crate::links::osc8(f.buffer_mut(), *r, url, is_link);
    }
    status_bar(f, app, status);
}

/// The info panel's width: WhatsApp Web's share of the screen, while the
/// conversation keeps 40 columns; None when that leaves it too narrow.
fn panel_width(total: u16) -> Option<u16> {
    let w = (total * 2 / 5).clamp(PANEL, 64).min(total.saturating_sub(40));
    (w >= 36).then_some(w)
}

fn sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    // The search field, a pill across the column.
    let field_w = (area.width as usize).saturating_sub(4);
    let searching = app.focus == Focus::Search;
    let (text, fg) = if app.search.is_empty() && !searching {
        ("Pesquisar ou começar uma nova conversa".to_string(), META_IN)
    } else {
        (app.search.clone(), TEXT)
    };
    let icon_fg = if searching { GREEN } else { META_IN };
    let text = truncate(&text, field_w.saturating_sub(4));
    let pad = field_w.saturating_sub(3 + text.width());
    let mut search = vec![Span::raw(" "), Span::styled(CAP_L, Style::new().fg(FIELD))];
    search.push(Span::styled(format!("{SEARCH}  "), Style::new().fg(icon_fg).bg(FIELD)));
    search.push(Span::styled(text, Style::new().fg(fg).bg(FIELD)));
    search.push(Span::styled(" ".repeat(pad), Style::new().bg(FIELD)));
    search.push(Span::styled(CAP_R, Style::new().fg(FIELD)));

    // The chips: All, Unread, Groups.
    let unread = app.chats.iter().filter(|c| c.unread > 0).count();
    let chips = [
        (Filter::All, "Tudo".to_string()),
        (Filter::Unread, if unread > 0 { format!("Não lidas {unread}") } else { "Não lidas".into() }),
        (Filter::Groups, "Grupos".into()),
    ];
    let mut chip_spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    app.chips.clear();
    for (filter, label) in chips {
        let (fg, bg) = if app.filter == filter { (CHIP_ON_TEXT, CHIP_ON) } else { (META_IN, FIELD) };
        let w = label.width() as u16 + 4;
        app.chips.push((Rect::new(x, area.y + 2, w, 1), filter));
        chip_spans.extend(pill(format!(" {label} "), fg, bg));
        chip_spans.push(Span::raw(" "));
        x += w + 1;
    }

    // The search field on top, then a blank row before the chips.
    app.search_area = Rect::new(area.x + 1, area.y, field_w as u16, 1);
    f.render_widget(Paragraph::new(vec![Line::from(search), Line::default(), Line::from(chip_spans)]), area);
    if searching {
        let x = area.x + 5 + app.search.width() as u16;
        f.set_cursor_position(Position::new(x.min(area.right().saturating_sub(2)), area.y));
    }

    let list = Rect { y: area.y + 4, height: area.height.saturating_sub(4), ..area };
    app.side_list = list;
    app.side_hits.clear();
    let cells = avatar_cells(app).filter(|_| app.settings.on(crate::config::PHOTOS));
    let items = app.items();
    if items.is_empty() {
        let msg = if app.filter == Filter::All { "Nenhuma conversa ainda" } else { "Nada encontrado" };
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
            Item::Header(t) => lines.push(Line::from(Span::styled(format!(" {t}"), Style::new().fg(GREEN)))),
            Item::Info(t) => lines.push(Line::from(Span::styled(format!(" {t}"), Style::new().fg(META_IN)))),
            Item::Chat(c) => {
                lines.extend(chat_item(c, width, avatar_w, on, app.typing.contains_key(&c.jid), app.draft_line(&c.jid)));
                pictures.push((y, c.jid.clone(), c.group));
                statuses.push((y, h, c.status));
            }
            Item::Contact(c) => {
                lines.extend(contact_item(&c.name, &phone(&c.jid), Style::new().fg(META_IN), width, avatar_w, on));
                pictures.push((y, c.jid.clone(), false));
            }
            Item::Number(n) => {
                let (sub, style) = if n.exists {
                    ("Começar uma conversa", Style::new().fg(GREEN))
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
    // Its corner cells bare, the pictures round them.
    if let Some((top, h)) = lit_at.filter(|_| app.picker.is_some() && width > 1) {
        for (row, col, d) in box_corners(0, h as usize, 0, width, SELECTED, 0) {
            let (x, y) = (inner.x + col as u16, top + row as u16);
            f.buffer_mut()[(x, y)].set_bg(Color::Reset);
            draw_deco(f, app, d, x, y);
        }
    }
    if let Some((w, _)) = cells {
        for (y, jid, group) in pictures {
            draw_avatar(f, app, &jid, group, Rect::new(inner.x + 1, y, w, 3));
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
            spans.push(Span::styled(chars[rel..end].iter().collect::<String>(), Style::new().fg(GREEN).bold()));
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

    let name_style = match (unread, c.archived) {
        (true, _) => Style::new().fg(TEXT).bold(),
        (false, true) => dim(),
        _ => Style::new().fg(TEXT),
    };
    let name = truncate(&display_name(&c.name, &c.jid), inner.saturating_sub(time.width() + 1));
    let gap = inner.saturating_sub(name.width() + time.width());
    let time_style = if unread && !c.muted { Style::new().fg(GREEN) } else { Style::new().fg(META_IN) };
    let top = Line::from(vec![
        Span::raw(lead.clone()),
        Span::styled(name, name_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(time, time_style),
        Span::raw(" "),
    ])
    .style(bg);

    let mut right: Vec<Span> = Vec::new();
    if c.muted {
        right.push(Span::styled("🔕", Style::new().fg(META_IN)));
    }
    if c.pinned {
        right.push(Span::styled("📌", Style::new().fg(META_IN)));
    }
    if unread {
        let badge = if c.muted { META_IN } else { GREEN };
        right.extend(pill(c.unread.to_string(), Color::Black, badge));
    }
    let right_w: usize = right.iter().map(|s| s.content.width()).sum::<usize>() + usize::from(c.muted && c.pinned);
    let mut left: Vec<Span> = vec![Span::raw(lead)];
    let room = inner.saturating_sub(right_w + 1);
    if typing {
        left.push(Span::styled(truncate("digitando…", room), Style::new().fg(GREEN)));
    } else if let Some(d) = draft {
        // What was left unsent, in place of the last message, as on the Web.
        let label = "Rascunho: ";
        left.push(Span::styled(truncate(label, room), Style::new().fg(GREEN).bold()));
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
    for (i, s) in right.into_iter().enumerate() {
        if i == 1 && c.muted && c.pinned {
            left.push(Span::raw(" "));
        }
        left.push(s);
    }
    left.push(Span::raw(" "));
    [top, Line::from(left).style(bg)]
}

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
                Line::from("WhatsApp TUI").fg(GREEN).bold(),
                Line::default(),
                Line::from("Escolha uma conversa à esquerda").style(dim()),
                Line::from("↑↓ navega · Enter abre · / busca").style(dim()),
            ],
        );
    };
    let chat = app.chat(&jid).cloned();
    let group = jid.ends_with("@g.us");
    let read_only = chat.as_ref().map(|c| c.read_only.clone()).unwrap_or_default();

    let input_h = if read_only.is_empty() {
        input_lines(&app.input, area.width.saturating_sub(8) as usize).len().clamp(1, 6) as u16 + 4 + crate::attach::rows(app)
    } else {
        3
    };
    let [header, msgs_area, input] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1), Constraint::Length(input_h)]).areas(area);
    app.msgs_area = msgs_area;
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
        Some((who, _)) if group => Span::styled(format!("{who} está digitando…"), Style::new().fg(GREEN)),
        Some(_) => Span::styled("digitando…", Style::new().fg(GREEN)),
        None if jid == "0@s.whatsapp.net" => Span::styled("Conta oficial do WhatsApp", Style::new().fg(META_IN)),
        None if group => Span::styled("clique para ver os dados do grupo", Style::new().fg(META_IN)),
        None if name == number => Span::styled("clique para ver os dados do contato", Style::new().fg(META_IN)),
        None => Span::styled(number, Style::new().fg(META_IN)),
    };
    let pad = " ".repeat(indent as usize);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::raw(pad.clone()), Span::styled(name, Style::new().fg(TEXT).bold())]),
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
    let (lines, decos, starts, bigs, links, audios) = message_lines(msgs, msgs_area.width as usize, group, cell_px(app), now);
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
    // Big emojis only whole: a terminal picture can't be cut.
    for (row, col, (w, eh), text) in bigs {
        if row >= start && row + eh as usize <= end {
            crate::bigemoji::draw(f, app, &text, Rect::new(msgs_area.x + col as u16, msgs_area.y + top_pad + (row - start) as u16, w, eh));
        }
    }
    if app.scroll > 0 {
        let label = " ↓ mais recentes ";
        let w = label.width() as u16 + 2;
        let r = Rect { x: msgs_area.right().saturating_sub(w + 2), y: msgs_area.bottom().saturating_sub(1), width: w, height: 1 };
        f.render_widget(Paragraph::new(Line::from(pill(label.into(), Color::Black, GREEN))), r);
    }

    if read_only.is_empty() {
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

/// The conversation as bubbles, a blank row between them. With pictures
/// in the terminal their corners are rounded and the first of each run of
/// one sender's messages has WhatsApp's tail, on its side: the returned
/// (row, col, deco) say where those pictures go. A message of only emojis
/// goes without a bubble, big, as (row, col, size, text).
/// Then the links' pieces, a row each, as (row, col, width, url); last,
/// the audio players.
type Bubbles = (
    Vec<Line<'static>>,
    Vec<(usize, usize, Deco)>,
    Vec<(String, usize)>,
    Vec<(usize, usize, (u16, u16), String)>,
    Vec<(usize, usize, usize, String)>,
    Vec<AudioSpot>,
);

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
        None if a.ptt => Span::styled(MIC, on(GREEN)),
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
    let accent = if from_me { LINK_OUT } else { LINK_IN };
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
fn message_lines(msgs: &[Msg], width: usize, group: bool, cell: Option<(u32, u32)>, now: Option<&Now>) -> Bubbles {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut decos = Vec::new();
    let mut starts = Vec::new();
    let mut bigs = Vec::new();
    let mut link_cells = Vec::new();
    let mut audios = Vec::new();
    let rounded = cell.is_some();
    if width < 16 {
        return (out, decos, starts, bigs, link_cells, audios);
    }
    let outer = (width * 3 / 4).clamp(16, 90).min(width - 4);
    let inner = outer - 2;
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
        let name_color = NAMES[m.sender.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)) % NAMES.len()];

        // Only emojis: big, the time in a pill under their right end.
        let big = cell.filter(|_| m.quote.is_empty()).and_then(|c| crate::bigemoji::cells(&m.text, c));
        if let Some((ew, eh)) = big.filter(|&(ew, _)| ew as usize + 4 <= width) {
            starts.push((m.id.clone(), out.len()));
            if group && !m.from_me && first {
                let name = Style::new().fg(name_color).add_modifier(Modifier::BOLD);
                out.push(Line::from(vec![Span::raw("  "), Span::styled(truncate(&m.sender_name, inner), name)]));
            }
            let mut stamp = vec![Span::styled(format!(" {}", t.format("%H:%M")), Style::new().fg(META_IN).bg(DAY_PILL))];
            // Big emojis only come with pictures: the ticks are one, over blanks.
            let tick_col = row_width(&stamp);
            if m.from_me {
                stamp.push(Span::styled(" ".repeat(crate::ticks::CELLS as usize), Style::new().bg(DAY_PILL)));
            }
            stamp.push(Span::styled(" ", Style::new().bg(DAY_PILL)));
            let pw = row_width(&stamp) + 2;
            let ew = ew as usize;
            let (ex, px) = if m.from_me { (width - 2 - ew, width.saturating_sub(2 + pw)) } else { (2, 2 + ew.saturating_sub(pw)) };
            bigs.push((out.len(), ex, (ew as u16, eh), m.text.clone()));
            out.extend((0..eh).map(|_| Line::default()));
            if m.from_me {
                decos.push((out.len(), px + 1 + tick_col, Deco::Ticks(m.status, ticks(m.status).1)));
            }
            let mut spans = vec![Span::raw(" ".repeat(px)), Span::styled(CAP_L, Style::new().fg(DAY_PILL))];
            spans.extend(stamp);
            spans.push(Span::styled(CAP_R, Style::new().fg(DAY_PILL)));
            out.push(Line::from(spans));
            prev_sender = Some(m.sender.as_str());
            continue;
        }

        let (bg, meta_fg) = if m.from_me { (BUBBLE_OUT, META_OUT) } else { (BUBBLE_IN, META_IN) };
        let body = Style::new().fg(TEXT).bg(bg);
        let meta = Style::new().fg(meta_fg).bg(bg);

        // The bubble's content, row by row.
        let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
        if group && !m.from_me && first {
            rows.push(vec![Span::styled(truncate(&m.sender_name, inner), Style::new().fg(name_color).bg(bg).add_modifier(Modifier::BOLD))]);
        }
        if !m.quote.is_empty() {
            let mut q = vec![Span::styled("▎", Style::new().fg(GREEN).bg(bg))];
            q.extend(crate::markup::line(&m.quote, inner.saturating_sub(2), meta.add_modifier(Modifier::ITALIC)));
            rows.push(q);
        }
        // Links underlined in their color, on every row they wrap onto.
        let link = Style::new().fg(if m.from_me { LINK_OUT } else { LINK_IN }).bg(bg).add_modifier(Modifier::UNDERLINED);
        let mut pieces = Vec::new();
        // An audio is a player, not its text.
        let audio = audio_of(m);
        let mut player = None;
        let n_text = if let Some(a) = &audio {
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
            let looks = crate::markup::Looks { base: body, link, shade: if m.from_me { CODE_OUT } else { CODE_IN }, dim: meta_fg };
            let (text, found) = crate::markup::rows(&m.text, inner, &looks);
            pieces.extend(found.into_iter().map(|(r, c, w, url)| (rows.len() + r, c, w, url)));
            let n_text = text.len();
            rows.extend(text);
            n_text
        };

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
        let inline = n_text > 0 && last_w + 2 + footer_w <= inner;
        let content_w = rows
            .iter()
            .map(|r| row_width(r))
            .chain([if inline { last_w + 2 + footer_w } else { footer_w }])
            .max()
            .unwrap_or(0);
        let bw = content_w + 2;
        // A column kept free on the tail's side.
        let x = if m.from_me { width.saturating_sub(bw + 2) } else { 2 };
        if !inline {
            let mut r = vec![Span::styled(" ".repeat(content_w - footer_w), body)];
            r.extend(footer.clone());
            rows.push(r);
        }

        let footer_row = rows.len() - 1;
        if audio.is_some() {
            rows.push(Vec::new());
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
            } else {
                r.push(Span::styled(" ".repeat(content_w - used), body));
            }
            spans.extend(r);
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
            if m.from_me {
                decos.push((top + n - 1, x + 1 + content_w - crate::ticks::CELLS as usize, Deco::Ticks(m.status, tick_color)));
            }
            if first {
                let col = if m.from_me { x + bw } else { x - 1 };
                decos.push((top, col, Deco::Tail(!m.from_me, bg)));
            }
        }
        link_cells.extend(pieces.into_iter().map(|(r, c, w, url)| (top + r, x + 1 + c, w, url)));
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
        prev_sender = Some(m.sender.as_str());
    }
    (out, decos, starts, bigs, link_cells, audios)
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
                let (word, color) = if o.open { ("Aberta", GREEN) } else { ("Fechada", CLOSED) };
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
                let green = |lines: &mut Vec<Line<'static>>, t: &str| {
                    for l in wrap(t, text_w) {
                        lines.push(Line::from(Span::styled(format!("  {l}"), Style::new().fg(GREEN))));
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
                        (true, _) => ("Aberta 24 horas", GREEN),
                        (_, true) => ("Aberta agora", GREEN),
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
                        lines.push(Line::from(Span::styled(format!("  {PIN} Abrir no Google Maps"), Style::new().fg(GREEN))));
                        maps_rows = Some((from, lines.len()));
                    }
                }
                if !d.email.is_empty() {
                    gap(&mut lines);
                    green(&mut lines, &d.email);
                }
                for site in &d.website {
                    gap(&mut lines);
                    green(&mut lines, site);
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
                        top.extend(pill(tag.into(), CHIP_ON_TEXT, CHIP_ON));
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
    let looks = crate::markup::Looks { base: Style::new().fg(TEXT), link: Style::new().fg(GREEN), shade: CODE_IN, dim: META_IN };
    for row in crate::markup::rows(text, width, &looks).0 {
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
        let Ok(p) = picker.new_protocol(img, Rect::new(0, 0, rect.width, rect.height).into(), Resize::Fit(None)) else { return };
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

fn row_width(r: &[Span]) -> usize {
    r.iter().map(|s| s.content.width()).sum()
}

fn status_bar(f: &mut Frame, app: &App, area: Rect) {
    if let Some((text, _)) = &app.notice {
        return f.render_widget(Paragraph::new(format!(" {text}")).red(), area);
    }
    let (send, newline) = if app.settings.on(crate::config::ENTER_SENDS) { ("Enter", "Alt+Enter") } else { ("Alt+Enter", "Enter") };
    let hints: String = match app.focus {
        _ if app.account.menu.is_some() || app.account.settings.is_some() => " ↑↓ move · Enter escolhe · Esc fecha".into(),
        _ if app.account.about => " Enter abre o link · Esc fecha".into(),
        _ if app.account.profile.is_some() => " Esc fecha".into(),
        Focus::Input if app.emoji_panel.open => " digite para buscar · ←↑↓→ escolhe · Enter insere · Tab categorias · Esc fecha".into(),
        Focus::Sidebar => " ↑↓ conversas · Enter abre · / busca ou número · 1 2 3 filtros · i dados · Espaço áudio · Tab mensagem · Alt+S menu · q sai".into(),
        Focus::Search => " digite para filtrar · ↑↓ escolhe · Enter abre · Esc limpa".into(),
        Focus::Input => format!(" {send} envia · {newline} quebra linha · Alt+E emojis · Alt+A anexa · Ctrl+B negrito · Ctrl+I itálico · Ctrl+V cola · PgUp/PgDn rola · Esc conversas · Alt+S menu · Ctrl+C sai"),
    };
    f.render_widget(Paragraph::new(hints).style(dim()), area);
}

fn local(ts: i64) -> DateTime<Local> {
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

fn day_label(t: DateTime<Local>) -> String {
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
