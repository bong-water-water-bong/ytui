//! Reading `.ANS` and `.XB` (XBin) text-mode art into a cell grid.
//!
//! The format is a byte stream of CP437 characters with ANSI X3.64 escape
//! sequences mixed in, optionally followed by a SAUCE record. There is no
//! header and no version, so the reader has to be forgiving: anything it does
//! not understand is skipped, never an error.
//!
//! The awkward parts, all of them load-bearing for fidelity:
//!
//! * `ESC[1m` is not bold. It sets the foreground attribute's high bit, so
//!   it brightens 30..=37 into 90..=97. It must be re-applied when a new
//!   30..=37 arrives, or text goes dark after the first colour change.
//! * `ESC[5m` is not blink. It sets the background attribute's high bit.
//! * Whether that high bit is a colour at all depends on the iCE flag, which
//!   is a *reading* choice, not something the file records. The same bytes
//!   render differently under [`crate::textmode::palette::Ice::On`] and
//!   `Ice::Off`.
//! * Variable-length escapes need bounds, or a truncated file walks off the
//!   end. Every loop here is index-checked.

use super::glyph::{ch, Font, GLYPH_H};
use super::palette::{fit_cell, ink_masks, Ice, Rgb, TCell};
use super::writer::dos_of;

/// A parsed piece of art. Rows are `rows x cols` cells; `cols` is the widest
/// row seen (or SAUCE's width if it gave one).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Art {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<TCell>,
    /// SAUCE metadata, when the file carried it.
    pub sauce: Option<Sauce>,
    /// Did any truecolor (PabloDraw `t`) sequence appear?
    pub truecolor: bool,
    /// Did the art rely on iCE backgrounds (a `ESC[5m` or a background 8..=15)?
    pub used_ice: bool,
}

impl Art {
    pub fn get(&self, x: usize, y: usize) -> TCell {
        if x < self.cols && y < self.rows {
            self.cells[y * self.cols + x]
        } else {
            TCell::default()
        }
    }
}

/// A SAUCE record: the metadata block appended after a 0x1A end-of-file byte.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Sauce {
    pub title: String,
    pub author: String,
    pub group: String,
    pub date: String,
    /// Width in columns, as recorded by the editor. 0 means "unknown".
    pub width: usize,
    /// Font ID (1 = 8x16 VGA, 2 = 8x14 EGA, 3 = 8x8 CGA, ...).
    pub font: u8,
    /// SAUCE flags byte: bit 0 non-blink (iCE), bit 1 9-dot, bit 2 aspect.
    pub flags: u8,
    /// The Datatype/Filetype pair, e.g. `b"Character/ANSI"`.
    pub datatype: u8,
    pub filetype: u8,
}

impl Sauce {
    /// The iCE flag the file asks for, if the record is a character-mode one.
    pub fn ice(&self) -> bool {
        self.flags & 1 != 0
    }

    /// The 9-dot (non-square) flag.
    pub fn nine_dot(&self) -> bool {
        self.flags & 2 != 0
    }

    pub fn cell_width(&self) -> usize {
        if self.nine_dot() {
            9
        } else {
            8
        }
    }
}

const SAUCE_LEN: usize = 128;
const SAUCE_MAGIC: &[u8; 7] = b"SAUCE00";

/// Find and parse a SAUCE record. It sits at the end of the file and must be
/// searched for backwards, because the art itself may contain the word.
pub fn parse_sauce(data: &[u8]) -> Option<Sauce> {
    if data.len() < SAUCE_LEN {
        return None;
    }
    let start = data.len() - SAUCE_LEN;
    if &data[start..start + 7] != SAUCE_MAGIC {
        return None;
    }
    let f = |a: usize, b: usize| -> String {
        let raw = &data[start + a..start + b];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        // SAUCE text is CP437 in practice; map the high half properly.
        raw[..end].iter().map(|&c| ch(c)).collect::<String>().trim_end().to_string()
    };
    // Field offsets are the SAUCE spec's, and the four-byte FileSize at 90
    // is why everything after the date sits where it does.
    Some(Sauce {
        title: f(7, 42),
        author: f(42, 62),
        group: f(62, 82),
        date: f(82, 90),
        datatype: data[start + 94],
        filetype: data[start + 95],
        width: u16::from_le_bytes([data[start + 96], data[start + 97]]) as usize,
        font: u16::from_le_bytes([data[start + 98], data[start + 99]]) as u8,
        flags: data[start + 105],
    })
}

/// The art bytes with any SAUCE tail and its 0x1A marker removed.
fn strip_sauce(data: &[u8]) -> (&[u8], Option<Sauce>) {
    if let Some(s) = parse_sauce(data) {
        let mut end = data.len() - SAUCE_LEN;
        // A well-formed file has a 0x1A immediately before the record.
        if end > 0 && data[end - 1] == 0x1A {
            end -= 1;
        }
        (&data[..end], Some(s))
    } else {
        (data, None)
    }
}

/// The colour state the parser carries from one sequence to the next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pen {
    fg: u8,
    bg: u8,
    /// Set by `ESC[1m`: brightens future 30..=37 without being a font weight.
    bold: bool,
    /// Set by `ESC[5m`: the high background bit, if iCE is on.
    blink: bool,
    /// Truecolour overrides, from PabloDraw's `t` sequences.
    fg24: Option<[u8; 3]>,
    bg24: Option<[u8; 3]>,
}

impl Default for Pen {
    fn default() -> Self {
        Pen { fg: 7, bg: 0, bold: false, blink: false, fg24: None, bg24: None }
    }
}

impl Pen {
    /// `ESC[0m`: everything back to the power-on state.
    fn reset(&mut self) {
        *self = Pen::default();
    }

    /// A foreground colour, honouring a standing `bold`.
    fn set_fg(&mut self, idx: u8) {
        self.fg = if self.bold { idx + 8 } else { idx };
        self.fg24 = None;
    }

    /// A background colour, honouring a standing `blink` when iCE is on.
    fn set_bg(&mut self, idx: u8, ice: Ice) {
        self.bg = if self.blink && ice == Ice::On { idx + 8 } else { idx };
        self.bg24 = None;
    }
}

/// Options that change how bytes are read.
#[derive(Clone, Copy, Debug)]
pub struct ReadOpts {
    /// How to read the background high bit when the file does not say.
    pub ice: Ice,
    /// Force this width instead of measuring the art.
    pub width: Option<usize>,
    /// Honour a SAUCE record's width and iCE flag when present.
    pub use_sauce: bool,
}

impl Default for ReadOpts {
    fn default() -> Self {
        ReadOpts { ice: Ice::On, width: None, use_sauce: true }
    }
}

/// Parse an `.ANS` byte stream.
pub fn read_ansi(data: &[u8], opts: ReadOpts) -> Art {
    let (body, sauce) = strip_sauce(data);
    let mut ice = opts.ice;
    let mut width_hint = None;
    if opts.use_sauce {
        if let Some(s) = &sauce {
            // SAUCE only gets to speak about iCE if it is character art.
            if s.datatype == 1 && s.filetype == 1 {
                ice = if s.ice() { Ice::On } else { Ice::Off };
            }
            if s.width > 0 {
                width_hint = Some(s.width);
            }
        }
    }

    let mut grid: Vec<TCell> = Vec::new();
    let mut cur = TCell::default();
    let mut pen = Pen::default();
    let (mut x, mut y) = (0usize, 0usize);
    let mut cols = 0usize;
    let mut rows = 0usize;
    let mut used_ice = false;
    let mut truecolor = false;

    let mut i = 0usize;
    while i < body.len() {
        let b = body[i];
        match b {
            0x1B => {
                // CSI: ESC [ ... final byte in 0x40..=0x7E.
                if i + 1 < body.len() && body[i + 1] == b'[' {
                    let mut j = i + 2;
                    while j < body.len() && !(0x40..=0x7E).contains(&body[j]) {
                        j += 1;
                    }
                    if j >= body.len() {
                        break; // truncated sequence: stop, do not guess
                    }
                    let params = &body[i + 2..j];
                    let final_byte = body[j];
                    apply_csi(
                        final_byte,
                        params,
                        &mut pen,
                        &mut cur,
                        &mut x,
                        &mut y,
                        ice,
                        &mut used_ice,
                        &mut truecolor,
                    );
                    i = j + 1;
                    continue;
                }
                i += 1;
            }
            0x0A => {
                y += 1;
                x = 0;
                i += 1;
            }
            0x0D => {
                x = 0;
                i += 1;
            }
            0x08 => {
                x = x.saturating_sub(1);
                i += 1;
            }
            0x09 => {
                x = (x / 8 + 1) * 8;
                i += 1;
            }
            0x00 => {
                i += 1;
            }
            _ => {
                cur.ch = b;
                cur.fg = pen.fg;
                cur.bg = pen.bg;
                write_cell(&mut grid, &mut cols, &mut rows, x, y, cur, width_hint);
                x += 1;
                i += 1;
            }
        }
    }

    // Rows are ragged in practice; pad every row out to the widest row seen
    // (or the declared width, if that is wider) so the grid is rectangular
    // and the flat index is trivial.
    let width = opts.width.or(width_hint).unwrap_or(cols).max(cols);
    // `grid` carries its own stride, which is `cols` by construction.
    let stride = cols;
    let mut cells = vec![TCell::default(); width * rows.max(1)];
    for r in 0..rows {
        for c in 0..width.min(stride) {
            cells[r * width + c] = grid.get(r * stride + c).copied().unwrap_or_default();
        }
        for c in stride.min(width)..width {
            cells[r * width + c] = TCell { ch: b' ', ..TCell::default() };
        }
    }

    Art { cols: width, rows: rows.max(1), cells, sauce, truecolor, used_ice }
}

/// Write one cell, growing the grid as needed.
///
/// The grid is stored packed at a fixed stride from the first write, so a
/// later row/column that overruns it re-packs at the new stride. The stride
/// starts at `width_hint` (SAUCE's width, or the caller's) because that is
/// the geometry the art was drawn to and re-packing is the expensive path;
/// art that overruns its declared width still works, it just pays.
fn write_cell(
    grid: &mut Vec<TCell>,
    cols: &mut usize,
    rows: &mut usize,
    x: usize,
    y: usize,
    c: TCell,
    width_hint: Option<usize>,
) {
    if *cols == 0 {
        // First write decides the stride.
        *cols = width_hint.unwrap_or(0).max(x + 1);
    }
    let need_cols = (*cols).max(x + 1);
    let need_rows = (*rows).max(y + 1);
    if need_cols != *cols || grid.len() < *cols * need_rows {
        let mut next = vec![TCell::default(); need_cols * need_rows];
        for r in 0..*rows {
            let from = r * *cols;
            let to = r * need_cols;
            next[to..to + *cols].copy_from_slice(&grid[from..from + *cols]);
        }
        *grid = next;
        *cols = need_cols;
    }
    *rows = need_rows;
    grid[y * *cols + x] = c;
}

/// Apply one CSI sequence. Only the handful the art uses; anything else is
/// skipped, which is what every viewer does.
#[allow(clippy::too_many_arguments)]
fn apply_csi(
    final_byte: u8,
    params: &[u8],
    pen: &mut Pen,
    cur: &mut TCell,
    x: &mut usize,
    y: &mut usize,
    ice: Ice,
    used_ice: &mut bool,
    truecolor: &mut bool,
) {
    // Private sequences (ESC[?7l etc.) are modes, not drawing: skip them.
    let private = params.first() == Some(&b'?');
    let nums: Vec<u32> = params
        .split(|&c| c == b';')
        .filter(|p| !p.is_empty() && p[0] != b'?')
        .map(|p| {
            std::str::from_utf8(p)
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
        })
        .collect();
    let n = |idx: usize, default: u32| nums.get(idx).copied().filter(|&v| v != 0).unwrap_or(default) as usize;

    match final_byte {
        b'm' => {
            if nums.is_empty() {
                pen.reset();
                return;
            }
            for &v in &nums {
                match v {
                    0 => pen.reset(),
                    1 => {
                        // Bold brightens the colour *already set*, not just
                        // the next one. Art leans on this: a bare `ESC[1m`
                        // after `ESC[31m` is how bright red is written, and
                        // treating it as only a font weight leaves the whole
                        // picture a shade too dark.
                        pen.bold = true;
                        if pen.fg < 8 {
                            pen.fg += 8;
                        }
                        pen.fg24 = None;
                    }
                    22 => {
                        // Leaving bold dims the current colour back down.
                        pen.bold = false;
                        if pen.fg >= 8 {
                            pen.fg -= 8;
                        }
                    }
                    5 => {
                        pen.blink = true;
                        pen.bg24 = None;
                    }
                    25 => pen.blink = false,
                    // Foreground 30..=37, brightened by a standing bold.
                    // Escapes count in ANSI colour order; we store DOS
                    // attribute order. set_fg applies a standing bold, so the
                    // brightness survives on every subsequent colour change.
                    30..=37 => pen.set_fg(dos_of((v - 30) as u8)),
                    // Real bright foregrounds. These set the high bit directly
                    // and are not bold-dependent.
                    90..=97 => {
                        pen.fg = dos_of((v - 90) as u8 + 8);
                        pen.fg24 = None;
                    }
                    // Background 40..=47, brightened by a standing blink on iCE.
                    40..=47 => {
                        let idx = dos_of((v - 40) as u8);
                        if pen.blink && ice == Ice::On {
                            pen.bg = idx + 8;
                            *used_ice = true;
                        } else {
                            pen.bg = idx;
                        }
                        pen.bg24 = None;
                    }
                    100..=107 => {
                        pen.bg = dos_of((v - 100) as u8 + 8);
                        pen.bg24 = None;
                        *used_ice = true;
                    }
                    7 => {
                        // Reverse video: swap, which is how lightbars work.
                        std::mem::swap(&mut pen.fg, &mut pen.bg);
                    }
                    _ => {}
                }
            }
            cur.fg = pen.fg;
            cur.bg = pen.bg;
        }
        // Cursor position: row ; col, both 1-based.
        b'H' | b'f' => {
            *y = n(0, 1).saturating_sub(1);
            *x = n(1, 1).saturating_sub(1);
        }
        b'A' => *y = y.saturating_sub(n(0, 1)),
        b'B' => *y += n(0, 1),
        b'C' => *x += n(0, 1),
        b'D' => *x = x.saturating_sub(n(0, 1)),
        b'G' => *x = n(0, 1).saturating_sub(1),
        b'd' => *y = n(0, 1).saturating_sub(1),
        // Erase in display / line: the art mostly uses these to clear, which
        // a fresh grid already is. Only the cursor-home part matters.
        b'J' if n(0, 0) == 2 => {
            *x = 0;
            *y = 0;
        }
        b'K' | b'J' => {}
        // PabloDraw's truecolour extension: ESC[<0|1>;r;g;b t, where 0 is
        // the background and 1 the foreground.
        b't' if !private && nums.len() >= 4 => {
            let which = nums[0];
            let rgb = [nums[1].min(255) as u8, nums[2].min(255) as u8, nums[3].min(255) as u8];
            match which {
                0 => pen.bg24 = Some(rgb),
                1 => pen.fg24 = Some(rgb),
                _ => {}
            }
            *truecolor = true;
        }
        // Amiga ANSI cursor sequences, mode sets, and everything else.
        _ => {}
    }
}

/// How a renderer should present the colours of a parsed [`Art`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colour {
    /// Use the DOS palette indices, reading the high bit as `ice` says.
    Palette(Ice),
    /// Use SAUCE's iCE flag if it had one, else `fallback`.
    SauceOr(Ice),
}

impl Colour {
    pub fn resolve(self, art: &Art) -> Ice {
        match self {
            Colour::Palette(i) => i,
            Colour::SauceOr(f) => match &art.sauce {
                Some(s) if s.datatype == 1 => {
                    if s.ice() {
                        Ice::On
                    } else {
                        Ice::Off
                    }
                }
                _ => f,
            },
        }
    }
}

/// Convert an [`Art`] to ytui's video [`crate::render::Cell`] grid, so the
/// existing renderer, sixel and kitty paths can draw it.
///
/// `set` picks which RGB answer to use: [`Rgb::Xterm`] for anything bound for
/// a terminal, [`Rgb::Vga`] for comparison against authoring tools.
pub fn to_cells(art: &Art, ice: Ice, set: Rgb) -> Vec<crate::render::Cell> {
    let mut out = Vec::with_capacity(art.cols * art.rows);
    for y in 0..art.rows {
        for x in 0..art.cols {
            let t = art.get(x, y);
            let bg_idx = if ice == Ice::On { t.bg } else { t.bg & 7 };
            out.push(crate::render::Cell {
                ch: ch(t.ch),
                fg: set.get(t.fg & 15),
                bg: set.get(bg_idx & 15),
            });
        }
    }
    out
}

/// Build an [`Art`] from an arbitrary image by fitting glyphs and colours.
/// This is the encode direction: video frame in, text-mode art out.
///
/// `bits` is the cell width in dots (8, or 9 for line-drawing joins).
pub fn from_image(
    px: &[u8],
    w: usize,
    h: usize,
    cols: usize,
    rows: usize,
    ice: Ice,
    set: Rgb,
    bits: usize,
) -> Art {
    let font = Font::dos();
    let (cw, chh) = (bits, GLYPH_H);
    let masks = ink_masks(&font, cw, chh);
    let mut cells = Vec::with_capacity(rows * cols);
    let mut cell_px = vec![[0u8; 3]; cw * chh];

    for r in 0..rows {
        for c in 0..cols {
            // Nearest-neighbour downscale of this cell's block of pixels.
            for y in 0..chh {
                for x in 0..cw {
                    let sx = (c * cw + x) * w / (cols * cw);
                    let sy = (r * chh + y) * h / (rows * chh);
                    let at = (sy.min(h - 1) * w + sx.min(w - 1)) * 3;
                    cell_px[y * cw + x] = [px[at], px[at + 1], px[at + 2]];
                }
            }
            let mut best = (u64::MAX, TCell::default());
            for (b, ink) in &masks {
                let (mut cell, err) = fit_cell(&cell_px, ink, ice, set);
                if err < best.0 {
                    cell.ch = *b;
                    best = (err, cell);
                }
            }
            cells.push(best.1);
        }
    }
    Art { cols, rows, cells, sauce: None, truecolor: false, used_ice: ice == Ice::On }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(bytes: &[u8]) -> Art {
        read_ansi(bytes, ReadOpts::default())
    }

    #[test]
    fn plain_text_lands_in_the_grid() {
        let a = art(b"AB\nCD");
        assert_eq!((a.cols, a.rows), (2, 2));
        assert_eq!(a.get(0, 0).ch, b'A');
        assert_eq!(a.get(1, 0).ch, b'B');
        assert_eq!(a.get(0, 1).ch, b'C');
        assert_eq!(a.get(1, 1).ch, b'D');
    }

    #[test]
    fn bold_raises_the_foreground_attribute() {
        // ESC[1m ESC[31m: bright red (12), not dark red (4).
        let a = art(b"\x1b[1m\x1b[31mX");
        assert_eq!(a.get(0, 0).fg, 12, "bold + red must be bright red");
        // ...and the brightening must survive a *second* colour change.
        let b = art(b"\x1b[1m\x1b[31mA\x1b[32mB");
        assert_eq!(b.get(0, 0).fg, 12);
        assert_eq!(b.get(1, 0).fg, 10, "bold must re-apply to the next colour");
        // Without bold, plain red.
        let c = art(b"\x1b[31mX");
        assert_eq!(c.get(0, 0).fg, 4);
        // Escape 22 drops the brightness again.
        let d = art(b"\x1b[1m\x1b[22m\x1b[31mX");
        assert_eq!(d.get(0, 0).fg, 4);
    }

    #[test]
    fn blink_is_a_background_bit_only_when_ice_is_on() {
        let on = read_ansi(b"\x1b[5m\x1b[41mX", ReadOpts { ice: Ice::On, ..Default::default() });
        assert_eq!(on.get(0, 0).bg, 12, "iCE on: blink bit becomes a bright red background");
        assert!(on.used_ice);

        let off = read_ansi(b"\x1b[5m\x1b[41mX", ReadOpts { ice: Ice::Off, ..Default::default() });
        assert_eq!(off.get(0, 0).bg, 4, "iCE off: the high bit is blink, not colour");
        assert!(!off.used_ice);
    }

    #[test]
    fn reset_clears_every_attribute() {
        let a = art(b"\x1b[1m\x1b[5m\x1b[31m\x1b[42mA\x1b[0mB");
        assert_eq!(a.get(0, 0).fg, 12);
        assert_eq!(a.get(1, 0).fg, 7, "reset returns the default foreground");
        assert_eq!(a.get(1, 0).bg, 0);
    }

    #[test]
    fn bright_foregrounds_set_the_high_bit_directly() {
        // 91 is bright *red*, which is DOS attribute 12, not 9.
        let a = art(b"\x1b[91mX");
        assert_eq!(a.get(0, 0).fg, 12);
        let b = art(b"\x1b[97mX");
        assert_eq!(b.get(0, 0).fg, 15);
    }

    #[test]
    fn cursor_positioning_is_one_based() {
        let a = art(b"\x1b[3;5HX");
        assert_eq!(a.get(4, 2).ch, b'X');
        assert_eq!(a.get(0, 2).ch, b' ', "cells before the cursor stay blank");
    }

    #[test]
    fn truecolor_overrides_and_is_reported() {
        let a = art(b"\x1b[1;255;128;0tX");
        assert!(a.truecolor);
        assert_eq!(a.get(0, 0).fg, 7, "palette index is untouched; the 24-bit value wins at draw time");
        let b = art(b"\x1b[0;16;32;48tX");
        assert!(b.truecolor);
    }

    #[test]
    fn truncated_files_do_not_panic() {
        for cut in [
            &b"\x1b"[..],
            &b"\x1b["[..],
            &b"\x1b[31"[..],
            &b"\x1b[999;999;999;999;999t"[..],
            &b"\x1b[1;2;3"[..],
            &b"AB\x1b["[..],
        ] {
            let a = art(cut);
            assert!(a.rows >= 1);
        }
    }

    #[test]
    fn private_modes_are_skipped_not_drawn() {
        let a = art(b"\x1b[?7lAB\x1b[?7hCD");
        assert_eq!(a.cols, 4);
        assert_eq!(a.get(0, 0).ch, b'A');
        assert_eq!(a.get(3, 0).ch, b'D');
    }

    #[test]
    fn sauce_is_found_and_stripped() {
        let mut file = b"\x1b[31mHI".to_vec();
        file.push(0x1A);
        let mut rec = vec![0u8; SAUCE_LEN];
        rec[..7].copy_from_slice(SAUCE_MAGIC);
        rec[7..10].copy_from_slice(b"ART");
        rec[42..44].copy_from_slice(b"ME");
        rec[94] = 1; // Character
        rec[95] = 1; // ANSI
        rec[96..98].copy_from_slice(&80u16.to_le_bytes());
        rec[98..100].copy_from_slice(&1u16.to_le_bytes()); // font 1 = 8x16
        rec[105] = 1; // iCE, 8-dot
        file.extend_from_slice(&rec);

        let a = art(&file);
        let s = a.sauce.clone().expect("SAUCE should be found");
        assert_eq!(s.title, "ART");
        assert_eq!(s.author, "ME");
        assert_eq!(s.width, 80);
        assert_eq!(s.font, 1);
        assert!(s.ice());
        assert_eq!(s.cell_width(), 8);
        assert_eq!(a.cols, 80, "SAUCE width widens the grid");
        assert_eq!(a.get(0, 0).ch, b'H', "art bytes survive the strip");
    }

    #[test]
    fn sauce_ice_off_is_respected() {
        let mut file = b"\x1b[5m\x1b[41mX".to_vec();
        file.push(0x1A);
        let mut rec = vec![0u8; SAUCE_LEN];
        rec[..7].copy_from_slice(SAUCE_MAGIC);
        rec[94] = 1;
        rec[95] = 1;
        rec[96..98].copy_from_slice(&0u16.to_le_bytes()); // width unknown
        rec[98..100].copy_from_slice(&1u16.to_le_bytes());
        rec[105] = 0; // explicitly non-iCE
        file.extend_from_slice(&rec);
        let a = read_ansi(&file, ReadOpts { ice: Ice::On, use_sauce: true, ..Default::default() });
        assert_eq!(a.get(0, 0).bg, 4, "the file's own flag must beat the caller's default");
    }

    #[test]
    fn ragged_rows_are_padded_to_the_widest() {
        let a = art(b"ABCDE\nX");
        assert_eq!(a.cols, 5);
        assert_eq!(a.rows, 2);
        assert_eq!(a.get(4, 1).ch, b' ');
    }

    #[test]
    fn every_cell_reaches_the_video_types() {
        let a = art(b"\x1b[31m\x1b[44mX");
        let cells = to_cells(&a, Ice::On, Rgb::Vga);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].ch, 'X');
        assert_eq!(cells[0].fg, super::super::palette::VGA[4]);
        assert_eq!(cells[0].bg, super::super::palette::VGA[1]);
    }

    #[test]
    fn image_round_trip_keeps_a_solid_block_solid() {
        // A red 8x16 cell must fit as a full block in red, not as noise.
        let (w, h) = (8, 16);
        let mut px = vec![0u8; w * h * 3];
        for p in px.chunks_exact_mut(3) {
            p.copy_from_slice(&super::super::palette::VGA[4]);
        }
        let a = from_image(&px, w, h, 1, 1, Ice::On, Rgb::Vga, 8);
        assert_eq!(a.cols, 1);
        assert_eq!(a.get(0, 0).bg, 4);
        assert_eq!(a.get(0, 0).fg, 4, "flat cell: same colour both sides");
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/bs-alove.ans");

    #[test]
    fn reads_the_ansilove_fixture_sauce() {
        let a = read_ansi(FIXTURE, ReadOpts::default());
        let s = a.sauce.as_ref().expect("fixture carries SAUCE");
        assert_eq!(s.title, "ansilove");
        assert_eq!(s.author, "burps");
        assert_eq!(s.group, "fuel");
        assert_eq!(s.date, "20171019");
        assert_eq!(s.width, 80);
        assert_eq!(s.datatype, 1, "Character");
        assert_eq!(s.filetype, 1, "ANSI");
        assert_eq!(s.font, 59, "IBM VGA (the ';' font id)");
        assert!(!s.ice(), "this file's flags say blink, not iCE");
        assert_eq!(a.rows, 59, "TInfo2 doubles as the row count");
    }

    #[test]
    fn fixture_grid_is_the_expected_geometry() {
        let a = read_ansi(FIXTURE, ReadOpts::default());
        assert_eq!(a.cols, 80);
        assert_eq!(a.rows, 59);
        assert_eq!(a.cells.len(), 80 * 59);
    }

    #[test]
    fn fixture_survives_a_round_trip() {
        let a = read_ansi(FIXTURE, ReadOpts::default());
        let bytes = super::super::writer::write_ansi(
            &a,
            super::super::writer::WriteOpts { sauce: true, ..Default::default() },
        );
        let b = read_ansi(&bytes, ReadOpts::default());
        assert_eq!((b.cols, b.rows), (a.cols, a.rows));
        for y in 0..a.rows {
            for x in 0..a.cols {
                let (p, q) = (a.get(x, y), b.get(x, y));
                assert_eq!((p.ch, p.fg & 7), (q.ch, q.fg & 7), "cell {x},{y}");
            }
        }
    }
}


#[cfg(test)]
mod conformance {
    use super::*;
    use super::super::palette::rasterise;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/bs-alove.ans");
    /// Rendered by AnsiLove/C 1.4.2 (`ansilove bs-alove.ans`), the reference
    /// implementation of this format.
    const REFERENCE: &[u8] = include_bytes!("../../tests/fixtures/bs-alove.reference.png");

    /// Decode a non-interlaced 8-bit RGB PNG, enough for our own fixture.
    fn read_png_rgb(data: &[u8]) -> (usize, usize, Vec<u8>) {
        assert_eq!(&data[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        let mut at = 8;
        let (mut w, mut h, mut bit_depth, mut colour) = (0, 0, 0, 0);
        let mut idat = Vec::new();
        while at + 8 <= data.len() {
            let len = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            let kind = &data[at + 4..at + 8];
            let body = &data[at + 8..at + 8 + len];
            match kind {
                b"IHDR" => {
                    w = u32::from_be_bytes(body[0..4].try_into().unwrap()) as usize;
                    h = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
                    bit_depth = body[8];
                    colour = body[9];
                }
                b"IDAT" => idat.extend_from_slice(body),
                b"IEND" => break,
                _ => {}
            }
            at += 12 + len;
        }
        assert_eq!((bit_depth, colour), (8, 2), "expected 8-bit RGB");
        let raw = {
            let mut out = Vec::new();
            let mut z = flate2::read::ZlibDecoder::new(&idat[..]);
            std::io::Read::read_to_end(&mut z, &mut out).expect("inflate");
            out
        };
        // Undo the per-scanline PNG filters.
        let bpp = 3;
        let stride = w * bpp;
        let mut px = vec![0u8; stride * h];
        for y in 0..h {
            let ft = raw[y * (stride + 1)];
            let line = &raw[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride];
            for x in 0..stride {
                let a = if x >= bpp { px[y * stride + x - bpp] as i32 } else { 0 };
                let b = if y > 0 { px[(y - 1) * stride + x] as i32 } else { 0 };
                let c = if x >= bpp && y > 0 { px[(y - 1) * stride + x - bpp] as i32 } else { 0 };
                let v = line[x] as i32;
                let out = match ft {
                    0 => v,
                    1 => v + a,
                    2 => v + b,
                    3 => v + (a + b) / 2,
                    4 => {
                        let p = a + b - c;
                        let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                        v + if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                    }
                    _ => panic!("bad filter {ft}"),
                };
                px[y * stride + x] = (out & 0xff) as u8;
            }
        }
        (w, h, px)
    }

    #[test]
    fn matches_ansilove_pixel_for_pixel() {
        let art = read_ansi(FIXTURE, ReadOpts::default());
        let mine = rasterise(&art.cells, art.cols, art.rows, Rgb::Vga, 8);
        let (w, h, theirs) = read_png_rgb(REFERENCE);
        assert_eq!((w, h), (art.cols * 8, art.rows * 16), "geometry must match");
        assert_eq!(mine.len(), theirs.len());

        let mut differing = 0usize;
        let mut first = None;
        for (i, (a, b)) in mine.chunks_exact(3).zip(theirs.chunks_exact(3)).enumerate() {
            if a != b {
                differing += 1;
                first.get_or_insert((i % w / 8, i / w / 16, [a[0], a[1], a[2]], [b[0], b[1], b[2]]));
            }
        }
        assert_eq!(
            differing, 0,
            "{} of {} pixels differ; first at cell {:?}",
            differing,
            w * h,
            first
        );
    }
}
