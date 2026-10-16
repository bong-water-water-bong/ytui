//! ANSI output for one-shot frame dumps (--snapshot).

use crate::render::Cell;
use std::fmt::Write as _;

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
