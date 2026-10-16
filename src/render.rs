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
    /// Clean ASCII art: directional strokes on edges, a short density ramp
    /// elsewhere, coloured ink on black, stable from frame to frame.
    Art,
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
    pub const ALL: [Mode; 7] = [Mode::Art, Mode::Ascii, Mode::Classic, Mode::Sextant, Mode::Quadrant, Mode::HalfBlock, Mode::Braille];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Art => "art",
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
    /// Art mode: keep characters steady across frames (set for video).
    pub temporal: bool,
    /// Art mode detail glyphs: symbols only, since letters and digits
    /// scattered through texture read as noise.
    art_glyphs: Glyphs,
    art_cache: HashMap<u32, u8>,
    art_prev: Vec<u8>,
    art_dims: (usize, usize),
    blur: Vec<f32>,
    gx: Vec<f32>,
    gy: Vec<f32>,
}

/// Art mode density ramp, darkest to brightest.
/// No directional characters here, so strokes always mean edges.
const RAMP: [char; 7] = [' ', '.', ':', '+', '*', '#', '@'];
/// Bends the ramp so the densest glyphs are kept for highlights; colour
/// carries the rest of the brightness.
const TONE_GAMMA: f32 = 1.4;
/// Art mode strokes by edge direction: bottom horizontal, mid horizontal,
/// vertical, rising diagonal, falling diagonal.
const STROKES: [char; 5] = ['_', '-', '|', '/', '\\'];
const EDGE: u8 = 100;
/// Art mode states from here up are shape-matched glyphs (index into `glyphs`).
const DETAIL: u8 = 128;
/// Luminance range inside a cell (0..1) needed for a shape-matched glyph.
const DETAIL_CONTRAST: f32 = 0.35;

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
        let art_glyphs = Glyphs {
            chars: glyphs.chars.iter().copied().filter(|c| !c.is_ascii_alphanumeric()).collect(),
            vecs: glyphs.chars.iter().zip(&glyphs.vecs).filter(|(c, _)| !c.is_ascii_alphanumeric()).map(|(_, v)| *v).collect(),
        };
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
            temporal: false,
            art_glyphs,
            art_cache: HashMap::new(),
            art_prev: Vec::new(),
            art_dims: (0, 0),
            blur: Vec::new(),
            gx: Vec::new(),
            gy: Vec::new(),
        }
    }

    pub fn render(&mut self, img: &Img, cols: usize, rows: usize, out: &mut Vec<Cell>) {
        out.clear();
        out.resize(cols * rows, Cell::default());
        if img.w < cols * CW || img.h < rows * CH || img.px.len() < img.w * img.h * 3 {
            return;
        }
        match self.mode {
            Mode::Art => self.art(img, cols, rows, out),
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

    /// Clean ASCII art. Edges (from Sobel gradients on a lightly blurred
    /// luminance image, summarised per cell with a structure tensor) become
    /// stroke characters along the edge direction; everything else maps to a
    /// 10-step density ramp. With `temporal` set, each cell keeps its
    /// previous character until the picture moves clearly past it, which
    /// removes frame-to-frame flicker.
    fn art(&mut self, img: &Img, cols: usize, rows: usize, out: &mut [Cell]) {
        let (w, h) = (img.w, img.h);
        let n = w * h;
        self.lum.resize(n, 0.0);
        for (i, p) in img.px[..n * 3].chunks_exact(3).enumerate() {
            self.lum[i] = (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0;
        }
        // 3x3 box blur: steadier gradients, less sensor / compression noise.
        self.blur.resize(n, 0.0);
        let at = |v: &[f32], x: isize, y: isize| v[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
        for y in 0..h as isize {
            for x in 0..w as isize {
                let mut s = 0.0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        s += at(&self.lum, x + dx, y + dy);
                    }
                }
                self.blur[y as usize * w + x as usize] = s / 9.0;
            }
        }
        self.gx.resize(n, 0.0);
        self.gy.resize(n, 0.0);
        for y in 0..h as isize {
            for x in 0..w as isize {
                let b = &self.blur;
                let gx = (at(b, x + 1, y - 1) + 2.0 * at(b, x + 1, y) + at(b, x + 1, y + 1)) - (at(b, x - 1, y - 1) + 2.0 * at(b, x - 1, y) + at(b, x - 1, y + 1));
                let gy = (at(b, x - 1, y + 1) + 2.0 * at(b, x, y + 1) + at(b, x + 1, y + 1)) - (at(b, x - 1, y - 1) + 2.0 * at(b, x, y - 1) + at(b, x + 1, y - 1));
                self.gx[y as usize * w + x as usize] = gx;
                self.gy[y as usize * w + x as usize] = gy;
            }
        }
        let keep = self.temporal && self.art_dims == (cols, rows) && self.art_prev.len() == cols * rows;
        if !keep {
            self.art_prev = vec![0; cols * rows];
            self.art_dims = (cols, rows);
        }
        // Sobel answers 4x a luminance step; this is a step of ~0.12.
        const T: f32 = 0.5;
        let area = (CW * CH) as f32;
        // Pass 1: per-cell structure tensor, edge coverage and luminance.
        struct Stat {
            jxx: f32,
            jyy: f32,
            jxy: f32,
            frac: f32,
            low: bool,
            lum: f32,
        }
        let mut stats = Vec::with_capacity(cols * rows);
        for cy in 0..rows {
            for cx in 0..cols {
                let (x0, y0) = (cx * CW, cy * CH);
                let (mut jxx, mut jyy, mut jxy) = (0f32, 0f32, 0f32);
                let (mut strong, mut wsum, mut ysum, mut lsum) = (0f32, 0f32, 0f32, 0f32);
                for py in 0..CH {
                    let row = (y0 + py) * w + x0;
                    for px in 0..CW {
                        let (gx, gy) = (self.gx[row + px], self.gy[row + px]);
                        jxx += gx * gx;
                        jyy += gy * gy;
                        jxy += gx * gy;
                        lsum += self.blur[row + px];
                        let m = (gx * gx + gy * gy).sqrt();
                        if m > T {
                            strong += 1.0;
                            wsum += m;
                            ysum += m * py as f32;
                        }
                    }
                }
                let low = wsum > 0.0 && ysum / wsum / CH as f32 > 0.62;
                stats.push(Stat { jxx, jyy, jxy, frac: strong / area, low, lum: lsum / area });
            }
        }
        // Tone is smoothed over neighbouring cells so flat areas get one
        // even texture instead of ragged bands where the ramp steps.
        let tone: Vec<f32> = (0..rows * cols)
            .map(|i| {
                let (cx, cy) = (i % cols, i / cols);
                let mut s = 4.0 * stats[i].lum;
                let mut n = 4.0;
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                    if x >= 0 && y >= 0 && (x as usize) < cols && (y as usize) < rows {
                        s += stats[y as usize * cols + x as usize].lum;
                        n += 1.0;
                    }
                }
                s / n
            })
            .collect();
        // Pass 2: pick each cell's character.
        for cy in 0..rows {
            for cx in 0..cols {
                let i = cy * cols + cx;
                let (x0, y0) = (cx * CW, cy * CH);
                let st = &stats[i];
                let (jxx, jyy, jxy, frac) = (st.jxx, st.jyy, st.jxy, st.frac);
                let coherence = ((jxx - jyy).powi(2) + 4.0 * jxy * jxy).sqrt() / (jxx + jyy + 1e-6);
                let prev = if keep { self.art_prev[i] } else { 0 };
                let was_edge = (EDGE..DETAIL).contains(&prev);
                let busy = frac > if was_edge || prev >= DETAIL { 0.10 } else { 0.18 };
                let state = if busy && coherence > 0.45 {
                    // Gradient angle (screen coords, y down); the edge runs across it.
                    let theta = 0.5 * (2.0 * jxy).atan2(jxx - jyy);
                    let phi = (theta + std::f32::consts::FRAC_PI_2).rem_euclid(std::f32::consts::PI);
                    let deg = phi.to_degrees();
                    let bin = if !(22.5..157.5).contains(&deg) {
                        // Horizontal: '_' when the edge sits low in the cell.
                        if st.low { 0 } else { 1 }
                    } else if (67.5..112.5).contains(&deg) {
                        2
                    } else if deg < 90.0 {
                        4 // runs right-and-down on screen
                    } else {
                        3
                    };
                    // Near a direction boundary, keep the previous stroke.
                    let near_boundary = [22.5f32, 67.5, 112.5, 157.5].iter().any(|b| (deg - b).abs() < 9.0);
                    if was_edge && near_boundary { prev } else { EDGE + bin }
                } else if busy && self.art_contrast(x0, y0, w, h) > DETAIL_CONTRAST {
                    // Strong detail with no single direction (lettering,
                    // corners): match the cell's light pattern to a shape.
                    let mut v = [0f32; 6];
                    for (k, pts) in self.inner.iter().enumerate() {
                        let mut s = 0.0;
                        for &(dx, dy) in pts {
                            let x = (x0 as i32 + dx).clamp(0, w as i32 - 1) as usize;
                            let y = (y0 as i32 + dy).clamp(0, h as i32 - 1) as usize;
                            s += self.blur[y * w + x];
                        }
                        v[k] = s / pts.len() as f32;
                    }
                    let lo = v.iter().copied().fold(1f32, f32::min);
                    let hi = v.iter().copied().fold(0f32, f32::max);
                    let k = 1.0 / (hi - lo).max(1e-3);
                    v.iter_mut().for_each(|x| *x = (*x - lo) * k);
                    let mut key = 0u32;
                    for x in v {
                        key = key << 5 | (x.clamp(0.0, 1.0) * 31.0 + 0.5) as u32;
                    }
                    let gi = *self.art_cache.entry(key).or_insert_with(|| self.art_glyphs.nearest(&v) as u8);
                    // A blank match means the detail is too faint to draw.
                    if self.art_glyphs.chars[gi as usize] == ' ' { 1 } else { DETAIL + gi }
                } else {
                    let x = tone[i].clamp(0.0, 1.0).powf(TONE_GAMMA) * RAMP.len() as f32;
                    let mut level = (x as usize).min(RAMP.len() - 1);
                    if (1..=RAMP.len() as u8).contains(&prev) {
                        let p = (prev - 1) as usize;
                        if (x - (p as f32 + 0.5)).abs() < 0.85 {
                            level = p;
                        }
                    }
                    level as u8 + 1
                };
                if keep || self.temporal {
                    self.art_prev[i] = state;
                }
                let ch = if state >= DETAIL {
                    self.art_glyphs.chars[(state - DETAIL) as usize]
                } else if state >= EDGE {
                    STROKES[(state - EDGE) as usize]
                } else {
                    RAMP[(state - 1) as usize]
                };
                let avg = box_avg(img, x0, y0, CW, CH);
                let mx = avg[0].max(avg[1]).max(avg[2]).max(1.0);
                // Ink on black: lift the colour, keeping its hue.
                let lift = 255.0 * (mx / 255.0).powf(0.5) / mx;
                out[i] = Cell { ch, fg: to_u8([avg[0] * lift, avg[1] * lift, avg[2] * lift]), bg: [0, 0, 0] };
            }
        }
    }

    /// Luminance range (max - min of the six sample circles) in a cell.
    fn art_contrast(&self, x0: usize, y0: usize, w: usize, h: usize) -> f32 {
        let (mut lo, mut hi) = (1f32, 0f32);
        for pts in &self.inner {
            let mut s = 0.0;
            for &(dx, dy) in pts {
                let x = (x0 as i32 + dx).clamp(0, w as i32 - 1) as usize;
                let y = (y0 as i32 + dy).clamp(0, h as i32 - 1) as usize;
                s += self.blur[y * w + x];
            }
            let m = s / pts.len() as f32;
            lo = lo.min(m);
            hi = hi.max(m);
        }
        hi - lo
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

/// Set when the terminal only shows the 256-colour xterm palette (e.g. tmux
/// without RGB): picture cells are then mapped to it by `xterm256`.
pub static PALETTE_256: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const CUBE: [i32; 6] = [0, 95, 135, 175, 215, 255];

/// xterm-256 palette entry for `c`, ordered-dithered on screen position so
/// gradients average out to the right colour instead of banding (tmux's own
/// conversion only rounds). Each channel picks between the two palette
/// levels around it, in proportion to where it sits between them.
/// Near-greys use the finer 24-step grey ramp.
pub fn xterm256(c: [u8; 3], x: usize, y: usize) -> u8 {
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    // Thresholds span 0.2..0.8: colours close to a palette level snap to it
    // (calm flat areas), the rest dither.
    let thr = 0.2 + 0.6 * (BAYER[y & 3][x & 3] as f32 + 0.5) / 16.0;
    // Index into `levels` after dithering `v` between its neighbours.
    let pick = |levels: &[i32], v: i32| -> usize {
        let k = levels.iter().rposition(|&l| l <= v).unwrap_or(0).min(levels.len() - 2);
        let f = (v - levels[k]) as f32 / (levels[k + 1] - levels[k]) as f32;
        if f > thr { k + 1 } else { k }
    };
    let v = [c[0] as i32, c[1] as i32, c[2] as i32];
    let (lo, hi) = (*v.iter().min().unwrap(), *v.iter().max().unwrap());
    if hi < 14 {
        // Dithering near-black against the darkest greys reads as faint boxes.
        return 16;
    }
    if hi - lo < 10 {
        // Black, the 24 greys (8, 18 .. 238), white.
        const GREY: [i32; 26] = [0, 8, 18, 28, 38, 48, 58, 68, 78, 88, 98, 108, 118, 128, 138, 148, 158, 168, 178, 188, 198, 208, 218, 228, 238, 255];
        return match pick(&GREY, (v[0] + v[1] + v[2]) / 3) {
            0 => 16,
            25 => 231,
            k => 231 + k as u8,
        };
    }
    let (r, g, b) = (pick(&CUBE, v[0]), pick(&CUBE, v[1]), pick(&CUBE, v[2]));
    16 + (36 * r + 6 * g + b) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth gradient with a bright disc, plus per-pixel noise.
    fn frame(cols: usize, rows: usize, seed: u32) -> Vec<u8> {
        let (w, h) = (cols * CW, rows * CH);
        let mut px = Vec::with_capacity(w * h * 3);
        let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
        for y in 0..h {
            for x in 0..w {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let noise = (s % 17) as f32 - 8.0;
                let (dx, dy) = (x as f32 - w as f32 / 2.0, (y as f32 - h as f32 / 2.0) * 0.5);
                let disc = if dx * dx + dy * dy < (w as f32 / 5.0).powi(2) { 120.0 } else { 0.0 };
                let v = (x as f32 / w as f32 * 120.0 + disc + noise).clamp(0.0, 255.0) as u8;
                px.extend_from_slice(&[v, v / 2, 255 - v]);
            }
        }
        px
    }

    fn changed(temporal: bool) -> usize {
        let (cols, rows) = (60, 20);
        let mut r = Renderer::new(Mode::Art);
        r.temporal = temporal;
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let (f1, f2) = (frame(cols, rows, 1), frame(cols, rows, 2));
        r.render(&Img { w: cols * CW, h: rows * CH, px: &f1 }, cols, rows, &mut a);
        r.render(&Img { w: cols * CW, h: rows * CH, px: &f2 }, cols, rows, &mut b);
        a.iter().zip(&b).filter(|(x, y)| x.ch != y.ch).count()
    }

    /// Inverse of the palette, for checking `xterm256`.
    fn palette(i: u8) -> [i32; 3] {
        if i >= 232 {
            let v = 8 + 10 * (i as i32 - 232);
            [v, v, v]
        } else {
            let i = i as usize - 16;
            [CUBE[i / 36], CUBE[i / 6 % 6], CUBE[i % 6]]
        }
    }

    #[test]
    fn palette_exact_colours_map_to_themselves() {
        for i in (16u8..=255).step_by(7) {
            let p = palette(i);
            let c = [p[0] as u8, p[1] as u8, p[2] as u8];
            for (x, y) in [(0, 0), (1, 2), (3, 3)] {
                assert_eq!(palette(xterm256(c, x, y)), p, "colour {i}");
            }
        }
    }

    #[test]
    fn dithered_palette_averages_to_the_colour() {
        for c in [[30u8, 90, 160], [200, 120, 60], [128, 128, 128], [250, 240, 200], [12, 40, 20], [100, 103, 98]] {
            let mut sum = [0i32; 3];
            for y in 0..4 {
                for x in 0..4 {
                    let p = palette(xterm256(c, x, y));
                    (0..3).for_each(|k| sum[k] += p[k]);
                }
            }
            for k in 0..3 {
                let avg = sum[k] / 16;
                assert!((avg - c[k] as i32).abs() <= 20, "{c:?} channel {k}: dithered average {avg}");
            }
        }
    }

    #[test]
    fn art_is_stable_across_noisy_frames() {
        let (loose, steady) = (changed(false), changed(true));
        eprintln!("cells whose character changed between two noisy frames: {loose} without hysteresis, {steady} with");
        assert!(steady * 3 <= loose.max(1), "hysteresis should remove most flicker");
    }
}
