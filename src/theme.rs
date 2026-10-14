//! The greens follow the terminal's theme: its green (color 2) is
//! WhatsApp's accent, and the shades of your bubbles are that green over
//! its background. Omarchy's theme is read from its files and followed as
//! it changes; elsewhere the terminal is asked once, at the start. Without
//! either, WhatsApp Web's own greens.

use ratatui::style::Color;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// 0xRRGGBB; NONE until the theme is known.
static GREEN: AtomicU32 = AtomicU32::new(NONE);
static BG: AtomicU32 = AtomicU32::new(NONE);
static FG: AtomicU32 = AtomicU32::new(NONE);
const NONE: u32 = u32::MAX;
const WHITE: u32 = 0xFFFFFF;

fn rgb(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// `a` with `t` of `b` over it.
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 0xFF) as f32, ((b >> s) & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// The theme's green and background, when known.
fn theme() -> Option<(u32, u32)> {
    let (g, bg) = (GREEN.load(Ordering::Relaxed), BG.load(Ordering::Relaxed));
    (g != NONE && bg != NONE).then_some((g, bg))
}

/// The terminal's text and background colors, as RGB, when known: the
/// QR code's light and dark modules.
pub fn fg_bg() -> Option<([u8; 3], [u8; 3])> {
    let (fg, bg) = (FG.load(Ordering::Relaxed), BG.load(Ordering::Relaxed));
    let px = |c: u32| [(c >> 16) as u8, (c >> 8) as u8, c as u8];
    (fg != NONE && bg != NONE).then(|| (px(fg), px(bg)))
}

/// A shade from the theme, or WhatsApp Web's own without one.
fn shade(wa: u32, f: impl Fn(u32, u32) -> u32) -> Color {
    rgb(theme().map_or(wa, |(g, bg)| f(g, bg)))
}

/// The accent: names, unread counts, buttons, what's on.
pub fn green() -> Color {
    shade(0x25D366, |g, _| g)
}

/// Behind a message just gone to, as strong as `k` (1 at first, fading to 0).
pub fn glow(k: f32) -> Color {
    let (g, bg) = theme().unwrap_or((0x25D366, 0x161717));
    rgb(mix(bg, g, 0.22 * k.clamp(0.0, 1.0)))
}

/// Behind text selected with the mouse: stronger than your bubbles.
pub fn selection() -> Color {
    shade(0x2A7A5A, |g, bg| mix(bg, g, 0.6))
}

/// Your bubbles.
pub fn bubble_out() -> Color {
    shade(0x144D37, |g, bg| mix(bg, g, 0.3))
}

/// Code and quotes in your bubbles: darker than them.
pub fn code_out() -> Color {
    shade(0x0E3A29, |g, bg| mix(bg, g, 0.2))
}

/// The rule over a business's buttons, in your bubbles.
pub fn rule_out() -> Color {
    shade(0x2B604B, |g, bg| mix(bg, g, 0.42))
}

/// The time and the ticks in your bubbles.
pub fn meta_out() -> Color {
    shade(0xA1B8AF, |g, bg| mix(mix(bg, g, 0.3), WHITE, 0.6))
}

/// Links in your bubbles.
pub fn link_out() -> Color {
    shade(0x7FD8A8, |g, _| mix(g, WHITE, 0.45))
}

/// A filter chip that's on, and its text.
pub fn chip_on() -> Color {
    shade(0x103529, |g, bg| mix(bg, g, 0.2))
}

pub fn chip_on_text() -> Color {
    shade(0xD9FDD3, |g, _| mix(g, WHITE, 0.75))
}

fn hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    (s.len() == 6).then(|| u32::from_str_radix(s, 16).ok()).flatten()
}

/// Omarchy's current theme, as its ghostty.conf has it.
fn omarchy_conf() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [".local/state/omarchy/current/theme/ghostty.conf", ".config/omarchy/current/theme/ghostty.conf"]
        .into_iter()
        .map(|p| home.join(p))
        .find(|p| p.is_file())
}

/// Its green (palette 2) and background; its foreground kept.
fn read_conf(path: &PathBuf) -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(path).ok()?;
    let (mut g, mut bg) = (None, None);
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "background" => bg = hex(v),
            "foreground" => {
                if let Some(c) = hex(v) {
                    FG.store(c, Ordering::Relaxed);
                }
            }
            "palette" => {
                if let Some(c) = v.trim().strip_prefix("2=") {
                    g = hex(c);
                }
            }
            _ => {}
        }
    }
    Some((g?, bg?))
}

fn set(g: u32, bg: u32) {
    GREEN.store(g, Ordering::Relaxed);
    BG.store(bg, Ordering::Relaxed);
}

/// Keeps up with Omarchy's theme: `poll` reads it again when its file changes.
pub struct Watch {
    path: Option<PathBuf>,
    stamp: Option<SystemTime>,
    at: Instant,
}

impl Watch {
    /// At the start, before anything else reads input: Omarchy's theme, or
    /// the terminal asked for its colors.
    pub fn start() -> Watch {
        let mut w = Watch { path: None, stamp: None, at: Instant::now() };
        if !w.reload() {
            if let Some((g, bg, fg)) = ask_terminal() {
                set(g, bg);
                if let Some(fg) = fg {
                    FG.store(fg, Ordering::Relaxed);
                }
            }
        }
        w
    }

    fn reload(&mut self) -> bool {
        let Some(path) = omarchy_conf() else { return false };
        // Through the links: a new theme is a new file.
        self.stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let read = read_conf(&path);
        self.path = Some(path);
        match read {
            Some((g, bg)) => {
                set(g, bg);
                true
            }
            None => false,
        }
    }

    /// Every second, whether the theme changed; true when it did.
    pub fn poll(&mut self) -> bool {
        if self.path.is_none() || self.at.elapsed() < Duration::from_secs(1) {
            return false;
        }
        self.at = Instant::now();
        let path = omarchy_conf();
        let stamp = path.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        if path == self.path && stamp == self.stamp {
            return false;
        }
        let before = theme();
        self.reload();
        theme() != before
    }
}

/// Asks the terminal for its color 2, its text and background (OSC 4, 10 and 11),
/// then for its attributes (DA1), whose answer every terminal gives: the
/// end of the replies. Read straight off stdin, in raw mode.
fn ask_terminal() -> Option<(u32, u32, Option<u32>)> {
    use std::io::{Read, Write};
    let mut out = std::io::stdout();
    out.write_all(b"\x1b]4;2;?\x1b\\\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[c").ok()?;
    out.flush().ok()?;
    let mut got = Vec::new();
    let end = Instant::now() + Duration::from_millis(300);
    let mut stdin = std::io::stdin();
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd, for as long as the call.
        if unsafe { libc::poll(&mut pfd, 1, left.as_millis() as i32) } <= 0 {
            break;
        }
        let mut buf = [0u8; 256];
        let n = stdin.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        got.extend_from_slice(&buf[..n]);
        // DA1's answer: ESC [ ? … c.
        if let Some(i) = find(&got, b"\x1b[?") {
            if got[i..].contains(&b'c') {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&got);
    let color = |key: &str| {
        let at = text.find(key)? + key.len();
        let rest = &text[at..];
        let spec: String = rest.chars().take_while(|c| c.is_ascii_hexdigit() || *c == '/').collect();
        let parts: Vec<&str> = spec.split('/').collect();
        if parts.len() != 3 {
            return None;
        }
        // Each channel 1 to 4 hex digits: its top byte.
        let byte = |p: &str| {
            let v = u32::from_str_radix(p, 16).ok()?;
            let bits = 4 * p.len() as u32;
            Some(if bits >= 8 { v >> (bits - 8) } else { v * 0x11 })
        };
        Some((byte(parts[0])? << 16) | (byte(parts[1])? << 8) | byte(parts[2])?)
    };
    Some((color("4;2;rgb:")?, color("11;rgb:")?, color("10;rgb:")))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
