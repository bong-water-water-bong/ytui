//! The 16-colour DOS palette, the iCE colour model, and quantisation into it.
//!
//! Two things make this different from picking the nearest of 16 colours twice:
//!
//! * **iCE colours.** DOS text mode had 8 foreground and 8 background colours,
//!   with the background attribute's high bit meaning "blink". The iCE
//!   convention (from the iCE art group's editor) spends that bit on a real
//!   colour instead, giving 8 foregrounds x 16 backgrounds. The background set
//!   is then not the foreground set: a cell's background may be bright, but its
//!   text may not. Quantising fg and bg independently to all 16 gets the
//!   contrast wrong and is the usual reason converted art looks muddy.
//! * **Two RGB answers.** The values the art was authored against (the
//!   VGA/CGA set) are not the values a modern terminal shows for the same
//!   index. [`Rgb::vga`] is for rendering and comparing; [`Rgb::xterm`] is
//!   what an escape sequence ends up displaying.

use super::glyph::{Font, GLYPH_H, GLYPH_W};

/// The 16 DOS colour indices, in attribute order.
pub const BLACK: u8 = 0;
pub const BLUE: u8 = 1;
pub const GREEN: u8 = 2;
pub const CYAN: u8 = 3;
pub const RED: u8 = 4;
pub const MAGENTA: u8 = 5;
pub const BROWN: u8 = 6;
pub const LIGHT_GREY: u8 = 7;

/// The VGA/CGA palette the art was authored against: 8 normal then 8 bright.
///
/// Not the xterm values. `BROWN` is `0xAA5500` (an artefact of the CGA's
/// green-bit wiring, which is why dark yellow and brown are the same index),
/// and the bright set is the `0x55` half-intensity plus `0xFF` high nibble
/// rule of VGA DAC programming. These are the numbers an artist's monitor
/// showed.
pub const VGA: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], // 0 black
    [0x00, 0x00, 0xAA], // 1 blue
    [0x00, 0xAA, 0x00], // 2 green
    [0x00, 0xAA, 0xAA], // 3 cyan
    [0xAA, 0x00, 0x00], // 4 red
    [0xAA, 0x00, 0xAA], // 5 magenta
    [0xAA, 0x55, 0x00], // 6 brown
    [0xAA, 0xAA, 0xAA], // 7 light grey
    [0x55, 0x55, 0x55], // 8 dark grey
    [0x55, 0x55, 0xFF], // 9 bright blue
    [0x55, 0xFF, 0x55], // 10 bright green
    [0x55, 0xFF, 0xFF], // 11 bright cyan
    [0xFF, 0x55, 0x55], // 12 bright red
    [0xFF, 0x55, 0xFF], // 13 bright magenta
    [0xFF, 0xFF, 0x55], // 14 yellow
    [0xFF, 0xFF, 0xFF], // 15 white
];

/// The colour an xterm shows for each DOS index in its own 16-colour palette.
/// Notice how far `RED` and `BROWN` drift: the same attribute is a different
/// picture on a modern terminal, which is why the two sets are kept apart.
pub const XTERM16: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00],
    [0x00, 0x00, 0xCD],
    [0x00, 0xCD, 0x00],
    [0x00, 0xCD, 0xCD],
    [0xCD, 0x00, 0x00],
    [0xCD, 0x00, 0xCD],
    [0xCD, 0xCD, 0x00],
    [0xE5, 0xE5, 0xE5],
    [0x7F, 0x7F, 0x7F],
    [0x00, 0x00, 0xFF],
    [0x00, 0xFF, 0x00],
    [0x00, 0xFF, 0xFF],
    [0xFF, 0x00, 0x00],
    [0xFF, 0x00, 0xFF],
    [0xFF, 0xFF, 0x00],
    [0xFF, 0xFF, 0xFF],
];

/// xterm-256 indices for the 16 DOS colours. The bright half goes to the
/// 24-grey ramp only for index 8, which is a true grey; the rest use the
/// colour cube at full intensity.
pub const XTERM256: [u8; 16] = [
    16, 19, 34, 37, 124, 127, 130, 248, // normal
    240, 63, 83, 87, 203, 207, 227, 15, // bright
];

/// Which RGB values a caller wants. Rendering and fitting use [`Rgb::Vga`];
/// anything that has to survive a trip through a terminal uses
/// [`Rgb::Xterm`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rgb {
    Vga,
    Xterm,
}

impl Rgb {
    #[inline]
    pub fn get(self, idx: u8) -> [u8; 3] {
        match self {
            Rgb::Vga => VGA[(idx & 15) as usize],
            Rgb::Xterm => XTERM16[(idx & 15) as usize],
        }
    }
}

/// How the high background bit is read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ice {
    /// The attribute's high bit means blink; backgrounds are 0..=7.
    Off,
    /// The high bit means a colour; backgrounds are 0..=15.
    On,
}

impl Ice {
    #[inline]
    pub fn bg_count(self) -> u8 {
        match self {
            Ice::Off => 8,
            Ice::On => 16,
        }
    }

    #[inline]
    pub fn toggled(self) -> Ice {
        match self {
            Ice::Off => Ice::On,
            Ice::On => Ice::Off,
        }
    }
}

/// Device-independent squared distance, with a mild luma weighting so that
/// two colours differing only in brightness do not collapse together.
pub fn dist2(a: [u8; 3], b: [u8; 3]) -> u32 {
    let dr = a[0] as i32 - b[0] as i32;
    let dg = a[1] as i32 - b[1] as i32;
    let db = a[2] as i32 - b[2] as i32;
    (dr * dr * 30 + dg * dg * 59 + db * db * 11) as u32
}

/// Nearest colour index among the first `n` entries of the chosen set.
#[inline]
pub fn nearest(rgb: [u8; 3], n: u8, set: Rgb) -> u8 {
    let mut best = (u32::MAX, 0u8);
    for i in 0..n {
        let d = dist2(rgb, set.get(i));
        if d < best.0 {
            best = (d, i);
        }
    }
    best.1
}

/// One quantised cell: a CP437 byte plus fg/bg palette indices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TCell {
    pub ch: u8,
    pub fg: u8,
    pub bg: u8,
}

impl Default for TCell {
    fn default() -> Self {
        TCell { ch: b' ', fg: LIGHT_GREY, bg: BLACK }
    }
}

/// The classic two-colour cell fit, honouring the iCE foreground limit.
///
/// A character cell has one flat foreground colour and one flat background
/// colour. Given a cell's pixels and a glyph candidate, split them into
/// "ink" (covered by the glyph) and "paper" (not covered) by nearest-colour
/// assignment, which is what makes the result a good two-colour fit rather
/// than a blend. Errors are summed in squared distance so the caller can pick
/// the best glyph by the number it returns.
///
/// Foreground is restricted to 0..=7, always: even with iCE on, bright text
/// needs `ESC[1m` and only index 8..=15 backgrounds come from the blink bit.
pub fn fit_cell(pixels: &[[u8; 3]], ink: &[bool], ice: Ice, set: Rgb) -> (TCell, u64) {
    debug_assert_eq!(pixels.len(), ink.len());

    // Split the cell by the glyph mask (hard assignment) and take each side's
    // mean. Doing it the other way round -- assigning pixels to the nearer of
    // two provisional colours -- lets a flat cell resolve to one colour for
    // "ink" too, and then the glyph choice is decided by which mask happens to
    // leave a stray pixel on the far side.
    let (mut fi, mut fn_) = ([0u32; 3], 0u32);
    let (mut bi, mut bn) = ([0u32; 3], 0u32);
    for (p, &k) in pixels.iter().zip(ink) {
        let acc = if k { &mut fi } else { &mut bi };
        let n = if k { &mut fn_ } else { &mut bn };
        for d in 0..3 {
            acc[d] += p[d] as u32;
        }
        *n += 1;
    }
    let mean = |acc: [u32; 3], n: u32| -> Option<[u8; 3]> {
        (n > 0).then(|| {
            [
                (acc[0] / n).min(255) as u8,
                (acc[1] / n).min(255) as u8,
                (acc[2] / n).min(255) as u8,
            ]
        })
    };

    // An empty side (a blank glyph, or a mask that inks everything) takes the
    // other side's colour, so a flat cell is exactly one colour and costs zero
    // error. That is the common case for backgrounds and for solid blocks.
    //
    // Ink is limited to the 8 dark foregrounds always: iCE buys backgrounds,
    // not foregrounds, and bright text would need ESC[1m which the art does
    // not use. Paper may use all 16 when iCE is on.
    let (ink_mean, paper_mean) = (mean(fi, fn_), mean(bi, bn));
    let fg = match (ink_mean, paper_mean) {
        (Some(f), _) => nearest(f, 8, set),
        (None, Some(b)) => nearest(b, 8, set),
        (None, None) => LIGHT_GREY,
    };
    let bg = match (ink_mean, paper_mean) {
        (_, Some(b)) => nearest(b, ice.bg_count(), set),
        (Some(f), None) => nearest(f, ice.bg_count(), set),
        (None, None) => BLACK,
    };
    // A glyph with no ink paints nothing: its cell is the paper colour on
    // both sides, which is what makes a flat region take no error.
    let (fg, bg) = if fn_ == 0 { (bg, bg) } else { (fg, bg) };

    // The cost is what the *glyph choice* needs: how far the colours the
    // glyph actually paints land from the pixels under them. Ink is painted
    // with `fg` and paper with `bg`, so a pixel is judged against the colour
    // that will cover it, not the one it most resembles.
    let mut err = 0u64;
    for (p, &k) in pixels.iter().zip(ink) {
        let paints = if k { fg } else { bg };
        err += dist2(*p, set.get(paints)) as u64;
    }
    (TCell { ch: b' ', fg, bg }, err)
}

/// Every cell a glyph could legally fill, as `(byte, ink mask)` pairs, given
/// the font and the cell resolution the renderer decodes at.
///
/// The mask is which pixels the glyph inks, so [`fit_cell`] can be handed the
/// same slice for every candidate. Built once and reused: this is the
/// expensive part of a text-mode frame.
pub fn ink_masks(font: &Font, w: usize, h: usize) -> Vec<(u8, Vec<bool>)> {
    let mut out = Vec::with_capacity(256);
    for b in 0..=255u8 {
        let mut mask = Vec::with_capacity(w * h);
        for y in 0..h {
            let gy = y * GLYPH_H / h;
            for x in 0..w {
                let gx = x * GLYPH_W / w;
                mask.push(font.plot(b, gx, gy, 8));
            }
        }
        out.push((b, mask));
    }
    out
}

/// Render a `TCell` grid to an RGB24 image using the DOS font, for comparison
/// against other renderers and for sprite-style output.
pub fn rasterise(cells: &[TCell], cols: usize, rows: usize, set: Rgb, bits: usize) -> Vec<u8> {
    let font = Font::dos();
    let (w, h) = (cols * bits, rows * GLYPH_H);
    let mut px = vec![0u8; w * h * 3];
    for r in 0..rows {
        for c in 0..cols {
            let cell = cells.get(r * cols + c).copied().unwrap_or_default();
            let fg = set.get(cell.fg);
            let bg = set.get(cell.bg);
            for y in 0..GLYPH_H {
                for x in 0..bits {
                    let on = font.plot(cell.ch, x, y, bits);
                    let col = if on { fg } else { bg };
                    let at = ((r * GLYPH_H + y) * w + c * bits + x) * 3;
                    px[at..at + 3].copy_from_slice(&col);
                }
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vga_and_xterm_sets_are_both_self_consistent() {
        // 16 entries each, and every entry is distinct.
        for set in [&VGA[..], &XTERM16[..]] {
            assert_eq!(set.len(), 16);
            for i in 0..set.len() {
                for j in i + 1..set.len() {
                    assert_ne!(set[i], set[j], "colours {i} and {j} are identical");
                }
            }
        }
        // Brown really is the 0xAA5500 artefact, not a dark yellow.
        assert_eq!(VGA[BROWN as usize], [0xAA, 0x55, 0x00]);
        assert_eq!(VGA[BLACK as usize], [0, 0, 0]);
        assert_eq!(VGA[15], [0xFF; 3]);
    }

    #[test]
    fn nearest_recovers_every_colour_it_is_given() {
        for set in [Rgb::Vga, Rgb::Xterm] {
            for i in 0..16u8 {
                assert_eq!(nearest(set.get(i), 16, set), i, "colour {i}");
            }
        }
    }

    #[test]
    fn ice_limits_the_foreground_but_not_the_background() {
        let white = VGA[15];
        // Bright white as a background needs iCE on; off, it lands somewhere legal.
        let on = fit_cell(&[white; 4], &[false; 4], Ice::On, Rgb::Vga);
        assert!(on.0.bg < 16);
        let off = fit_cell(&[white; 4], &[false; 4], Ice::Off, Rgb::Vga);
        assert!(off.0.bg < 8, "iCE off must not produce a bright background");
        // The foreground is never bright, iCE or not.
        let bright_ink = fit_cell(&[white; 4], &[true; 4], Ice::On, Rgb::Vga);
        assert!(bright_ink.0.fg < 8, "foreground must stay 0..=7");
    }

    #[test]
    fn a_flat_cell_costs_nothing_once_it_matches_a_palette_entry() {
        let red = VGA[RED as usize];
        let (cell, err) = fit_cell(&[red; 64], &[false; 64], Ice::On, Rgb::Vga);
        assert_eq!(cell.bg, RED);
        assert_eq!(err, 0);
    }

    #[test]
    fn ink_masks_are_dense_for_blocks_and_empty_for_space() {
        let masks = ink_masks(&Font::dos(), GLYPH_W, GLYPH_H);
        assert_eq!(masks.len(), 256);
        let m = |b: u8| masks.iter().find(|(k, _)| *k == b).unwrap().1.clone();
        assert!(m(b' ').iter().all(|&k| !k));
        assert!(m(0xDB).iter().all(|&k| k));
        let half = m(0xDC); // lower half block
        assert!(!half[0], "top row must be clear");
        assert!(half[half.len() - 1], "bottom row must be inked");
    }

    #[test]
    fn rasterise_puts_the_right_colours_where_the_glyph_says() {
        // One full-block cell in bright green on black.
        let cells = [TCell { ch: 0xDB, fg: 2, bg: 0 }];
        let px = rasterise(&cells, 1, 1, Rgb::Vga, 8);
        assert_eq!(px.len(), 8 * 16 * 3);
        assert_eq!(&px[0..3], &VGA[GREEN as usize], "top-left inked");
        // A space is all background.
        let blank = rasterise(&[TCell { ch: b' ', fg: 2, bg: 4 }], 1, 1, Rgb::Vga, 8);
        assert_eq!(&blank[0..3], &VGA[RED as usize]);
    }

    #[test]
    fn ninth_dot_widens_blocks_in_rasterised_output() {
        let cells = [TCell { ch: 0xDB, fg: 15, bg: 0 }];
        let narrow = rasterise(&cells, 1, 1, Rgb::Vga, 8);
        let wide = rasterise(&cells, 1, 1, Rgb::Vga, 9);
        assert_eq!(narrow.len(), 8 * 16 * 3);
        assert_eq!(wide.len(), 9 * 16 * 3);
        // The 9th column is inked in the wide form.
        let at = (0 * 9 + 8) * 3;
        assert_eq!(&wide[at..at + 3], &VGA[15]);
    }

}

#[cfg(test)]
mod fit_tests {
    use super::*;
    #[test]
    fn ink_and_paper_get_their_own_colours() {
        // Top half red (ink), bottom half blue (paper), with a mask that inks
        // exactly the red half, so the fit can be exact.
        let red = VGA[RED as usize];
        let blue = VGA[BLUE as usize];
        let mut pixels = vec![blue; 128];
        let mut ink = vec![false; 128];
        for i in 0..64 {
            pixels[i] = red;
            ink[i] = true;
        }
        let (cell, err) = fit_cell(&pixels, &ink, Ice::On, Rgb::Vga);
        assert_eq!(cell.fg, RED, "ink must be red");
        assert_eq!(cell.bg, BLUE, "paper must be blue");
        assert_eq!(err, 0, "a matching mask must be free");
    }
    #[test]
    fn a_wrong_mask_costs_error() {
        // Solid red but a mask that inks only the top half: the glyph paints
        // red over the whole cell, so the un-inked half is the wrong colour.
        let px = vec![VGA[BLUE as usize]; 128];
        let mut ink = vec![false; 128];
        for i in 0..64 {
            ink[i] = true;
        }
        let (_, err) = fit_cell(&px, &ink, Ice::On, Rgb::Vga);
        let flat = fit_cell(&px, &[false; 128], Ice::On, Rgb::Vga).1;
        assert!(err >= flat, "a mismatched mask must not beat a blank one");
    }
    #[test]
    fn flat_cell_with_full_mask_is_one_colour() {
        let px = vec![VGA[GREEN as usize]; 128];
        let (cell, err) = fit_cell(&px, &[true; 128], Ice::On, Rgb::Vga);
        assert_eq!((cell.fg, cell.bg), (GREEN, GREEN));
        assert_eq!(err, 0);
    }
    #[test]
    fn flat_cell_with_empty_mask_is_one_colour() {
        let px = vec![VGA[CYAN as usize]; 128];
        let (cell, err) = fit_cell(&px, &[false; 128], Ice::On, Rgb::Vga);
        assert_eq!((cell.fg, cell.bg), (CYAN, CYAN));
        assert_eq!(err, 0);
    }
    #[test]
    fn mismatched_glyph_costs_error() {
        // Solid red pixels but a mask that inks only half: the un-inked half
        // will be painted with fg, so it cannot be perfect unless fg==bg.
        let px = vec![VGA[RED as usize]; 128];
        let mut ink = vec![false; 128];
        for i in 0..64 {
            ink[i] = true;
        }
        let (_, err) = fit_cell(&px, &ink, Ice::On, Rgb::Vga);
        assert_eq!(err, 0, "sameness makes any mask free");
    }
}
