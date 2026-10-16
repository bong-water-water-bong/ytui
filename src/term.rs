//! Diffing truecolour cell writer. Only changed cells are emitted, colour
//! escapes are skipped when unchanged, and every frame is wrapped in a
//! synchronized-update block (DEC 2026) so terminals that support it never
//! show a half-drawn frame.

use crate::render::Cell;
use std::fmt::Write as _;
use std::io::Write;

pub struct Screen {
    prev: Vec<Cell>,
    geom: (usize, usize, usize, usize),
    buf: String,
}

impl Screen {
    pub fn new() -> Screen {
        Screen { prev: Vec::new(), geom: (0, 0, 0, 0), buf: String::new() }
    }

    pub fn invalidate(&mut self) {
        self.prev.clear();
    }

    /// Draw `cells` (cols x rows) with the top-left at terminal (x0, y0).
    pub fn draw(&mut self, cells: &[Cell], cols: usize, rows: usize, x0: usize, y0: usize) -> std::io::Result<()> {
        let full = self.geom != (cols, rows, x0, y0) || self.prev.len() != cells.len();
        self.geom = (cols, rows, x0, y0);
        let b = &mut self.buf;
        b.clear();
        b.push_str("\x1b[?2026h");
        let mut cur: Option<([u8; 3], [u8; 3])> = None;
        for y in 0..rows {
            let mut at = usize::MAX; // column the cursor is known to be at
            for x in 0..cols {
                let c = cells[y * cols + x];
                if !full && self.prev[y * cols + x] == c {
                    continue;
                }
                if at != x {
                    let _ = write!(b, "\x1b[{};{}H", y0 + y + 1, x0 + x + 1);
                }
                let (fg_ch, bg_ch) = match cur {
                    Some((f, g)) => (f != c.fg && c.ch != ' ', g != c.bg),
                    None => (true, true),
                };
                if fg_ch || bg_ch {
                    b.push_str("\x1b[");
                    if fg_ch {
                        let _ = write!(b, "38;2;{};{};{}", c.fg[0], c.fg[1], c.fg[2]);
                    }
                    if bg_ch {
                        if fg_ch {
                            b.push(';');
                        }
                        let _ = write!(b, "48;2;{};{};{}", c.bg[0], c.bg[1], c.bg[2]);
                    }
                    b.push('m');
                    let f = if fg_ch { c.fg } else { cur.map(|x| x.0).unwrap_or(c.fg) };
                    cur = Some((f, c.bg));
                }
                b.push(c.ch);
                at = x + 1;
            }
        }
        b.push_str("\x1b[0m\x1b[?2026l");
        self.prev.clear();
        self.prev.extend_from_slice(cells);
        let mut out = std::io::stdout().lock();
        out.write_all(b.as_bytes())?;
        out.flush()
    }
}

/// One-shot ANSI dump (used by --snapshot).
pub fn to_ansi(cells: &[Cell], cols: usize, rows: usize) -> String {
    let mut s = String::new();
    for y in 0..rows {
        for x in 0..cols {
            let c = cells[y * cols + x];
            let _ = write!(
                s,
                "\x1b[38;2;{};{};{};48;2;{};{};{}m{}",
                c.fg[0], c.fg[1], c.fg[2], c.bg[0], c.bg[1], c.bg[2], c.ch
            );
        }
        s.push_str("\x1b[0m\n");
    }
    s
}
