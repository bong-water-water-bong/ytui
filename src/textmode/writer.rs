//! Writing a cell grid out as `.ANS` bytes, with a SAUCE record.
//!
//! Two output styles, because the trade is real and both are wanted:
//!
//! * [`Style::Compact`] uses carriage-return/line-feed between rows and is
//!   the smaller file. It relies on the terminal's own line wrapping.
//! * [`Style::Portable`] places every row with an absolute cursor move and
//!   never depends on wrapping.
//!
//! Portable exists because of a specific, ugly fact: art exactly as wide as
//! the terminal makes the terminal wrap *by itself* at the last column, and
//! then the file's own CR/LF advances a second time, so every row is followed
//! by a blank line. Clients disagree about when the pending wrap fires:
//! xterm defers it until the next character, SyncTERM takes it immediately.
//! `ESC[?7l` turns wrapping off and fixes it, but mTelnet, NetRunner and
//! RGTerm ignore that sequence entirely. Absolute positioning is the only
//! thing every client honours, so a file that must look right everywhere
//! pays for it in bytes.
//!
//! Colours are written as the *palette indices* the art uses, never
//! converted: `ESC[0;31m` means "attribute 4" to the terminal, and which red
//! that is, is the terminal's business. That is what keeps a file faithful to
//! the art rather than to our guess about the viewer.

use super::palette::Ice;
use super::palette::LIGHT_GREY;
use super::reader::{Art, Sauce};

/// The attribute a fresh screen starts with: light grey on black.
const DEFAULT_FG: u8 = LIGHT_GREY;
const DEFAULT_BG: u8 = 0;

/// How to lay rows out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// CR/LF between rows, saving escapes. Smallest output.
    Compact,
    /// `ESC[row;colH` before every row. Survives every client.
    Portable,
}

/// Options for [`write_ansi`].
#[derive(Clone, Copy, Debug)]
pub struct WriteOpts {
    pub style: Style,
    /// How the background high bit should be read, so the escape we emit for
    /// a bright background is one the reader will decode back to the same
    /// colour. Off means backgrounds 8..=15 are folded into 0..=7.
    pub ice: Ice,
    /// Append a SAUCE record, including the iCE and 9-dot flags.
    pub sauce: bool,
    /// Cell width in dots, recorded in SAUCE.
    pub bits: usize,
    /// Emit `ESC[8m`+conceal for fully-blank cells instead of writing a space
    /// with the background colour. Only helps when backgrounds are uniform.
    pub skip_blank_bg: bool,
}

impl Default for WriteOpts {
    fn default() -> Self {
        WriteOpts { style: Style::Portable, ice: Ice::On, sauce: true, bits: 8, skip_blank_bg: false }
    }
}

/// DOS attribute index for each ANSI colour position 0..=15.
///
/// The two orders disagree in the middle: DOS orders an attribute byte
/// `IRGB` where blue is bit 0 and red is bit 2, so attribute 1 is blue and 4
/// is red. ANSI orders `ESC[31m` red, `ESC[34m` blue. Both halves keep their
/// own order, so the bright half uses the same permutation.
const ANSI_TO_DOS: [u8; 16] = [
    0, // 30 black
    4, // 31 red
    2, // 32 green
    6, // 33 yellow / brown
    1, // 34 blue
    5, // 35 magenta
    3, // 36 cyan
    7, // 37 light grey
    8, // 90 dark grey
    12, // 91 bright red
    10, // 92 bright green
    14, // 93 bright yellow
    9,  // 94 bright blue
    13, // 95 bright magenta
    11, // 96 bright cyan
    15, // 97 white
];

/// ANSI colour position for a DOS attribute index: the inverse of
/// [`ANSI_TO_DOS`].
pub fn ansi_of(dos: u8) -> u8 {
    let d = dos & 15;
    ANSI_TO_DOS.iter().position(|&x| x == d).unwrap_or(7) as u8
}

/// DOS attribute index for an ANSI colour position 0..=15.
pub fn dos_of(ansi: u8) -> u8 {
    ANSI_TO_DOS[(ansi & 15) as usize]
}

/// The escape for a foreground index 0..=15.
///
/// The DOS attribute order is not the ANSI escape order: attribute 1 is blue
/// and attribute 4 is red, while `ESC[31m` is red and `ESC[34m` is blue. Every
/// conversion goes through [`ansi_of`] so the swap happens exactly once.
fn fg_seq(idx: u8) -> Vec<u8> {
    let a = ansi_of(idx & 15);
    if a < 8 {
        // 30..=37. No ESC[1m: bold would brighten it further, and we are
        // expressing the attribute directly.
        format!("\x1b[3{}m", a).into_bytes()
    } else {
        // 90..=97 sets the high bit directly and is not bold-dependent.
        format!("\x1b[{}m", 90 + (a - 8)).into_bytes()
    }
}

/// The escape for a background index, given how the high bit will be read.
fn bg_seq(idx: u8, ice: Ice) -> Vec<u8> {
    let idx = if ice == Ice::On { idx & 15 } else { idx & 7 };
    let a = ansi_of(idx);
    if a < 8 {
        format!("\x1b[{}m", 40 + a).into_bytes()
    } else {
        // Only reachable with iCE on: 100..=107.
        format!("\x1b[{}m", 100 + (a - 8)).into_bytes()
    }
}

/// Serialise a grid to `.ANS` bytes.
///
/// The output is a plain byte stream: CP437 glyph bytes, escape sequences,
/// CR/LF or absolute moves, and optionally `0x1A` + SAUCE. Colours are
/// emitted as attributes, so the same file looks right in SyncTERM, ansilove
/// and a web viewer that honours the palette.
pub fn write_ansi(art: &Art, opts: WriteOpts) -> Vec<u8> {
    let mut out = Vec::with_capacity(art.cols * art.rows * 4 + 256);
    // The pen starts at the power-on attribute, so a cell that already has
    // the default colours costs no escapes at all.
    let mut pen: Option<(u8, u8)> = None;

    for y in 0..art.rows {
        // Rows are placed absolutely in Portable, and CR/LF'd in Compact.
        match opts.style {
            Style::Portable => {
                out.extend_from_slice(format!("\x1b[{};1H", y + 1).as_bytes());
                // A cursor move does not touch the pen, so colours carry
                // across rows and a repeated style costs nothing.
            }
            Style::Compact => {}
        }

        for x in 0..art.cols {
            let mut cell = art.cells[y * art.cols + x];
            if opts.ice == Ice::Off {
                cell.bg &= 7;
            }
            // Emit only what changed, and emit nothing for a cell that
            // already matches the pen. That keeps the stream minimal *and*
            // makes write(read(x)) a fixed point: the decoded pen always
            // equals the attribute the escape set, so a re-encode writes the
            // same bytes.
            let (pf, pb) = pen.unwrap_or((DEFAULT_FG, DEFAULT_BG));
            if pf != cell.fg {
                out.extend_from_slice(&fg_seq(cell.fg));
            }
            if pb != cell.bg {
                out.extend_from_slice(&bg_seq(cell.bg, opts.ice));
            }
            pen = Some((cell.fg, cell.bg));
            // CP437 byte, verbatim. 0x00 is a NUL, not a printable glyph, so
            // substitute a space rather than truncate the file.
            out.push(if cell.ch == 0 { b' ' } else { cell.ch });
        }

        if opts.style == Style::Compact && y + 1 < art.rows {
            out.extend_from_slice(b"\r\n");
        }
    }

    out.extend_from_slice(b"\x1b[0m");
    if opts.sauce {
        out.push(0x1A);
        out.extend_from_slice(&sauce_record(art, opts));
    }
    out
}

/// Build the 128-byte SAUCE record for an art grid.
fn sauce_record(art: &Art, opts: WriteOpts) -> Vec<u8> {
    let mut r = vec![0u8; 128];
    r[..7].copy_from_slice(b"SAUCE00");
    // Fields are space-padded CP437. Narrow them to bytes we can encode.
    let put = |r: &mut Vec<u8>, at: usize, len: usize, s: &str| {
        let bytes: Vec<u8> = s.bytes().take(len).collect();
        for (i, b) in bytes.iter().enumerate() {
            r[at + i] = *b;
        }
        for i in bytes.len()..len {
            r[at + i] = b' ';
        }
    };
    let title = art.sauce.as_ref().map(|s| s.title.as_str()).unwrap_or("");
    let author = art.sauce.as_ref().map(|s| s.author.as_str()).unwrap_or("");
    let group = art.sauce.as_ref().map(|s| s.group.as_str()).unwrap_or("");
    put(&mut r, 7, 35, title);
    put(&mut r, 42, 20, author);
    put(&mut r, 62, 20, group);
    put(&mut r, 82, 8, "20260101");
    let w = (art.cols.min(u16::MAX as usize)) as u16;
    r[96..98].copy_from_slice(&w.to_le_bytes());
    r[98] = if opts.bits == 9 { 1 } else { 1 }; // font 1 = 8x16 VGA
    let mut flags = 0u8;
    if opts.ice == Ice::On {
        flags |= 1; // non-blink / iCE
    }
    if opts.bits == 9 {
        flags |= 2;
    }
    r[99] = flags;
    r[90] = 1; // Datatype: Character
    r[91] = 1; // Filetype: ANSI
    let rows = (art.rows.min(u16::MAX as usize)) as u16;
    r[104..106].copy_from_slice(&rows.to_le_bytes());
    r
}

/// A [`Sauce`] for `Art`, or a default one, so callers can set metadata.
pub fn sauce_for(art: &Art) -> Sauce {
    art.sauce.clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::palette::{Ice, TCell};
    use super::super::reader::{read_ansi, Art, ReadOpts};
    use super::*;

    fn art_of(chars: &[u8], cols: usize, rows: usize) -> Art {
        let mut cells = Vec::new();
        for i in 0..cols * rows {
            cells.push(TCell { ch: *chars.get(i).unwrap_or(&b' '), fg: 7, bg: 0 });
        }
        Art { cols, rows, cells, sauce: None, truecolor: false, used_ice: false }
    }

    #[test]
    fn round_trip_preserves_glyphs_and_colours() {
        let mut cells = Vec::new();
        for i in 0..40u8 {
            cells.push(TCell { ch: 0xB0 + (i % 16), fg: i % 16, bg: (i + 3) % 16 });
        }
        let art = Art { cols: 8, rows: 5, cells, sauce: None, truecolor: false, used_ice: true };
        for style in [Style::Portable, Style::Compact] {
            let bytes = write_ansi(&art, WriteOpts { style, ..Default::default() });
            let back = read_ansi(&bytes, ReadOpts { ice: Ice::On, use_sauce: false, width: Some(8) });
            assert_eq!((back.cols, back.rows), (8, 5), "{style:?} size");
            for y in 0..5 {
                for x in 0..8 {
                    let (a, b) = (art.cells[y * 8 + x], back.get(x, y));
                    assert_eq!((a.ch, a.fg, a.bg), (b.ch, b.fg, b.bg), "{style:?} cell {x},{y}");
                }
            }
        }
    }

    #[test]
    fn portable_places_every_row_absolutely() {
        let art = art_of(b"ABCDEFGHIJKLMNOPQRSTUVWX", 4, 3);
        let bytes = write_ansi(&art, WriteOpts { style: Style::Portable, sauce: false, ..Default::default() });
        // No bare CR/LF, since those are what band on strict clients.
        assert!(!bytes.contains(&b'\n'), "portable output must not rely on newlines");
        assert_eq!(&bytes[..6], b"\x1b[1;1H");
    }

    #[test]
    fn compact_uses_newlines_and_is_smaller() {
        let art = art_of(b"ABCDEFGHIJKLMNOPQRSTUVWX", 4, 3);
        let portable = write_ansi(&art, WriteOpts { style: Style::Portable, sauce: false, ..Default::default() });
        let compact = write_ansi(&art, WriteOpts { style: Style::Compact, sauce: false, ..Default::default() });
        assert!(compact.contains(&b'\n'));
        assert!(compact.len() < portable.len(), "compact {} vs portable {}", compact.len(), portable.len());
    }

    #[test]
    fn ice_off_folds_bright_backgrounds() {
        let art = Art {
            cols: 1,
            rows: 1,
            cells: vec![TCell { ch: b'X', fg: 7, bg: 12 }],
            sauce: None,
            truecolor: false,
            used_ice: true,
        };
        let off = write_ansi(&art, WriteOpts { ice: Ice::Off, sauce: false, ..Default::default() });
        // DOS 12 (bright red) folds to 4 (red), which is ANSI 31: ESC[41,
        // never ESC[101.
        let text = String::from_utf8_lossy(&off);
        assert!(text.contains("[41m"), "{text:?}");
        assert!(!text.contains("[101m"), "{text:?}");
    }

    #[test]
    fn bright_foreground_uses_the_high_bit_not_bold() {
        let art = Art {
            cols: 1,
            rows: 1,
            cells: vec![TCell { ch: b'X', fg: 12, bg: 0 }],
            sauce: None,
            truecolor: false,
            used_ice: false,
        };
        let bytes = write_ansi(&art, WriteOpts { sauce: false, ..Default::default() });
        let text = String::from_utf8_lossy(&bytes);
        // DOS 12 is bright red, which is ANSI 91, not 94.
        assert!(text.contains("[91m"), "bright red must be 91: {text:?}");
        assert!(!text.contains("[1m"), "must not use bold: {text:?}");
    }

    #[test]
    fn sauce_records_width_ice_and_nine_dot() {
        let art = art_of(b"AB", 2, 1);
        let bytes = write_ansi(&art, WriteOpts { bits: 9, ice: Ice::On, ..Default::default() });
        let n = bytes.len();
        assert_eq!(&bytes[n - 128..n - 121], b"SAUCE00");
        let back = read_ansi(&bytes, ReadOpts::default());
        let s = back.sauce.expect("sauce");
        assert_eq!(s.width, 2);
        assert!(s.ice(), "iCE flag");
        assert!(s.nine_dot(), "9-dot flag");
        assert_eq!(s.cell_width(), 9);
        assert_eq!(s.datatype, 1);
        assert_eq!(s.filetype, 1);
    }

    #[test]
    fn output_re_reads_idempotently() {
        // Encoding twice must be a fixed point: the second encode of the
        // decoded first must equal the first, byte for byte.
        let mut cells = Vec::new();
        for i in 0..64u8 {
            cells.push(TCell { ch: b' ' + (i % 90), fg: (i % 8), bg: (i % 16) });
        }
        let art = Art { cols: 16, rows: 4, cells, sauce: None, truecolor: false, used_ice: true };
        let opts = WriteOpts { sauce: true, ..Default::default() };
        let once = write_ansi(&art, opts);
        let decoded = read_ansi(&once, ReadOpts { use_sauce: false, width: Some(16), ..Default::default() });
        let twice = write_ansi(&decoded, opts);
        assert_eq!(once, twice, "write(read(x)) must be a fixed point");
    }

    #[test]
    fn nul_glyphs_become_spaces_not_file_truncation() {
        let art = Art {
            cols: 2,
            rows: 1,
            cells: vec![TCell { ch: 0, fg: 7, bg: 0 }, TCell { ch: b'X', fg: 7, bg: 0 }],
            sauce: None,
            truecolor: false,
            used_ice: false,
        };
        let bytes = write_ansi(&art, WriteOpts { sauce: false, ..Default::default() });
        let back = read_ansi(&bytes, ReadOpts { use_sauce: false, width: Some(2), ..Default::default() });
        assert_eq!(back.get(1, 0).ch, b'X', "a NUL must not eat the rest of the row");
    }
}

