//! Profile pictures: round, as WhatsApp shows them, and the gray silhouette
//! (a person, or two for a group) for whoever has none.

use image::{imageops::FilterType, DynamicImage, Rgba, RgbaImage};

const BG: [u8; 3] = [0x6A, 0x7B, 0x84];
const FG: [u8; 3] = [0xCF, 0xD4, 0xD6];

/// How much of a pixel lies inside a circle, for smooth edges.
fn in_circle(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

fn in_ellipse(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> f32 {
    let d = (((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2)).sqrt();
    ((1.0 - d) * rx.min(ry) + 0.5).clamp(0.0, 1.0)
}

/// The picture cut to a circle `side` pixels across.
pub fn round(img: &DynamicImage, side: u32) -> DynamicImage {
    let mut img = img.resize_to_fill(side, side, FilterType::Triangle).to_rgba8();
    let r = side as f32 / 2.0;
    for (x, y, p) in img.enumerate_pixels_mut() {
        p[3] = (p[3] as f32 * in_circle(x as f32 + 0.5, y as f32 + 0.5, r, r, r)) as u8;
    }
    DynamicImage::ImageRgba8(img)
}

/// A head and shoulders at (cx, cy) as shares of the side, `scale` of full size.
fn person(x: f32, y: f32, s: f32, cx: f32, scale: f32) -> f32 {
    let head = in_circle(x, y, cx * s, 0.40 * s, 0.17 * s * scale);
    let body = in_ellipse(x, y, cx * s, (0.40 + 0.56 * scale) * s, 0.33 * s * scale, 0.30 * s * scale);
    head.max(body)
}

/// WhatsApp's default picture: a silhouette in a gray circle.
pub fn silhouette(side: u32, group: bool) -> DynamicImage {
    let s = side as f32;
    let r = s / 2.0;
    let mut img = RgbaImage::new(side, side);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let shape = if group { person(px, py, s, 0.62, 0.78).max(person(px, py, s, 0.40, 0.9)) } else { person(px, py, s, 0.5, 1.0) };
        let mix = |i: usize| (BG[i] as f32 + (FG[i] as f32 - BG[i] as f32) * shape) as u8;
        *p = Rgba([mix(0), mix(1), mix(2), (255.0 * in_circle(px, py, r, r, r)) as u8]);
    }
    DynamicImage::ImageRgba8(img)
}

/// The official account's picture: WhatsApp's logo, white on green, as
/// WhatsApp Web shows it.
pub fn official(side: u32) -> DynamicImage {
    let logo = image::load_from_memory(include_bytes!("../assets/official.png")).expect("official.png");
    logo.resize_exact(side, side, image::imageops::FilterType::CatmullRom)
}

fn in_rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    ((x - x0).min(x1 - x).min(y - y0).min(y1 - y) + 0.5).clamp(0.0, 1.0)
}

/// An audio file's picture: a white headphone on WhatsApp's orange circle.
pub fn headphones(side: u32) -> DynamicImage {
    let s = side as f32;
    let r = s / 2.0;
    let mut img = RgbaImage::new(side, side);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        // The band over the top, an arc down to the cups at its ends.
        let (cx, cy) = (r, 0.54 * s);
        let band = if py <= cy { in_circle(px, py, cx, cy, 0.25 * s) * (1.0 - in_circle(px, py, cx, cy, 0.20 * s)) } else { 0.0 };
        let cups = in_rect(px, py, 0.25 * s, 0.50 * s, 0.37 * s, 0.73 * s).max(in_rect(px, py, 0.63 * s, 0.50 * s, 0.75 * s, 0.73 * s));
        let white = band.max(cups);
        let mix = |c: u8| (c as f32 + (255.0 - c as f32) * white) as u8;
        *p = Rgba([mix(0xFF), mix(0x9F), mix(0x1C), (255.0 * in_circle(px, py, r, r, r)) as u8]);
    }
    DynamicImage::ImageRgba8(img)
}
