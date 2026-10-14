//! ratatui's crossterm backend, with one fix for wide emojis. After one
//! with U+FE0F, ratatui also redraws the column it covers (some terminals
//! leave it dirty) with a blank, and the backend writes that one without
//! moving there, as if the emoji had moved the cursor one column instead of
//! two: the blank and the rest of the run land one column to the right.
//! Moving there instead, the blank would wipe the emoji. So the blank goes
//! first and the emoji over it, and a move follows every wide cell. Seen
//! where a picture (the button down to the newest) had been over such an
//! emoji.

use std::io::{self, Stdout};

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use unicode_width::UnicodeWidthStr;

pub struct Wide(pub CrosstermBackend<Stdout>);

impl Backend for Wide {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut cells: Vec<_> = content.collect();
        // The cells a wide one covers before it, so it's drawn over them.
        let mut i = 0;
        while i < cells.len() {
            let (x, y, cell) = cells[i];
            let mut j = i + 1;
            while j < cells.len() && cells[j].1 == y && cells[j].0 < x + width(cell) {
                j += 1;
            }
            cells[i..j].rotate_left(1);
            i = j;
        }
        // A run ends after each wide cell, so the next one starts with a move.
        let mut run = Vec::new();
        for (x, y, cell) in cells {
            run.push((x, y, cell));
            if width(cell) > 1 {
                self.0.draw(run.drain(..))?;
            }
        }
        self.0.draw(run.into_iter())
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.0.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.0.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.0.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.0.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.0.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.0.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.0)
    }
}

/// Columns a cell takes on screen. A picture's cell carries escapes: one.
fn width(cell: &Cell) -> u16 {
    let s = cell.symbol();
    if s.chars().any(char::is_control) { 1 } else { s.width() as u16 }
}
