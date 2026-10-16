//! Frame -> terminal cells. Input is an RGB24 image of exactly
//! `cols*CW` x `rows*CH` pixels; output is one `Cell` per terminal cell.

use crate::glyphs::{self, Glyphs, AFFECT, CH, CW, INNER, OUTER};
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
    /// Two-tone shape ASCII: glyphs only where there is edge detail.
    Ascii,
    /// Harri-style absolute brightness ASCII, colour ink on a dark ground.
    Classic,
    Sextant,
    Quadrant,
    HalfBlock,
    Braille,
}

impl Mode {
    pub const ALL: [Mode; 6] = [Mode::Ascii, Mode::Classic, Mode::Sextant, Mode::Quadrant, Mode::HalfBlock, Mode::Braille];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Ascii => "ascii",
            Mode::Classic => "classic",
            Mode::Sextant => "sextant",
            Mode::Quadrant => "quadrant",
            Mode::HalfBlock => "halfblock",
            Mode::Braille => "braille",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.iter().copied().find(|m| m.name().starts_with(&s.to_ascii_lowercase()))
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
    /// Tint the ASCII background with a dimmed cell colour.
    pub ascii_bg: bool,
    pub dir_exp: f32,
    pub glob_exp: f32,
    /// Two-tone ASCII: cells whose luminance range is below this are flat.
    pub flat: f32,
    /// Classic ASCII: luminance at or below this is empty space (the rest is
    /// rescaled). Used for 3D scenes so dim floors / fog don't become noise.
    pub black: f32,
    /// Classic ASCII: glyph density from the brightest channel instead of
    /// luminance, so saturated colours (a red object) still get solid glyphs.
    pub value_density: bool,
    pub glyphs: Glyphs,
    inner: Vec<Vec<(i32, i32)>>,
    outer: Vec<Vec<(i32, i32)>>,
    cache: HashMap<u32, u8>,
    lum: Vec<f32>,
}

const QUAD: [char; 16] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
];

/// Sextant glyph for a 6-bit mask (bit0 TL, 1 TR, 2 ML, 3 MR, 4 BL, 5 BR).
fn sextant(m: u32) -> char {
    match m {
        0 => ' ',
        21 => '▌',
        42 => '▐',
        63 => '█',
        _ => {
            let idx = m - 1 - (m > 21) as u32 - (m > 42) as u32;
            char::from_u32(0x1FB00 + idx).unwrap_or('?')
        }
    }
}

/// Braille dot bit for sub-pixel (col, row) in a 2x4 grid.
const BRAILLE_BIT: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

fn to_u8(c: [f32; 3]) -> [u8; 3] {
    [c[0].clamp(0.0, 255.0) as u8, c[1].clamp(0.0, 255.0) as u8, c[2].clamp(0.0, 255.0) as u8]
}

impl Renderer {
    pub fn new(mode: Mode) -> Renderer {
        let glyphs = Glyphs::load();
        let inner = INNER.iter().map(|&(u, v)| glyphs::circle_offsets(u, v, CW, CH)).collect();
        let outer = OUTER.iter().map(|&(u, v)| glyphs::circle_offsets(u, v, CW, CH)).collect();
        Renderer {
            mode,
            ascii_bg: true,
            dir_exp: 2.5,
            glob_exp: 1.8,
            flat: 0.10,
            black: 0.0,
            value_density: false,
            glyphs,
            inner,
            outer,
            cache: HashMap::new(),
            lum: Vec::new(),
        }
    }

    pub fn render(&mut self, img: &Img, cols: usize, rows: usize, out: &mut Vec<Cell>) {
        out.clear();
        out.resize(cols * rows, Cell::default());
        if img.w < cols * CW || img.h < rows * CH || img.px.len() < img.w * img.h * 3 {
            return;
        }
        match self.mode {
            Mode::Ascii => self.ascii(img, cols, rows, out, true),
            Mode::Classic => self.ascii(img, cols, rows, out, false),
            Mode::HalfBlock => {
                for cy in 0..rows {
                    for cx in 0..cols {
                        let (x, y) = (cx * CW, cy * CH);
                        let top = box_avg(img, x, y, CW, CH / 2);
                        let bot = box_avg(img, x, y + CH / 2, CW, CH / 2);
                        out[cy * cols + cx] = Cell { ch: '▀', fg: to_u8(top), bg: to_u8(bot) };
                    }
                }
            }
            Mode::Quadrant => mosaic(img, cols, rows, out, 2, 2, |m| QUAD[m as usize], false),
            Mode::Sextant => mosaic(img, cols, rows, out, 2, 3, sextant, false),
            Mode::Braille => mosaic(img, cols, rows, out, 2, 4, |m| {
                let mut b = 0;
                for i in 0..8 {
                    if m & (1 << i) != 0 {
                        b |= BRAILLE_BIT[i % 2][i / 2];
                    }
                }
                char::from_u32(0x2800 + b).unwrap()
            }, true),
        }
    }

    fn ascii(&mut self, img: &Img, cols: usize, rows: usize, out: &mut [Cell], relative: bool) {
        let (w, h) = (img.w, img.h);
        self.lum.resize(w * h, 0.0);
        let (black, scale) = if relative { (0.0, 1.0) } else { (self.black, 1.0 / (1.0 - self.black).max(0.05)) };
        for (i, p) in img.px[..w * h * 3].chunks_exact(3).enumerate() {
            let l = if self.value_density && !relative {
                p[0].max(p[1]).max(p[2]) as f32 / 255.0
            } else {
                (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0
            };
            self.lum[i] = ((l - black) * scale).max(0.0);
        }
        if self.cache.len() > 1 << 21 {
            self.cache.clear();
        }
        let px = img.px;
        // Mean luminance (and optionally colour) over a circle's pixels.
        let sample = |lum: &[f32], pts: &[(i32, i32)], ox: i32, oy: i32, rgb: Option<&mut [f32; 3]>| -> f32 {
            let mut s = 0.0;
            let mut c = [0f32; 3];
            for &(dx, dy) in pts {
                let x = (ox + dx).clamp(0, w as i32 - 1) as usize;
                let y = (oy + dy).clamp(0, h as i32 - 1) as usize;
                s += lum[y * w + x];
                let p = &px[(y * w + x) * 3..];
                c[0] += p[0] as f32;
                c[1] += p[1] as f32;
                c[2] += p[2] as f32;
            }
            let n = pts.len() as f32;
            if let Some(o) = rgb {
                *o = [c[0] / n, c[1] / n, c[2] / n];
            }
            s / n
        };
        let mut col = [[0f32; 3]; 6];
        for cy in 0..rows {
            for cx in 0..cols {
                let (ox, oy) = ((cx * CW) as i32, (cy * CH) as i32);
                let mut v = [0f32; 6];
                for i in 0..6 {
                    v[i] = sample(&self.lum, &self.inner[i], ox, oy, Some(&mut col[i]));
                }
                let mut ext = [0f32; 10];
                for i in 0..10 {
                    ext[i] = sample(&self.lum, &self.outer[i], ox, oy, None);
                }
                let avg = box_avg(img, cx * CW, cy * CH, CW, CH);
                if relative {
                    // Stretch the cell's own luminance range to 0..1 so the
                    // glyph encodes local detail, not absolute brightness.
                    let lo = v.iter().copied().fold(1f32, f32::min);
                    let hi = v.iter().copied().fold(0f32, f32::max);
                    if hi - lo < self.flat {
                        out[cy * cols + cx] = Cell { ch: ' ', fg: to_u8(avg), bg: to_u8(avg) };
                        continue;
                    }
                    let k = 1.0 / (hi - lo);
                    v.iter_mut().for_each(|x| *x = (*x - lo) * k);
                    ext.iter_mut().for_each(|x| *x = ((*x - lo) * k).clamp(0.0, 1.0));
                }
                // Directional contrast: darken a component relative to the
                // brightest neighbour reaching towards it -> crisp edges.
                for i in 0..6 {
                    let m = AFFECT[i].iter().fold(v[i], |m, &e| m.max(ext[e]));
                    if m > 1e-4 {
                        v[i] = m * (v[i] / m).powf(self.dir_exp);
                    }
                }
                // Global contrast within the cell.
                let m = v.iter().copied().fold(0f32, f32::max);
                if m > 1e-4 {
                    v.iter_mut().for_each(|x| *x = m * (*x / m).powf(self.glob_exp));
                }
                let mut key = 0u32;
                for x in v {
                    key = key << 5 | (x.clamp(0.0, 1.0) * 31.0 + 0.5) as u32;
                }
                let gi = match self.cache.get(&key) {
                    Some(&g) => g as usize,
                    None => {
                        let g = self.glyphs.nearest(&v);
                        self.cache.insert(key, g as u8);
                        g
                    }
                };
                let cell = if relative {
                    // Ink = bright side of the cell, ground = dark side,
                    // weighted by each circle's normalised level.
                    let (mut f, mut b, mut wf, mut wb) = ([0f32; 3], [0f32; 3], 0f32, 0f32);
                    for i in 0..6 {
                        let t = v[i].clamp(0.0, 1.0);
                        for d in 0..3 {
                            f[d] += col[i][d] * t;
                            b[d] += col[i][d] * (1.0 - t);
                        }
                        wf += t;
                        wb += 1.0 - t;
                    }
                    let f = if wf > 0.0 { [f[0] / wf, f[1] / wf, f[2] / wf] } else { avg };
                    let b = if wb > 0.0 { [b[0] / wb, b[1] / wb, b[2] / wb] } else { avg };
                    Cell { ch: self.glyphs.chars[gi], fg: to_u8(f), bg: to_u8(b) }
                } else {
                    // Glyphs cover a fraction of the cell, so lift the ink
                    // colour (hue-preserving) to keep perceived brightness.
                    let mx = avg[0].max(avg[1]).max(avg[2]).max(1.0);
                    let lift = 255.0 * (mx / 255.0).powf(0.55) / mx;
                    let fg = to_u8([avg[0] * lift, avg[1] * lift, avg[2] * lift]);
                    let bg = if self.ascii_bg { to_u8([avg[0] * 0.22, avg[1] * 0.22, avg[2] * 0.22]) } else { [0; 3] };
                    Cell { ch: self.glyphs.chars[gi], fg, bg }
                };
                out[cy * cols + cx] = cell;
            }
        }
    }
}

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
    let n = (w * h) as f32;
    [s[0] as f32 / n, s[1] as f32 / n, s[2] as f32 / n]
}

/// Split each cell into `sx` x `sy` sub-pixels and pick the two-colour
/// partition with the least squared error (exhaustive; at most 128 masks).
/// `glyph` maps a mask of foreground sub-pixels (row-major bits) to a char.
/// `dark_bg`: put the darker group in the background and dim it (braille,
/// whose small dots read poorly on a bright background).
fn mosaic(
    img: &Img,
    cols: usize,
    rows: usize,
    out: &mut [Cell],
    sx: usize,
    sy: usize,
    glyph: impl Fn(u32) -> char,
    dark_bg: bool,
) {
    let n = sx * sy;
    let (pw, ph) = (CW / sx, CH / sy);
    let mut c = [[0f32; 3]; 8];
    for cy in 0..rows {
        for cx in 0..cols {
            let mut tot = [0f32; 3];
            let mut sq = 0f32;
            for j in 0..sy {
                for i in 0..sx {
                    let k = j * sx + i;
                    c[k] = box_avg(img, cx * CW + i * pw, cy * CH + j * ph, pw, ph);
                    for d in 0..3 {
                        tot[d] += c[k][d];
                        sq += c[k][d] * c[k][d];
                    }
                }
            }
            // The last sub-pixel is pinned to the background group so each
            // partition is visited once; fg/bg swap covers the inverse.
            let mut best = (f32::MAX, 0u32, [0f32; 3], [0f32; 3]);
            for mask in 0..(1u32 << (n - 1)) {
                let mut a = [0f32; 3];
                let mut na = 0f32;
                for k in 0..n - 1 {
                    if mask & (1 << k) != 0 {
                        for d in 0..3 {
                            a[d] += c[k][d];
                        }
                        na += 1.0;
                    }
                }
                let nb = n as f32 - na;
                let b = [tot[0] - a[0], tot[1] - a[1], tot[2] - a[2]];
                let mut err = sq - (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]) / nb;
                if na > 0.0 {
                    err -= (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]) / na;
                }
                if err < best.0 - 1e-3 {
                    let ma = if na > 0.0 { [a[0] / na, a[1] / na, a[2] / na] } else { [0.0; 3] };
                    best = (err, mask, ma, [b[0] / nb, b[1] / nb, b[2] / nb]);
                }
            }
            let (_, mut mask, mut fg, mut bg) = best;
            if dark_bg {
                let l = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                if mask == 0 || (l(fg) - l(bg)).abs() < 14.0 {
                    // Flat cell: light every dot in the cell's mean colour.
                    mask = (1 << n) - 1;
                    fg = [tot[0] / n as f32, tot[1] / n as f32, tot[2] / n as f32];
                    bg = fg;
                } else if l(fg) < l(bg) {
                    mask = !mask & ((1 << n) - 1);
                    std::mem::swap(&mut fg, &mut bg);
                }
                bg = [bg[0] * 0.35, bg[1] * 0.35, bg[2] * 0.35];
            }
            out[cy * cols + cx] = Cell { ch: glyph(mask), fg: to_u8(fg), bg: to_u8(bg) };
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
