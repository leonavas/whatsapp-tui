//! Links in message text, as WhatsApp finds them: http(s), www. and e-mails.

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::Rect;
use std::num::NonZeroU16;
use unicode_width::UnicodeWidthStr;

/// Each link as (first char, past its last char, what opens it).
pub fn find(text: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let mut end = i;
        while end < chars.len() && !chars[end].is_whitespace() {
            end += 1;
        }
        let (mut a, mut b) = (i, end);
        i = end;
        while a < b && "([{<\"'*_~".contains(chars[a]) {
            a += 1;
        }
        // A closing paren stays when the link opened one, as in Wikipedia's.
        while a < b {
            let c = chars[b - 1];
            let open = chars[a..b].iter().filter(|&&c| c == '(').count();
            let close = chars[a..b].iter().filter(|&&c| c == ')').count();
            if ".,;:!?]}>\"'*_~".contains(c) || (c == ')' && close > open) {
                b -= 1;
            } else {
                break;
            }
        }
        let word: String = chars[a..b].iter().collect();
        let lower = word.to_lowercase();
        // Control chars would break out of the OSC 8 escape.
        let target = if word.chars().any(char::is_control) {
            continue;
        } else if ["http://", "https://"].iter().any(|s| lower.starts_with(s) && lower.len() > s.len()) {
            word
        } else if lower.starts_with("www.") && lower[4..].contains('.') {
            format!("https://{word}")
        } else if email(&word) {
            format!("mailto:{word}")
        } else {
            continue;
        };
        out.push((a, b, target));
    }
    out
}

fn email(s: &str) -> bool {
    let Some((user, domain)) = s.split_once('@') else { return false };
    let labels: Vec<&str> = domain.split('.').collect();
    !user.is_empty()
        && user.chars().all(|c| c.is_alphanumeric() || "._%+-".contains(c))
        && labels.len() > 1
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && labels.last().is_some_and(|t| t.len() >= 2 && t.chars().all(char::is_alphabetic))
}

/// Wraps each cell of `rect` in an OSC 8 hyperlink, for the terminal's own
/// Ctrl+click. Only cells still drawn as a link (`is_link`) are touched, so
/// whatever was drawn over them keeps its text. The escapes would count in
/// ratatui's width, hence the forced one.
pub fn osc8(buf: &mut Buffer, rect: Rect, url: &str, is_link: impl Fn(&ratatui::buffer::Cell) -> bool) {
    // One id for every piece, so a wrapped link lights up whole on hover.
    let id = url.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    for x in rect.left()..rect.right() {
        let Some(cell) = buf.cell_mut((x, rect.y)) else { continue };
        let sym = cell.symbol().to_string();
        let Some(w) = NonZeroU16::new(sym.width() as u16).filter(|_| is_link(cell)) else { continue };
        cell.set_symbol(&format!("\x1b]8;id=wa{id};{url}\x1b\\{sym}\x1b]8;;\x1b\\"));
        cell.set_diff_option(CellDiffOption::ForcedWidth(w));
    }
}

/// Opens it in the browser. Quiet, as the browser's output would land on the
/// TUI, and reaped off the main thread.
pub fn open(url: &str) {
    let child = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut c) = child {
        std::thread::spawn(move || c.wait());
    }
}
