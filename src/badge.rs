//! The unread count over a picture's top-right in the narrow list, as
//! WhatsApp's phone app shows it: a green circle (a pill for two digits or
//! more) with a ring cut around it, so the row's background shows between
//! it and the photo, and in the digits. The digits are the font's own
//! outlines, filled here.

use image::{Rgba, RgbaImage};
use rustybuzz::ttf_parser::{Face, GlyphId, OutlineBuilder};
use std::sync::OnceLock;

/// Bold monospace, as the mockup's; else whatever fontconfig picks.
const BOLD: [&str; 3] = [
    "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Bold.ttf",
    "/usr/share/fonts/TTF/JetBrainsMono-Bold.ttf",
    "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Bold.ttf",
];

/// Sub-rows a pixel is sampled at; across, the spans are exact.
const SUB: usize = 5;
/// Straight pieces a curve becomes.
const STEPS: usize = 6;

fn fc_match(pattern: &str) -> Option<String> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    String::from_utf8(out.stdout).ok().filter(|p| !p.is_empty())
}

fn load(known: &[&str], pattern: &str) -> Option<Vec<u8>> {
    let picked = std::iter::once_with(|| fc_match(pattern)).flatten();
    known.iter().map(|p| p.to_string()).chain(picked).find_map(|p| std::fs::read(p).ok())
}

/// The digits' font, read once.
fn digits() -> Option<Face<'static>> {
    static FONT: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    Face::parse(FONT.get_or_init(|| load(&BOLD, "monospace:bold")).as_deref()?, 0).ok()
}

/// The terminal's icons' font (a Nerd Font's): the first one asked for `c` has, read once.
fn icons(c: char) -> Option<Face<'static>> {
    static FONT: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    let data = FONT.get_or_init(|| load(&[], &format!("monospace:charset={:x}", c as u32)));
    Face::parse(data.as_deref()?, 0).ok().filter(|f| f.glyph_index(c).is_some())
}

type Edge = (f32, f32, f32, f32);

/// A glyph's outline as straight edges, in pixels, y down.
struct Edges {
    lines: Vec<Edge>,
    at: (f32, f32),
    start: (f32, f32),
    scale: f32,
    origin: (f32, f32),
}

impl Edges {
    fn pt(&self, x: f32, y: f32) -> (f32, f32) {
        (self.origin.0 + x * self.scale, self.origin.1 - y * self.scale)
    }

    fn to(&mut self, p: (f32, f32)) {
        self.lines.push((self.at.0, self.at.1, p.0, p.1));
        self.at = p;
    }

    /// A curve through `f(t)`, t from 0 to 1, in straight pieces.
    fn curve(&mut self, f: impl Fn(f32) -> (f32, f32)) {
        for i in 1..=STEPS {
            self.to(f(i as f32 / STEPS as f32));
        }
    }
}

impl OutlineBuilder for Edges {
    fn move_to(&mut self, x: f32, y: f32) {
        self.at = self.pt(x, y);
        self.start = self.at;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.to(p);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, b, c) = (self.at, self.pt(x1, y1), self.pt(x, y));
        self.curve(|t| {
            let u = 1.0 - t;
            (u * u * a.0 + 2.0 * u * t * b.0 + t * t * c.0, u * u * a.1 + 2.0 * u * t * b.1 + t * t * c.1)
        });
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b, c, d) = (self.at, self.pt(x1, y1), self.pt(x2, y2), self.pt(x, y));
        self.curve(|t| {
            let u = 1.0 - t;
            let (k0, k1, k2, k3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            (k0 * a.0 + k1 * b.0 + k2 * c.0 + k3 * d.0, k0 * a.1 + k1 * b.1 + k2 * c.1 + k3 * d.1)
        });
    }

    fn close(&mut self) {
        if self.at != self.start {
            self.to(self.start);
        }
    }
}

/// How much of each pixel of a `w` × `h` image the edges' inside covers
/// (nonzero winding).
fn coverage(lines: &[Edge], w: u32, h: u32) -> Vec<f32> {
    let mut cov = vec![0.0; (w * h) as usize];
    let top = lines.iter().map(|l| l.1.min(l.3)).fold(f32::MAX, f32::min).max(0.0) as u32;
    let bottom = (lines.iter().map(|l| l.1.max(l.3)).fold(0.0, f32::max).ceil() as u32).min(h);
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in top..bottom {
        for k in 0..SUB {
            let y = row as f32 + (k as f32 + 0.5) / SUB as f32;
            xs.clear();
            for &(x0, y0, x1, y1) in lines {
                if (y0 <= y) != (y1 <= y) {
                    xs.push((x0 + (y - y0) / (y1 - y0) * (x1 - x0), if y1 > y0 { 1 } else { -1 }));
                }
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (mut wind, mut from) = (0, 0.0);
            for &(x, dir) in &xs {
                let was = wind;
                wind += dir;
                if was == 0 {
                    from = x;
                } else if wind == 0 {
                    // The span [from, x), each pixel by how much of it it covers.
                    let (a, b) = (from.max(0.0), x.min(w as f32));
                    for px in a.floor() as u32..(b.ceil() as u32).min(w) {
                        let part = b.min(px as f32 + 1.0) - a.max(px as f32);
                        cov[(row * w + px) as usize] += part.max(0.0) / SUB as f32;
                    }
                }
            }
        }
    }
    cov
}

/// The glyphs of `s` side by side, at their pen positions in font units,
/// and their ink's box (x0, y0, x1, y1), y up.
fn layout(face: &Face, s: &str) -> Option<(Vec<(GlyphId, f32)>, [f32; 4])> {
    let (mut pen, mut ink) = (0.0, [f32::MAX, f32::MAX, f32::MIN, f32::MIN]);
    let mut glyphs = Vec::new();
    for c in s.chars() {
        let id = face.glyph_index(c)?;
        if let Some(b) = face.glyph_bounding_box(id) {
            ink = [ink[0].min(pen + b.x_min as f32), ink[1].min(b.y_min as f32), ink[2].max(pen + b.x_max as f32), ink[3].max(b.y_max as f32)];
        }
        glyphs.push((id, pen));
        pen += face.glyph_hor_advance(id)? as f32;
    }
    (ink[0] <= ink[2]).then_some((glyphs, ink))
}

/// The glyphs' edges at `scale`, their pen's origin at `origin` (pixels).
fn outline(face: &Face, glyphs: &[(GlyphId, f32)], scale: f32, origin: (f32, f32)) -> Vec<Edge> {
    let mut e = Edges { lines: Vec::new(), at: (0.0, 0.0), start: (0.0, 0.0), scale, origin };
    for &(id, pen) in glyphs {
        e.origin = (origin.0 + pen * scale, origin.1);
        face.outline_glyph(id, &mut e);
    }
    e.lines
}

/// `color` over the pixel, `a` of it.
fn over(p: &mut Rgba<u8>, color: [u8; 3], a: f32) {
    let below = p[3] as f32 / 255.0 * (1.0 - a);
    let out = a + below;
    if out <= 0.0 {
        return;
    }
    for i in 0..3 {
        p[i] = ((color[i] as f32 * a + p[i] as f32 * below) / out).round() as u8;
    }
    p[3] = (out * 255.0).round() as u8;
}

/// The badge `h` pixels tall over `img`, about (cx, cy) and kept inside
/// it: a circle, or for two characters or more a pill as wide as they are
/// and a bit; just the circle with no `label` (marked as unread). A ring
/// `ring` wide around it and the digits are cut out of the picture.
pub fn stamp(img: &mut RgbaImage, label: &str, color: [u8; 3], (cx, cy): (f32, f32), h: f32, ring: f32) {
    let (w, hi) = (img.width() as f32, img.height() as f32);
    let size = h * 0.62;
    let face = digits().filter(|_| !label.is_empty());
    let text = face.as_ref().and_then(|f| layout(f, label).map(|(g, ink)| (f, g, ink, size / f.units_per_em() as f32)));
    let long = label.chars().count() > 1;
    let bw = match &text {
        Some((_, _, ink, scale)) if long => ((ink[2] - ink[0]) * scale + h * 0.62).max(h),
        _ => h,
    };
    let cx = cx.min(w - ring - bw / 2.0).max(ring + bw / 2.0);
    let cy = cy.max(ring + h / 2.0).min(hi - ring - h / 2.0);
    // The digits' ink centered, the pen on whole pixels for crisp edges.
    let cover = text.map_or(Vec::new(), |(f, glyphs, ink, scale)| {
        let origin = ((cx - (ink[0] + ink[2]) / 2.0 * scale).round(), (cy + (ink[1] + ink[3]) / 2.0 * scale).round());
        coverage(&outline(f, &glyphs, scale, origin), img.width(), img.height())
    });
    let (r, half) = (h / 2.0, (bw - h) / 2.0);
    let (x0, x1) = (((cx - bw / 2.0 - ring - 1.0).max(0.0)) as u32, ((cx + bw / 2.0 + ring + 1.0).ceil() as u32).min(img.width()));
    let (y0, y1) = (((cy - r - ring - 1.0).max(0.0)) as u32, ((cy + r + ring + 1.0).ceil() as u32).min(img.height()));
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // How far from the pill's spine, a segment across its middle.
            let d = ((px - px.clamp(cx - half, cx + half)).powi(2) + (py - cy).powi(2)).sqrt();
            let fill = (r - d + 0.5).clamp(0.0, 1.0);
            let cut = (r + ring - d + 0.5).clamp(0.0, 1.0);
            let ink = cover.get((y * img.width() + x) as usize).map_or(0.0, |c| c.min(1.0));
            let p = img.get_pixel_mut(x, y);
            p[3] = (p[3] as f32 * (1.0 - cut)).round() as u8;
            over(p, color, fill * (1.0 - ink));
        }
    }
}

/// The icon `c` of the terminal's Nerd Font in `color`, its ink `tall`
/// pixels high (at most `wide` across), from `x` and centered on `cy`, as
/// Ghostty draws it; its ink's box (x0, y0, x1, y1), or None without it.
pub fn icon(img: &mut RgbaImage, c: char, color: [u8; 3], x: f32, cy: f32, tall: f32, wide: f32) -> Option<[f32; 4]> {
    let face = icons(c)?;
    let (glyphs, ink) = layout(&face, &c.to_string())?;
    let (iw, ih) = (ink[2] - ink[0], ink[3] - ink[1]);
    let scale = (tall / ih).min(wide / iw);
    let origin = ((x - ink[0] * scale).round(), (cy + (ink[1] + ink[3]) / 2.0 * scale).round());
    let cov = coverage(&outline(&face, &glyphs, scale, origin), img.width(), img.height());
    for (p, a) in img.pixels_mut().zip(cov) {
        if a > 0.0 {
            over(p, color, a.min(1.0));
        }
    }
    Some([origin.0 + ink[0] * scale, origin.1 - ink[3] * scale, origin.0 + ink[2] * scale, origin.1 - ink[1] * scale])
}
