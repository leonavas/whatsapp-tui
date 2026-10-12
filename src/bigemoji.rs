//! A message of only emojis, one to three, shown big with no bubble, as
//! WhatsApp does. The terminal can't scale text, so they're a picture: each
//! one's bitmap from the color emoji font (Noto's are PNGs, in CBDT).

use crate::app::App;
use image::{imageops, DynamicImage, RgbaImage};
use ratatui::layout::Rect;
use ratatui::Frame;
use ratatui_image::{Image, Resize};
use rustybuzz::ttf_parser::{GlyphId, RasterImageFormat};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use unicode_segmentation::UnicodeSegmentation;

/// How tall the emojis stand, in rows.
const ROWS: u16 = 3;

const FONTS: [&str; 3] = [
    "/usr/share/fonts/noto/NotoColorEmoji.ttf",
    "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
    "/usr/share/fonts/google-noto-color-emoji-fonts/NotoColorEmoji.ttf",
];

/// Each text's bitmaps, or None when the font can't draw all of it.
static GLYPHS: Mutex<Option<HashMap<String, Option<Vec<RgbaImage>>>>> = Mutex::new(None);

/// Read once, on the first such message; whatever fontconfig picks if
/// Noto's isn't where distros put it.
fn font() -> Option<&'static [u8]> {
    static FONT: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    FONT.get_or_init(|| {
        let picked = std::process::Command::new("fc-match")
            .args(["-f", "%{file}", "emoji"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok());
        FONTS.iter().map(|p| p.to_string()).chain(picked).find_map(|p| std::fs::read(p).ok())
    })
    .as_deref()
}

/// The emojis, when that's all there is besides whitespace.
fn split(text: &str) -> Option<Vec<&str>> {
    // The longest emoji, a kiss in two skin tones, is 35 bytes.
    if text.len() > 160 {
        return None;
    }
    let all: Vec<&str> = text.graphemes(true).filter(|g| !g.trim().is_empty()).collect();
    let emoji = |g: &str| emojis::get(g).is_some() || emojis::get(&g.replace('\u{fe0f}', "")).is_some();
    ((1..=3).contains(&all.len()) && all.iter().all(|g| emoji(g))).then_some(all)
}

/// Shaped, so ZWJ sequences, flags, keycaps and skin tones become their one
/// glyph; one the font has no single picture for leaves the message as text.
fn rasterize(text: &str) -> Option<Vec<RgbaImage>> {
    let face = rustybuzz::Face::from_slice(font()?, 0)?;
    split(text)?
        .into_iter()
        .map(|g| {
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(g);
            let shaped = rustybuzz::shape(&face, &[], buf);
            // Variation selectors come out as empty glyphs with no advance.
            let mut pics = shaped
                .glyph_infos()
                .iter()
                .zip(shaped.glyph_positions())
                .filter(|(_, p)| p.x_advance > 0)
                .filter_map(|(i, _)| face.glyph_raster_image(GlyphId(i.glyph_id as u16), 128));
            let (Some(pic), None) = (pics.next(), pics.next()) else { return None };
            if pic.format != RasterImageFormat::PNG {
                return None;
            }
            image::load_from_memory_with_format(pic.data, image::ImageFormat::Png).ok().map(|i| i.to_rgba8())
        })
        .collect()
}

/// The size in cells of the message's emojis shown big, each about square,
/// side by side; None when it's not only emojis or the font lacks one.
pub fn cells(text: &str, cell: (u32, u32)) -> Option<(u16, u16)> {
    split(text)?;
    let mut cache = GLYPHS.lock().ok()?;
    let n = cache.get_or_insert_with(HashMap::new).entry(text.to_string()).or_insert_with(|| rasterize(text)).as_ref()?.len();
    let each = ((ROWS as u32 * cell.1) as f64 / cell.0 as f64).round() as u16;
    Some((each * n as u16, ROWS))
}

/// The emojis over `rect`, each fit to its share of it.
fn picture(text: &str, w: u32, h: u32) -> Option<DynamicImage> {
    let cache = GLYPHS.lock().ok()?;
    let glyphs = cache.as_ref()?.get(text)?.as_ref()?;
    let slot = w / glyphs.len() as u32;
    let mut out = RgbaImage::new(w, h);
    for (i, g) in glyphs.iter().enumerate() {
        let scale = (slot as f32 / g.width() as f32).min(h as f32 / g.height() as f32);
        let (gw, gh) = ((g.width() as f32 * scale) as u32, (g.height() as f32 * scale) as u32);
        let g = imageops::resize(g, gw.max(1), gh.max(1), imageops::FilterType::Lanczos3);
        imageops::overlay(&mut out, &g, (i as u32 * slot + (slot - gw) / 2) as i64, ((h - gh) / 2) as i64);
    }
    Some(DynamicImage::ImageRgba8(out))
}

pub fn draw(f: &mut Frame, app: &mut App, text: &str, rect: Rect) {
    let Some(picker) = app.picker.as_ref() else { return };
    let key = (format!("emoji:{text}"), rect.width, rect.height);
    if !app.protocols.contains_key(&key) {
        let s = picker.font_size();
        let Some(img) = picture(text, rect.width as u32 * s.width as u32, rect.height as u32 * s.height as u32) else { return };
        let Ok(p) = picker.new_protocol(img, Rect::new(0, 0, rect.width, rect.height).into(), Resize::Fit(None)) else { return };
        app.protocols.insert(key.clone(), p);
    }
    f.render_widget(Image::new(&app.protocols[&key]), rect);
}
