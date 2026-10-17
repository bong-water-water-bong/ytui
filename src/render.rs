//! Frame -> terminal cells. Input is an RGB24 image of exactly
//! `cols*CW` x `rows*CH` pixels; output is one `Cell` per terminal cell, in
//! 24-bit colour.
//!
//! Both modes follow react-video-ascii by Lucas Jin (MIT, see NOTICE),
//! translated from its two WebGL passes (src/shaders/fragment_pass1.glsl and
//! fragment.glsl) to the terminal:
//!
//! - Shape (pass 1): sample the cell's luma in six circles, raise each
//!   sample relative to the cell's brightest (`max * (s / max)^contrast`,
//!   the reference's shape exponent, 2.0 by default) so edges sharpen, then
//!   take the character with the nearest shape vector.
//! - Colour (pass 2): the cell's colour has its HSL lightness scaled by
//!   `bright` and saturation by `sat`. The glyph is drawn in that colour
//!   over the same colour times `bg_level` (the reference's `bgOpacity`):
//!   a terminal draws a glyph exactly as the shader's
//!   `mix(bgColor, cellColor, glyphMask)`.
//!
//! Braille is ytui's own extension of the same look: the 2x4 dots take the
//! place of the character set. Each dot's luma goes through the same shape
//! step and is then compared with an ordered-dither threshold, so a cell
//! lights more dots the brighter it is and the dots follow edges.

use crate::glyphs::{self, Glyphs, CH, CIRCLES, CW};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: [0; 3], bg: [0; 3] }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mode {
    /// 2x4 braille dots in boosted colour over a dimmed copy of the picture.
    Braille,
    /// Shape-matched ASCII in boosted colour over a dimmed copy of the
    /// picture: the react-video-ascii look.
    Glow,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Braille, Mode::Glow];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Braille => "braille",
            Mode::Glow => "glow",
        }
    }

    /// Parse a mode name or a prefix of one.
    pub fn parse(s: &str) -> Option<Mode> {
        let want = s.to_ascii_lowercase();
        Mode::ALL.iter().copied().find(|m| !want.is_empty() && m.name().starts_with(&want))
    }

    pub fn next(self) -> Mode {
        let i = Mode::ALL.iter().position(|&m| m == self).unwrap();
        Mode::ALL[(i + 1) % Mode::ALL.len()]
    }
}

pub struct Img<'a> {
    pub w: usize,
    pub h: usize,
    pub px: &'a [u8],
}

pub struct Renderer {
    pub mode: Mode,
    /// Look panel: HSL lightness and saturation multipliers, how much of the
    /// picture shows behind the glyphs (0 = black, 1 = full), and the shape
    /// exponent.
    pub bright: f32,
    pub sat: f32,
    pub bg_level: f32,
    pub contrast: f32,
    glyphs: Glyphs,
    circles: Vec<Vec<(i32, i32)>>,
    /// Quantised shape vector -> glyph index.
    cache: HashMap<u32, u8>,
}

/// Braille dot bit for dot (col, row) in a 2x4 grid.
const BRAILLE_BIT: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// Ordered-dither thresholds for the 2x4 dots, indexed [row][col].
const DITHER: [[f32; 2]; 4] = [[0.5 / 8.0, 4.5 / 8.0], [6.5 / 8.0, 2.5 / 8.0], [1.5 / 8.0, 5.5 / 8.0], [7.5 / 8.0, 3.5 / 8.0]];

/// Luma with the reference's (Rec. 601) weights, 0..=1.
#[inline]
fn luma(p: &[u8]) -> f32 {
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
}

/// The shape step: `max * (s / max)^exp` for each sample.
fn sharpen(v: &mut [f32], exp: f32) {
    let max = v.iter().copied().fold(0f32, f32::max);
    if max > 0.0 {
        v.iter_mut().for_each(|s| *s = (*s / max).powf(exp) * max);
    }
}

fn rgb2hsl(c: [f32; 3]) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    let l = (mx + mn) * 0.5;
    let d = mx - mn;
    let (mut h, mut s) = (0.0, 0.0);
    if d > 0.0 {
        s = d / (1.0 - (2.0 * l - 1.0).abs());
        h = if mx == c[0] {
            ((c[1] - c[2]) / d).rem_euclid(6.0) / 6.0
        } else if mx == c[1] {
            ((c[2] - c[0]) / d + 2.0) / 6.0
        } else {
            ((c[0] - c[1]) / d + 4.0) / 6.0
        };
    }
    [h, s, l]
}

fn hsl2rgb(c: [f32; 3]) -> [f32; 3] {
    let [h, s, l] = c;
    let a = s * (1.0 - (2.0 * l - 1.0).abs());
    let ch = |o: f32| ((((h * 6.0 + o).rem_euclid(6.0)) - 3.0).abs() - 1.0).clamp(0.0, 1.0);
    [l + a * (ch(0.0) - 0.5), l + a * (ch(4.0) - 0.5), l + a * (ch(2.0) - 0.5)]
}

/// The reference's colour grade: lightness x `bright`, saturation x `sat`.
fn grade(c: [f32; 3], bright: f32, sat: f32) -> [f32; 3] {
    let mut hsl = rgb2hsl(c);
    hsl[2] = (hsl[2] * bright).clamp(0.0, 1.0);
    hsl[1] = (hsl[1] * sat).clamp(0.0, 1.0);
    hsl2rgb(hsl)
}

fn to_u8(c: [f32; 3], k: f32) -> [u8; 3] {
    let q = |v: f32| (v * k * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    [q(c[0]), q(c[1]), q(c[2])]
}

/// Mean colour (0..=1) of a pixel box.
fn box_avg(img: &Img, x: usize, y: usize, w: usize, h: usize) -> [f32; 3] {
    let mut s = [0u32; 3];
    for yy in y..y + h {
        let row = &img.px[(yy * img.w + x) * 3..(yy * img.w + x + w) * 3];
        for p in row.chunks_exact(3) {
            s[0] += p[0] as u32;
            s[1] += p[1] as u32;
            s[2] += p[2] as u32;
        }
    }
    let n = (w * h) as f32 * 255.0;
    [s[0] as f32 / n, s[1] as f32 / n, s[2] as f32 / n]
}

impl Renderer {
    pub fn new(mode: Mode) -> Renderer {
        let glyphs = Glyphs::load();
        let circles = CIRCLES.iter().map(|&(u, v)| glyphs::circle_offsets(u, v, CW, CH)).collect();
        // The reference's defaults.
        Renderer { mode, bright: 1.0, sat: 1.0, bg_level: 0.3, contrast: 2.0, glyphs, circles, cache: HashMap::new() }
    }

    pub fn render(&mut self, img: &Img, cols: usize, rows: usize, out: &mut Vec<Cell>) {
        out.clear();
        out.resize(cols * rows, Cell::default());
        if img.w < cols * CW || img.h < rows * CH || img.px.len() < img.w * img.h * 3 {
            return;
        }
        match self.mode {
            Mode::Glow => self.glow(img, cols, rows, out),
            Mode::Braille => self.braille(img, cols, rows, out),
        }
    }

    /// Glyph colour and ground colour for a cell. The shader samples the
    /// video once at the cell centre; the centre 2x2 pixels of the decoded
    /// cell are that same bilinear sample.
    fn colours(&self, img: &Img, cx: usize, cy: usize) -> ([u8; 3], [u8; 3]) {
        let c = grade(box_avg(img, cx * CW + CW / 2 - 1, cy * CH + CH / 2 - 1, 2, 2), self.bright, self.sat);
        (to_u8(c, 1.0), to_u8(c, self.bg_level))
    }

    fn glow(&mut self, img: &Img, cols: usize, rows: usize, out: &mut [Cell]) {
        if self.cache.len() > 1 << 20 {
            self.cache.clear();
        }
        let (w, h) = (img.w as i32, img.h as i32);
        for cy in 0..rows {
            for cx in 0..cols {
                let (ox, oy) = ((cx * CW) as i32, (cy * CH) as i32);
                let mut v = [0f32; 6];
                for (i, pts) in self.circles.iter().enumerate() {
                    let mut s = 0.0;
                    for &(dx, dy) in pts {
                        let x = (ox + dx).clamp(0, w - 1) as usize;
                        let y = (oy + dy).clamp(0, h - 1) as usize;
                        s += luma(&img.px[(y * img.w + x) * 3..]);
                    }
                    v[i] = s / pts.len() as f32;
                }
                sharpen(&mut v, self.contrast);
                let key = v.iter().fold(0u32, |k, &x| k << 5 | (x.clamp(0.0, 1.0) * 31.0 + 0.5) as u32);
                let gi = *self.cache.entry(key).or_insert_with(|| self.glyphs.nearest(&v) as u8) as usize;
                let (fg, bg) = self.colours(img, cx, cy);
                out[cy * cols + cx] = Cell { ch: self.glyphs.chars[gi], fg, bg };
            }
        }
    }

    fn braille(&mut self, img: &Img, cols: usize, rows: usize, out: &mut [Cell]) {
        let (dw, dh) = (CW / 2, CH / 4);
        for cy in 0..rows {
            for cx in 0..cols {
                let mut v = [0f32; 8];
                let mut c = [[0f32; 3]; 8];
                for j in 0..4 {
                    for i in 0..2 {
                        let k = j * 2 + i;
                        c[k] = box_avg(img, cx * CW + i * dw, cy * CH + j * dh, dw, dh);
                        v[k] = 0.299 * c[k][0] + 0.587 * c[k][1] + 0.114 * c[k][2];
                    }
                }
                sharpen(&mut v, self.contrast);
                // Dots take the colour of the dots that light, the ground the
                // colour of the ones that don't, so edges keep both sides.
                let (mut bits, mut on, mut off, mut n) = (0, [0f32; 3], [0f32; 3], 0);
                for j in 0..4 {
                    for i in 0..2 {
                        let k = j * 2 + i;
                        let lit = v[k] > DITHER[j][i];
                        let acc = if lit { &mut on } else { &mut off };
                        for d in 0..3 {
                            acc[d] += c[k][d];
                        }
                        if lit {
                            bits |= BRAILLE_BIT[i][j];
                            n += 1;
                        }
                    }
                }
                let mean = |a: [f32; 3], k: usize| if k == 0 { None } else { Some([a[0] / k as f32, a[1] / k as f32, a[2] / k as f32]) };
                let all = mean([on[0] + off[0], on[1] + off[1], on[2] + off[2]], 8).unwrap();
                let fg = grade(mean(on, n).unwrap_or(all), self.bright, self.sat);
                let bg = grade(mean(off, 8 - n).unwrap_or(all), self.bright, self.sat);
                out[cy * cols + cx] = Cell { ch: char::from_u32(0x2800 + bits).unwrap(), fg: to_u8(fg, 1.0), bg: to_u8(bg, self.bg_level) };
            }
        }
    }

    /// Thumbnails: half blocks (two colours per cell, top and bottom), the
    /// most legible picture at the small sizes thumbnails are drawn at.
    pub fn thumb(img: &Img, cols: usize, rows: usize, out: &mut Vec<Cell>) {
        out.clear();
        out.resize(cols * rows, Cell::default());
        if img.w < cols * CW || img.h < rows * CH || img.px.len() < img.w * img.h * 3 {
            return;
        }
        for cy in 0..rows {
            for cx in 0..cols {
                let (x, y) = (cx * CW, cy * CH);
                let top = box_avg(img, x, y, CW, CH / 2);
                let bot = box_avg(img, x, y + CH / 2, CW, CH / 2);
                out[cy * cols + cx] = Cell { ch: '▀', fg: to_u8(top, 1.0), bg: to_u8(bot, 1.0) };
            }
        }
    }
}

/// Auto-exposure for video: measures each frame's brightness spread (1st to
/// 99th percentile of luma) and, when the frame is under-exposed, lifts it
/// so its highlights reach white, with one gain and offset for all three
/// channels (so hues hold). Well-exposed frames are left as they are. The
/// correction eases in over a few frames so it doesn't pump, and the gain is
/// capped so fades to black don't turn into amplified noise.
pub struct Exposure {
    pub on: bool,
    lo: f32,
    gain: f32,
    primed: bool,
}

const MAX_GAIN: f32 = 3.5;
const MAX_OFFSET: f32 = 0.35;

impl Exposure {
    pub fn new(on: bool) -> Exposure {
        Exposure { on, lo: 0.0, gain: 1.0, primed: false }
    }

    /// Forget history (after a seek or a new video) so the next frame applies at once.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    /// Current (offset, gain), for display / tests.
    pub fn params(&self) -> (f32, f32) {
        (self.lo, self.gain)
    }

    /// Update the correction from a new RGB24 frame. `blend` is how much of
    /// the new measurement to take (1.0 = jump straight to it).
    pub fn measure(&mut self, px: &[u8], blend: f32) {
        let mut hist = [0u32; 256];
        let mut n = 0u32;
        // Every 7th pixel is plenty for percentiles and keeps this cheap.
        for p in px.chunks_exact(3).step_by(7) {
            let l = (54 * p[0] as u32 + 183 * p[1] as u32 + 19 * p[2] as u32) >> 8;
            hist[l.min(255) as usize] += 1;
            n += 1;
        }
        if n == 0 {
            return;
        }
        let pct = |q: f32| {
            let target = (n as f32 * q) as u32;
            let mut acc = 0;
            for (i, &c) in hist.iter().enumerate() {
                acc += c;
                if acc > target {
                    return i as f32 / 255.0;
                }
            }
            1.0
        };
        let (lo, hi) = (pct(0.01), pct(0.99));
        // Only correct under-exposure: a frame whose highlights already
        // reach near-white is left alone (haze or low contrast there is
        // usually intentional). The darker the highlights, the more the
        // black level is also pulled down.
        let dark = ((0.85 - hi) / 0.35).clamp(0.0, 1.0);
        let lo = (lo * 0.5 * dark).min(MAX_OFFSET);
        let gain = (1.0 / (hi - lo).max(1e-3)).clamp(1.0, MAX_GAIN);
        let b = if self.primed { blend.clamp(0.0, 1.0) } else { 1.0 };
        self.lo += (lo - self.lo) * b;
        self.gain += (gain - self.gain) * b;
        self.primed = true;
    }

    /// Apply the current correction in place.
    pub fn apply(&self, px: &mut [u8]) {
        if !self.on || (self.gain < 1.02 && self.lo < 0.01) {
            return;
        }
        let mut lut = [0u8; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = (((i as f32 / 255.0 - self.lo) * self.gain).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        px.iter_mut().for_each(|c| *c = lut[*c as usize]);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn flat(cols: usize, rows: usize, c: [u8; 3]) -> Vec<u8> {
        c.repeat(cols * CW * rows * CH)
    }

    fn render(mode: Mode, px: &[u8], cols: usize, rows: usize) -> Vec<Cell> {
        let mut r = Renderer::new(mode);
        let mut out = Vec::new();
        r.render(&Img { w: cols * CW, h: rows * CH, px }, cols, rows, &mut out);
        out
    }

    #[test]
    fn modes_parse_by_prefix_and_cycle() {
        assert_eq!(Mode::parse("b"), Some(Mode::Braille));
        assert_eq!(Mode::parse("GLOW"), Some(Mode::Glow));
        assert_eq!(Mode::parse(""), None);
        assert_eq!(Mode::parse("sextant"), None);
        assert_eq!(Mode::Braille.next(), Mode::Glow);
        assert_eq!(Mode::Glow.next(), Mode::Braille);
    }

    #[test]
    fn grade_is_identity_at_one_and_scales_lightness() {
        for c in [[0.2, 0.5, 0.9], [1.0, 0.0, 0.0], [0.3, 0.3, 0.3], [0.0, 0.0, 0.0]] {
            let g = grade(c, 1.0, 1.0);
            for d in 0..3 {
                assert!((g[d] - c[d]).abs() < 1e-5, "{c:?} -> {g:?}");
            }
        }
        let grey = grade([0.4, 0.4, 0.4], 1.5, 1.0);
        assert!((grey[0] - 0.6).abs() < 1e-5);
        let desat = grade([1.0, 0.0, 0.0], 1.0, 0.0);
        assert!((desat[0] - desat[1]).abs() < 1e-5 && (desat[1] - desat[2]).abs() < 1e-5);
    }

    #[test]
    fn glow_black_is_empty_and_white_is_dense_over_a_dimmed_ground() {
        let out = render(Mode::Glow, &flat(4, 2, [0; 3]), 4, 2);
        assert!(out.iter().all(|c| c.ch == ' ' && c.bg == [0; 3]));
        let out = render(Mode::Glow, &flat(4, 2, [255; 3]), 4, 2);
        for c in &out {
            assert_ne!(c.ch, ' ');
            assert_eq!(c.fg, [255; 3]);
            assert_eq!(c.bg, [77; 3], "ground is 0.3 of the colour");
        }
    }

    #[test]
    fn braille_lights_dots_in_proportion_to_brightness() {
        let dots = |v: u8| {
            let out = render(Mode::Braille, &flat(1, 1, [v; 3]), 1, 1);
            (out[0].ch as u32 - 0x2800).count_ones()
        };
        assert_eq!(dots(0), 0);
        assert_eq!(dots(255), 8);
        assert_eq!(dots(128), 4);
        let mut last = 0;
        for v in (0..=255).step_by(16) {
            let n = dots(v as u8);
            assert!(n >= last, "dot count must not drop as brightness rises");
            last = n;
        }
    }

    #[test]
    fn braille_dots_follow_an_edge() {
        // Left half of the cell white, right half black: only the left
        // column of dots (bits 0x01 0x02 0x04 0x40) may light.
        let mut px = vec![0u8; CW * CH * 3];
        for y in 0..CH {
            for x in 0..CW / 2 {
                px[(y * CW + x) * 3..(y * CW + x) * 3 + 3].copy_from_slice(&[255; 3]);
            }
        }
        let out = render(Mode::Braille, &px, 1, 1);
        assert_eq!(out[0].ch as u32 - 0x2800, 0x01 | 0x02 | 0x04 | 0x40);
    }

    #[test]
    fn exposure_lifts_a_dark_frame_and_leaves_a_bright_one() {
        let mut e = Exposure::new(true);
        let mut dark: Vec<u8> = (0..3000).map(|i| (i % 100) as u8).collect();
        e.measure(&dark, 1.0);
        let before = dark.iter().map(|&v| v as u32).max().unwrap();
        e.apply(&mut dark);
        assert!(dark.iter().map(|&v| v as u32).max().unwrap() > before);
        let mut e = Exposure::new(true);
        let mut bright: Vec<u8> = (0..3000).map(|i| (i % 256) as u8).collect();
        let copy = bright.clone();
        e.measure(&bright, 1.0);
        e.apply(&mut bright);
        assert_eq!(bright, copy);
    }
}
