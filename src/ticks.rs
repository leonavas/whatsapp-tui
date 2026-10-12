//! WhatsApp's message status as small pictures: a clock while it's on its
//! way, one check sent, two delivered, two in blue read.

use image::{DynamicImage, Rgba, RgbaImage};

/// Cells the picture spans, on one row; the same for every status so the
/// time doesn't shift as it changes.
pub const CELLS: u16 = 2;

/// Strokes in WhatsApp's 16×11 icon box: segments, and the clock's ring.
fn strokes(status: u8) -> (Vec<[f32; 4]>, Option<(f32, f32, f32)>) {
    match status {
        0 => (vec![[8.0, 5.5, 8.0, 2.6], [8.0, 5.5, 10.2, 6.6]], Some((8.0, 5.5, 4.6))),
        1 => (vec![[2.5, 5.8, 5.9, 9.2], [5.9, 9.2, 12.7, 1.6]], None),
        // The second check's short arm stops at the first's long one.
        _ => (vec![[0.9, 5.8, 4.3, 9.2], [4.3, 9.2, 11.1, 1.6], [7.3, 7.9, 8.6, 9.2], [8.6, 9.2, 15.3, 1.6]], None),
    }
}

fn segment_dist(px: f32, py: f32, [x0, y0, x1, y1]: [f32; 4]) -> f32 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let t = (((px - x0) * dx + (py - y0) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((px - x0 - t * dx).powi(2) + (py - y0 - t * dy).powi(2)).sqrt()
}

/// The ticks for `status` in `color`, over `w`×`h` pixels (CELLS cells of
/// one row), see-through around the strokes so they sit on any bubble.
pub fn picture(status: u8, color: [u8; 3], w: u32, h: u32) -> DynamicImage {
    let (wf, hf) = (w as f32, h as f32);
    // About the digits' height, centered on them.
    let scale = (hf * 0.42 / 11.0).min(wf * 0.8 / 16.0);
    let (ox, oy) = (wf * 0.2, hf * 0.54 - 5.5 * scale);
    let half = (1.4 * scale).max(1.1) / 2.0;
    let (segs, ring) = strokes(status);
    let mut img = RgbaImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        // Back into the icon's box, in its units.
        let (ix, iy) = ((x as f32 + 0.5 - ox) / scale, (y as f32 + 0.5 - oy) / scale);
        let mut d = segs.iter().map(|&s| segment_dist(ix, iy, s)).fold(f32::MAX, f32::min);
        if let Some((cx, cy, r)) = ring {
            d = d.min((((ix - cx).powi(2) + (iy - cy).powi(2)).sqrt() - r).abs());
        }
        let a = (half - d * scale + 0.5).clamp(0.0, 1.0);
        *p = Rgba([color[0], color[1], color[2], (255.0 * a) as u8]);
    }
    DynamicImage::ImageRgba8(img)
}
