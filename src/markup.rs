//! WhatsApp's formatting: `*bold*`, `_italic_`, `~strike~`, ```` ```mono``` ````
//! and `` `code` `` inline; `> ` quotes, `- ` / `* ` bullets and `1. `
//! numbered items at a line's start. One scan serves the bubbles (markers
//! hidden), the message box (markers dimmed, in place), the previews and
//! the plugin's HTML.

use crate::app::App;
use crate::bridge::Mention;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Per-char looks from `scan`.
pub const MARK: u16 = 1;
pub const BOLD: u16 = 2;
pub const ITALIC: u16 = 4;
pub const STRIKE: u16 = 8;
/// Between single backticks.
pub const CODE: u16 = 16;
/// Between ``` fences, which may span lines; fences included.
pub const MONO: u16 = 32;
/// The `> ` of a quote line.
pub const QUOTE: u16 = 64;
/// The `- ` or `* ` of a bulleted item.
pub const BULLET: u16 = 128;
/// The `1. ` of a numbered item.
pub const NUMBER: u16 = 256;
const LEAD: u16 = QUOTE | BULLET | NUMBER;

#[derive(Clone, Copy)]
enum Lead {
    Quote,
    Bullet,
    Number(u64),
}

/// A line's quote or list prefix and how many chars it takes. WhatsApp
/// wants it at the very start, its space included.
fn prefix(line: &[char]) -> Option<(Lead, usize)> {
    match line {
        ['>', ' ', ..] => Some((Lead::Quote, 2)),
        ['-' | '*', ' ', ..] => Some((Lead::Bullet, 2)),
        _ => {
            let d = line.iter().take_while(|c| c.is_ascii_digit()).count();
            let numbered = (1..=3).contains(&d) && line.get(d) == Some(&'.') && line.get(d + 1) == Some(&' ');
            numbered.then(|| (Lead::Number(line[..d].iter().collect::<String>().parse().unwrap_or(1)), d + 2))
        }
    }
}

/// The ``` blocks as (start, past the closing fence). A fence left
/// without its pair is text, as WhatsApp shows it.
fn fences(c: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut open = None;
    let mut i = 0;
    while i + 3 <= c.len() {
        if c[i..i + 3] != ['`'; 3] {
            i += 1;
            continue;
        }
        match open.take() {
            // Nothing between them is no block: this one opens the next.
            Some(a) if i == a + 3 => open = Some(i),
            Some(a) => out.push((a, i + 3)),
            None => open = Some(i),
        }
        i += 3;
    }
    out
}

/// Looks for each char of `c`.
pub fn scan(c: &[char]) -> Vec<u16> {
    let mut f = vec![0u16; c.len()];
    // A link is taken whole: `_` and `~` in a URL are no markers.
    let mut link = vec![0usize; c.len()];
    for (a, b, _) in crate::links::find(&c.iter().collect::<String>()) {
        link[a] = b - a;
    }
    let mut from = 0;
    for (a, b) in fences(c) {
        lines(c, from, a, &link, &mut f);
        f[a..b].fill(MONO);
        for k in (a..a + 3).chain(b - 3..b) {
            f[k] |= MARK;
        }
        from = b;
    }
    lines(c, from, c.len(), &link, &mut f);
    f
}

/// The lines between `a` and `b`; a prefix counts only at a line's start.
fn lines(c: &[char], a: usize, b: usize, link: &[usize], f: &mut [u16]) {
    let mut s = a;
    loop {
        let e = c[s..b].iter().position(|&x| x == '\n').map_or(b, |p| s + p);
        let mut from = s;
        if s == 0 || c[s - 1] == '\n' {
            if let Some((lead, n)) = prefix(&c[s..e]) {
                f[s..s + n].fill(match lead {
                    Lead::Quote => QUOTE,
                    Lead::Bullet => BULLET,
                    Lead::Number(_) => NUMBER,
                });
                from = s + n;
            }
        }
        inline(c, from, e, 0, link, f);
        if e >= b {
            break;
        }
        s = e + 1;
    }
}

/// One line's inline marks, WhatsApp's way: a marker opens after a non-word
/// char and before a non-space, and closes after a non-space and before a
/// non-word char, so `snake_case` and `2*3*4` stay as typed.
fn inline(c: &[char], a: usize, b: usize, style: u16, link: &[usize], f: &mut [u16]) {
    let mut i = a;
    while i < b {
        let n = link[i].min(b - i);
        if n > 0 {
            f[i..i + n].fill(style);
            i += n;
            continue;
        }
        let ch = c[i];
        let opens = matches!(ch, '*' | '_' | '~' | '`')
            && (i == 0 || !c[i - 1].is_alphanumeric())
            && i + 1 < b
            && !c[i + 1].is_whitespace();
        match opens.then(|| closing(c, i + 1, b, ch, link)).flatten() {
            Some(j) => {
                f[i] = style | MARK;
                f[j] = style | MARK;
                match ch {
                    '`' => f[i + 1..j].fill(style | CODE),
                    '*' => inline(c, i + 1, j, style | BOLD, link, f),
                    '_' => inline(c, i + 1, j, style | ITALIC, link, f),
                    _ => inline(c, i + 1, j, style | STRIKE, link, f),
                }
                i = j + 1;
            }
            None => {
                f[i] = style;
                i += 1;
            }
        }
    }
}

fn closing(c: &[char], start: usize, b: usize, mark: char, link: &[usize]) -> Option<usize> {
    let mut j = start;
    while j < b {
        let n = link[j].min(b - j);
        if n > 0 {
            j += n;
            continue;
        }
        if c[j] == mark && j > start && !c[j - 1].is_whitespace() && c.get(j + 1).is_none_or(|x| !x.is_alphanumeric()) {
            return Some(j);
        }
        j += 1;
    }
    None
}

// ---- drawing -----------------------------------------------------------------

/// A look on `base`: the styles as modifiers, code on `shade`.
pub fn style(look: u16, base: Style, shade: Color) -> Style {
    let mut s = base;
    for (bit, m) in [(BOLD, Modifier::BOLD), (ITALIC, Modifier::ITALIC), (STRIKE, Modifier::CROSSED_OUT)] {
        if look & bit != 0 {
            s = s.add_modifier(m);
        }
    }
    if look & (CODE | MONO) != 0 {
        s = s.bg(shade);
    }
    s
}

/// What a mention's link opens: that person's info, not a page.
pub const MENTION: &str = "wa-mention:";

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The text with each @'s digits replaced by the name, as WhatsApp shows
/// it, and where each mention is as (first char, past its last, MENTION
/// and the jid). An @ counts at a word's start, with five digits or more.
pub fn mentions(text: &str, ms: &[Mention]) -> (String, Vec<(usize, usize, String)>) {
    if ms.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let c: Vec<char> = text.chars().collect();
    let (mut out, mut at, mut n) = (String::new(), Vec::new(), 0);
    let mut i = 0;
    while i < c.len() {
        if c[i] == '@' && (i == 0 || !word(c[i - 1])) {
            let j = (i + 1..c.len()).find(|&k| !c[k].is_ascii_digit()).unwrap_or(c.len());
            let user: String = c[i + 1..j].iter().collect();
            let whole = j - i > 5 && c.get(j).is_none_or(|&x| !word(x));
            if let Some(m) = ms.iter().find(|m| m.user == user).filter(|_| whole) {
                let shown = format!("@{}", m.name);
                let len = shown.chars().count();
                at.push((n, n + len, format!("{MENTION}{}", m.jid)));
                out.push_str(&shown);
                n += len;
                i = j;
                continue;
            }
        }
        out.push(c[i]);
        n += 1;
        i += 1;
    }
    (out, at)
}

/// The colors `rows` draws with: the text's, a link's, a mention's, code's
/// background and a quote's grey.
pub struct Looks {
    pub base: Style,
    pub link: Style,
    pub mention: Style,
    pub shade: Color,
    pub dim: Color,
}

/// Rows of spans, and each link's piece as (row, col, width, url).
pub type Rows = (Vec<Vec<Span<'static>>>, Vec<(usize, usize, usize, String)>);

/// The text in rows of `width`, as WhatsApp Web shows it: markers hidden,
/// quotes behind a bar, list items with their bullet or number and a
/// hanging indent. Wrapped on what's shown, so widths and links add up.
/// `mentions` (from `mentions`) are drawn as links to their person.
pub fn rows(text: &str, width: usize, lk: &Looks, mentions: &[(usize, usize, String)]) -> Rows {
    let c: Vec<char> = text.chars().collect();
    let f = scan(&c);
    let mut found = crate::links::find(text);
    found.extend(mentions.iter().cloned());
    let owner = |i: usize| found.iter().position(|&(a, b, _)| (a..b).contains(&i));
    let (mut out, mut links) = (Vec::new(), Vec::new());
    let mut s = 0;
    loop {
        let e = c[s..].iter().position(|&x| x == '\n').map_or(c.len(), |p| s + p);
        let n = (s..e).take_while(|&k| f[k] & LEAD != 0).count();
        let kind = if n > 0 { f[s] & LEAD } else { 0 };
        let shown: Vec<usize> = (s + n..e).filter(|&k| f[k] & MARK == 0).collect();
        // A line that was only markers, as a ``` block's own lines, goes.
        if !shown.is_empty() || e == s + n {
            let (lead, lead_style) = match kind {
                QUOTE => ("▎ ".to_string(), lk.base.fg(lk.dim)),
                BULLET => ("• ".to_string(), lk.base),
                NUMBER => (c[s..s + n].iter().collect(), lk.base),
                _ => (String::new(), lk.base),
            };
            let lw = lead.width();
            let body = if kind == QUOTE { lk.base.fg(lk.dim) } else { lk.base };
            let vis: String = shown.iter().map(|&k| c[k]).collect();
            for (r, (l, at)) in crate::ui::wrap_indexed(&vis, width.saturating_sub(lw).max(1)).into_iter().enumerate() {
                let mut spans = Vec::new();
                if lw > 0 {
                    // The bar on every row of a quote; under an item, its indent.
                    let t = if r == 0 || kind == QUOTE { lead.clone() } else { " ".repeat(lw) };
                    spans.push(Span::styled(t, lead_style));
                }
                let chars: Vec<char> = l.chars().collect();
                let (mut col, mut k) = (lw, 0);
                while k < chars.len() {
                    let o = shown[at[k]];
                    let (who, look) = (owner(o), f[o]);
                    let mut piece = String::new();
                    while k < chars.len() && owner(shown[at[k]]) == who && f[shown[at[k]]] == look {
                        piece.push(chars[k]);
                        k += 1;
                    }
                    let w = piece.width();
                    let st = match who {
                        Some(j) => {
                            links.push((out.len(), col, w, found[j].2.clone()));
                            let st = if found[j].2.starts_with(MENTION) { lk.mention } else { lk.link };
                            style(look, st, lk.shade)
                        }
                        None => style(look, body, lk.shade),
                    };
                    spans.push(Span::styled(piece, st));
                    col += w;
                }
                out.push(spans);
            }
        }
        if e >= c.len() {
            break;
        }
        s = e + 1;
    }
    (out, links)
}

/// The text on one row, for the chat list and a reply's quote: markers
/// hidden, lines run together, cut at `width` with an ellipsis.
pub fn line(text: &str, width: usize, base: Style) -> Vec<Span<'static>> {
    let c: Vec<char> = text.chars().collect();
    let f = scan(&c);
    let mut cells: Vec<(char, u16)> = Vec::new();
    for (&ch, &look) in c.iter().zip(&f) {
        if look & (MARK | QUOTE) != 0 {
            continue;
        }
        let ch = match ch {
            '\n' => ' ',
            '-' | '*' if look & BULLET != 0 => '•',
            _ => ch,
        };
        // No room for code's background on one row.
        cells.push((ch, look & !(CODE | MONO)));
    }
    if cells.iter().map(|(ch, _)| ch.width().unwrap_or(0)).sum::<usize>() > width {
        let mut w = 0;
        let keep = cells.iter().take_while(|(ch, _)| {
            w += ch.width().unwrap_or(0);
            w < width
        });
        let n = keep.count();
        cells.truncate(n);
        cells.push(('…', 0));
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (ch, look) in cells {
        let st = style(look, base, Color::Reset);
        match spans.last_mut().filter(|s| s.style == st) {
            Some(s) => s.content.to_mut().push(ch),
            None => spans.push(Span::styled(ch.to_string(), st)),
        }
    }
    spans
}

/// A row of the message box, `start` chars into the text `looks` scanned:
/// styled as it'll be sent, its markers dimmed but in place, so the caret's
/// math stays the text's. A quote's `>` and a bullet are drawn as the bubble
/// draws them, one cell each.
pub fn typed_row(row: &str, start: usize, looks: &[u16], base: Style, shade: Color, dim: Color) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (k, ch) in row.chars().enumerate() {
        let look = looks.get(start + k).copied().unwrap_or(0);
        let mut st = style(look, base, shade);
        if look & (MARK | QUOTE | BULLET) != 0 {
            st = st.fg(dim).remove_modifier(Modifier::CROSSED_OUT);
        }
        let ch = match ch {
            '>' if look & QUOTE != 0 => '▎',
            '-' | '*' if look & BULLET != 0 => '•',
            _ => ch,
        };
        match spans.last_mut().filter(|s| s.style == st) {
            Some(s) => s.content.to_mut().push(ch),
            None => spans.push(Span::styled(ch.to_string(), st)),
        }
    }
    spans
}

/// The text as HTML for scripts: escaped, with `<b>`, `<i>`, `<s>` and
/// `<tt>` (monospace and code); quotes and bullets as the bubbles draw them.
pub fn html(text: &str) -> String {
    const TAGS: [(u16, &str); 4] = [(BOLD, "b"), (ITALIC, "i"), (STRIKE, "s"), (CODE | MONO, "tt")];
    let c: Vec<char> = text.chars().collect();
    let f = scan(&c);
    let mut out = String::new();
    let mut open = 0u16;
    let close = |out: &mut String, open: u16| {
        for (_, t) in TAGS.iter().rev().filter(|(bit, _)| open & bit != 0) {
            out.push_str(&format!("</{t}>"));
        }
    };
    for (&ch, &look) in c.iter().zip(&f) {
        if look & MARK != 0 {
            continue;
        }
        let want = TAGS.iter().filter(|(bit, _)| look & bit != 0).fold(0, |acc, (bit, _)| acc | bit);
        if want != open {
            close(&mut out, open);
            for (_, t) in TAGS.iter().filter(|(bit, _)| want & bit != 0) {
                out.push_str(&format!("<{t}>"));
            }
            open = want;
        }
        match ch {
            '>' if look & QUOTE != 0 => out.push('▎'),
            '-' | '*' if look & BULLET != 0 => out.push('•'),
            _ => out.push_str(&crate::events::escape(&ch.to_string())),
        }
    }
    close(&mut out, open);
    out
}

// ---- the box -------------------------------------------------------------------

/// The logical line holding `at`: its start and end, in chars.
fn line_at(chars: &[char], at: usize) -> (usize, usize) {
    let start = chars[..at].iter().rposition(|&c| c == '\n').map_or(0, |p| p + 1);
    let end = chars[at..].iter().position(|&c| c == '\n').map_or(chars.len(), |p| at + p);
    (start, end)
}

impl App {
    /// Formatting shortcuts in the message box, as in slick: Ctrl+B bold,
    /// Ctrl+I italic (when the terminal tells it from Tab), Ctrl+Shift+X
    /// strikethrough, Ctrl+Shift+M monospace, Ctrl+Shift+C code and
    /// Ctrl+Shift+9 quote; Alt+B, Alt+I, Alt+X, Alt+M and Alt+C where the
    /// terminal keeps the Ctrl ones to itself. True when handled.
    pub(crate) fn format_key(&mut self, k: KeyEvent) -> bool {
        let m = k.modifiers;
        let (ctrl, shift, alt) = (m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::SHIFT), m.contains(KeyModifiers::ALT));
        let KeyCode::Char(c) = k.code else { return false };
        let mark = match c.to_ascii_lowercase() {
            'b' if (ctrl && !shift && !alt) || (alt && !ctrl) => "*",
            'i' if (ctrl && !shift && !alt) || (alt && !ctrl) => "_",
            'x' if (ctrl && shift && !alt) || (alt && !ctrl) => "~",
            'm' if (ctrl && shift && !alt) || (alt && !ctrl) => "```",
            'c' if (ctrl && shift && !alt) || (alt && !ctrl) => "`",
            '9' | '(' if ctrl && shift && !alt => ">",
            _ => return false,
        };
        if mark == ">" {
            self.toggle_quote();
        } else {
            self.wrap_mark(mark);
        }
        true
    }

    /// A pair of `mark` with the caret between them. Right before a closing
    /// mark it steps over it; on an empty pair it takes the pair away.
    fn wrap_mark(&mut self, mark: &str) {
        let m: Vec<char> = mark.chars().collect();
        let chars: Vec<char> = self.input.chars().collect();
        let at = self.caret.min(chars.len());
        let (start, _) = line_at(&chars, at);
        let after = chars[at..].starts_with(&m);
        let before = at >= start + m.len() && chars[at - m.len()..at] == m[..];
        if after && before {
            self.input = chars[..at - m.len()].iter().chain(&chars[at + m.len()..]).collect();
            self.caret = at - m.len();
            return;
        }
        // A ``` block may span lines; the others stay on one.
        let from = if m.len() == 3 { 0 } else { start };
        let opened = chars[from..at].windows(m.len()).filter(|w| *w == &m[..]).count() % 2 == 1;
        if after && opened {
            self.caret = at + m.len();
            return;
        }
        // A marker glued to a word wouldn't count.
        if at > start && chars[at - 1].is_alphanumeric() {
            self.insert(" ");
        }
        self.insert(&mark.repeat(2));
        self.caret -= m.len();
    }

    /// The line becomes a quote, or stops being one.
    fn toggle_quote(&mut self) {
        let chars: Vec<char> = self.input.chars().collect();
        let at = self.caret.min(chars.len());
        let (start, _) = line_at(&chars, at);
        if chars[start..].starts_with(&['>', ' ']) {
            self.input = chars[..start].iter().chain(&chars[start + 2..]).collect();
            self.caret = if at >= start + 2 { at - 2 } else { start };
        } else {
            self.caret = start;
            self.insert("> ");
            self.caret = at + 2;
        }
    }

    /// A new line that stays in the quote or list, the next number on; on
    /// an empty item the marker goes instead, ending the list, as in slick.
    pub(crate) fn newline(&mut self) {
        let chars: Vec<char> = self.input.chars().collect();
        let at = self.caret.min(chars.len());
        let (start, end) = line_at(&chars, at);
        // Not inside a ``` block: the scan only finds prefixes outside them.
        let listed = scan(&chars).get(start).is_some_and(|l| l & LEAD != 0);
        let Some((lead, n)) = listed.then(|| prefix(&chars[start..end])).flatten() else { return self.insert("\n") };
        if chars[start + n..end].iter().all(|c| c.is_whitespace()) {
            self.input = chars[..start].iter().chain(&chars[start + n..]).collect();
            self.caret = if at >= start + n { at - n } else { start };
            return;
        }
        let next = match lead {
            Lead::Quote => "> ".to_string(),
            Lead::Bullet => chars[start..start + n].iter().collect(),
            Lead::Number(k) => format!("{}. ", k + 1),
        };
        self.insert(&format!("\n{next}"));
    }
}
