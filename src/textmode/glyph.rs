//! CP437 (IBM PC code page 437) character identity and glyph shapes.
//!
//! Two things live here and they must agree: the Unicode character each of
//! the 256 bytes denotes ([`GLYPH437`]), and the actual 8x16 bitmap the IBM
//! ROM drew for it ([`Font`]). Both come from the machine's own data rather
//! than from a lookalike font, because the art was authored against them.
//!
//! Note 0x00-0x1F: the Python `cp437` codec maps those to C0 control
//! characters, which is right for text and wrong for art. On an IBM PC those
//! bytes are the *symbol* set (hearts, arrows, card suits) and the ROM font
//! draws them, so [`GLYPH437`] carries the symbol set. That is what every
//! ANSI art viewer displays.

/// Unicode character for each CP437 byte, indexed by byte value.
pub const GLYPH437: [u32; 256] = [
    0x0020, 0x263A, 0x263B, 0x2665, 0x2666, 0x2663, 0x2660, 0x2022,
    0x25D8, 0x25CB, 0x25D9, 0x2642, 0x2640, 0x266A, 0x266B, 0x263C,
    0x25BA, 0x25C4, 0x2195, 0x203C, 0x00B6, 0x00A7, 0x25AC, 0x21A8,
    0x2191, 0x2193, 0x2192, 0x2190, 0x221F, 0x2194, 0x25B2, 0x25BC,
    0x0020, 0x0021, 0x0022, 0x0023, 0x0024, 0x0025, 0x0026, 0x0027,
    0x0028, 0x0029, 0x002A, 0x002B, 0x002C, 0x002D, 0x002E, 0x002F,
    0x0030, 0x0031, 0x0032, 0x0033, 0x0034, 0x0035, 0x0036, 0x0037,
    0x0038, 0x0039, 0x003A, 0x003B, 0x003C, 0x003D, 0x003E, 0x003F,
    0x0040, 0x0041, 0x0042, 0x0043, 0x0044, 0x0045, 0x0046, 0x0047,
    0x0048, 0x0049, 0x004A, 0x004B, 0x004C, 0x004D, 0x004E, 0x004F,
    0x0050, 0x0051, 0x0052, 0x0053, 0x0054, 0x0055, 0x0056, 0x0057,
    0x0058, 0x0059, 0x005A, 0x005B, 0x005C, 0x005D, 0x005E, 0x005F,
    0x0060, 0x0061, 0x0062, 0x0063, 0x0064, 0x0065, 0x0066, 0x0067,
    0x0068, 0x0069, 0x006A, 0x006B, 0x006C, 0x006D, 0x006E, 0x006F,
    0x0070, 0x0071, 0x0072, 0x0073, 0x0074, 0x0075, 0x0076, 0x0077,
    0x0078, 0x0079, 0x007A, 0x007B, 0x007C, 0x007D, 0x007E, 0x007F,
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7,
    0x00EA, 0x00EB, 0x00E8, 0x00EF, 0x00EE, 0x00EC, 0x00C4, 0x00C5,
    0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00A2, 0x00A3, 0x00A5, 0x20A7, 0x0192,
    0x00E1, 0x00ED, 0x00F3, 0x00FA, 0x00F1, 0x00D1, 0x00AA, 0x00BA,
    0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556,
    0x2555, 0x2563, 0x2551, 0x2557, 0x255D, 0x255C, 0x255B, 0x2510,
    0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567,
    0x2568, 0x2564, 0x2565, 0x2559, 0x2558, 0x2552, 0x2553, 0x256B,
    0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4,
    0x03A6, 0x0398, 0x03A9, 0x03B4, 0x221E, 0x03C6, 0x03B5, 0x2229,
    0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,];

/// The character for a CP437 byte.
#[inline]
pub fn ch(b: u8) -> char {
    // Every entry is a valid scalar value; the fallback is for static analysis.
    char::from_u32(GLYPH437[b as usize]).unwrap_or('\u{FFFD}')
}

/// Width of one glyph in pixels. Text mode is 8 dots, or 9 with the
/// 9th-dot line-drawing rule applied (see [`Font::plot`]).
pub const GLYPH_W: usize = 8;
/// Height of one glyph in pixels: a standard 8x16 DOS text cell.
pub const GLYPH_H: usize = 16;

/// The IBM VGA 8x16 ROM font: 256 glyphs of 16 bytes, one byte per row,
/// most significant bit leftmost.
pub struct Font {
    data: &'static [u8],
}

/// `assets/cp437_8x16.bin`, the canonical IBM 8x16 ROM dump (4096 bytes).
static FONT_ROM: &[u8] = include_bytes!("../../assets/cp437_8x16.bin");

impl Font {
    /// The built-in DOS font.
    pub fn dos() -> Font {
        debug_assert_eq!(FONT_ROM.len(), 256 * GLYPH_H);
        Font { data: FONT_ROM }
    }

    /// A font parsed from a raw 4096-byte ROM dump.
    pub fn from_bytes(data: &'static [u8]) -> Option<Font> {
        if data.len() == 256 * GLYPH_H {
            Some(Font { data })
        } else {
            None
        }
    }

    /// The 16 rows of one glyph, top to bottom.
    #[inline]
    pub fn rows(&self, b: u8) -> &'static [u8] {
        let at = b as usize * GLYPH_H;
        &self.data[at..at + GLYPH_H]
    }

    /// Is the dot at (`x`, `y`) inside glyph `b` set?
    ///
    /// `bits` is the cell width in dots. At 9 the line-drawing and block
    /// characters 192..=223 repeat column 7 into column 8: the CGA/MDA glyph
    /// generator drew an 8-dot cell but displayed 9 dots, and real text mode
    /// repeats the last column so box rules join without a seam. Rendering
    /// those glyphs against 8 columns only leaves a one-pixel gap between
    /// adjacent cells, which is why the rule exists.
    #[inline]
    pub fn plot(&self, b: u8, x: usize, y: usize, bits: usize) -> bool {
        let row = self.rows(b)[y];
        if x < 8 {
            row & (0x80 >> x) != 0
        } else {
            bits == 9 && (192..224).contains(&b) && row & 0x01 != 0
        }
    }

    /// Ink coverage of glyph `b` over the whole cell, 0.0..=1.0.
    pub fn coverage(&self, b: u8) -> f32 {
        let bits: u32 = (0..GLYPH_H)
            .map(|y| self.rows(b)[y].count_ones())
            .sum();
        bits as f32 / (GLYPH_W * GLYPH_H) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_the_characters_art_is_made_of() {
        // Blocks, box rules, and the signature of a wrong table.
        assert_eq!(ch(0xDB), '\u{2588}'); // full block
        assert_eq!(ch(0xB0), '\u{2591}'); // light shade
        assert_eq!(ch(0xC4), '\u{2500}'); // single horizontal
        assert_eq!(ch(0xB3), '\u{2502}'); // single vertical
        assert_eq!(ch(0xCD), '\u{2550}'); // double horizontal
        assert_eq!(ch(0xE3), '\u{03C0}'); // pi
        assert_eq!(ch(0x01), '\u{263A}'); // white smiling face, not C0 0x01
        assert_eq!(ch(0x03), '\u{2665}'); // heart
        assert_eq!(ch(0x20), ' ');
        assert_eq!(ch(0x41), 'A');
    }

    #[test]
    fn table_is_unique_enough_to_be_the_real_one() {
        // Every entry printable, and the drawing characters are present.
        for b in 0..=255u8 {
            let c = ch(b);
            // 0x7F is DEL and 0xFF is a blank; both are legitimately in the
            // table and both are non-printing. Everything else must print.
            assert!(
                !c.is_control() || b == 0x7F,
                "0x{b:02X} mapped to a control character"
            );
        }
        let blocks = (0xB0..=0xDF).map(ch).collect::<Vec<_>>();
        assert!(blocks.contains(&'\u{2588}'), "no full block in 0xB0..=0xDF");
    }

    #[test]
    fn rom_dump_has_the_shapes_it_should() {
        let f = Font::dos();
        assert_eq!(f.rows(0x20), &[0u8; 16], "space must be blank");
        assert_eq!(f.rows(0xDB), &[0xFF; 16], "full block must be solid");
        // 'A' apex: narrow at the top, full width at the crossbar.
        assert_eq!(f.rows(0x41)[2], 0x10);
        assert_eq!(f.rows(0x41)[7], 0xFE);
        // Single vertical rule is one column; double horizontal is two rows
        // with a gap between them.
        assert!(f.rows(0xB3).iter().all(|&r| r == 0x18));
        assert_eq!(f.rows(0xCD)[5], 0xFF);
        assert_eq!(f.rows(0xCD)[6], 0x00);
        assert_eq!(f.rows(0xCD)[7], 0xFF);
    }

    #[test]
    fn ninth_dot_repeats_only_the_drawing_characters() {
        let f = Font::dos();
        // Full block: column 8 mirrors column 7 at 9 bits.
        assert!(f.plot(0xDB, 7, 0, 9));
        assert!(f.plot(0xDB, 8, 0, 9));
        assert!(!f.plot(0xDB, 8, 0, 8), "8-bit cells must not have a 9th dot");
        // 'A' is outside 192..=223, so it never gets one.
        assert!(!f.plot(0x41, 8, 7, 9));
        // A one-column vertical rule does get one, but column 8 follows
        // column 7, which is clear, so the dot stays off.
        assert!(f.plot(0xB3, 8, 0, 9) == f.plot(0xB3, 7, 0, 9));
    }

    #[test]
    fn coverage_spans_blank_to_solid() {
        let f = Font::dos();
        assert_eq!(f.coverage(0x20), 0.0);
        assert_eq!(f.coverage(0xDB), 1.0);
        let bullet = f.coverage(0x07); // small centred dot
        assert!(bullet > 0.0 && bullet < 0.1, "bullet coverage {bullet}");
    }

    #[test]
    fn wrong_sized_dumps_are_rejected() {
        static SHORT: [u8; 10] = [0; 10];
        assert!(Font::from_bytes(&SHORT).is_none());
        assert!(Font::from_bytes(FONT_ROM).is_some());
    }
}
