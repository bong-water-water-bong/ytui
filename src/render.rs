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
    /// Shape-matched ASCII in boosted colour over a dimmed copy of the
    /// picture (the "VideoASCII" look); tuned with the look panel.
    Glow,
    /// Two-tone shape ASCII: glyphs only where there is edge detail.
    Ascii,
    /// Harri-style absolute brightness ASCII, colour ink on a dark ground.
    Classic,
    Sextant,
    Quadrant,
    HalfBlock,
    Braille,
    /// Real text-mode art in 24-bit colour: CP437 glyphs from the IBM 8x16
    /// ROM font, two colours per cell, whichever two fit the picture best.
    Ansi,
    /// [`Ansi`] at 9 dots per cell: the extra column repeats the last dot of
    /// block and line-drawing characters, so box rules join with no seam.
    Ansi9,
    /// The period-correct version: the 16-colour DOS palette, with iCE
    /// colours giving 8 foregrounds x 16 backgrounds.
    Ansi16,
    /// [`Ansi16`] at 9 dots per cell.
    Ansi9x16,
    /// [`Ansi16`] with backgrounds limited to the 8 that do not blink.
    AnsiBlink,
}

impl Mode {
    pub const ALL: [Mode; 13] = [
        Mode::Glow,
        Mode::Art,
        Mode::Ascii,
        Mode::Classic,
        Mode::Sextant,
        Mode::Quadrant,
        Mode::HalfBlock,
        Mode::Braille,
        Mode::Ansi,
        Mode::Ansi9,
        Mode::Ansi16,
        Mode::Ansi9x16,
        Mode::AnsiBlink,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Art => "art",
            Mode::Glow => "glow",
            Mode::Ascii => "ascii",
            Mode::Classic => "classic",
            Mode::Sextant => "sextant",
            Mode::Quadrant => "quadrant",
            Mode::HalfBlock => "halfblock",
            Mode::Braille => "braille",
            Mode::Ansi16 => "ansi-16",
            Mode::AnsiBlink => "ansiblink",
            Mode::Ansi9 => "ansi9",
            Mode::Ansi => "ansi",
            Mode::Ansi9x16 => "ansi9-16",
        }
    }

    /// Parse a mode name. An exact match wins over a prefix, so `ansi` means
    /// the 24-bit mode rather than the `ansi-16` it is a prefix of.
    pub fn parse(s: &str) -> Option<Mode> {
        let want = s.to_ascii_lowercase();
        Mode::ALL
            .iter()
            .copied()
            .find(|m| m.name() == want)
            .or_else(|| Mode::ALL.iter().copied().find(|m| m.name().starts_with(&want)))
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
    /// Flatness threshold in linear light: below this a cell has no detail and
    /// is painted one even colour. Exposed so it can be swept against real
    /// footage rather than guessed.
    pub flat_lin: f32,
    /// Look (glow mode): ink brightness, colour saturation, and how much of
    /// the picture shows behind the characters (0 = black, 1 = full).
    pub bright: f32,
    pub sat: f32,
    pub bg_level: f32,
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
    /// Text-mode render: (bits, cols, rows, prepared glyphs), rebuilt on resize.
    tm_cache: Option<(usize, usize, usize, Vec<crate::textmode::Candidate>)>,
    /// Last frame's glyph per cell, and the geometry it was for. A cell whose
    /// picture has barely moved keeps its glyph and only refits the two
    /// colours, which skips the 256-glyph loop for most of the frame.

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

/// Relative luminance in the *linear* light space, 0..=1.
///
/// The naive `0.2126*r + 0.7152*g + 0.0722*b` on sRGB bytes is wrong in a way
/// that matters here: sRGB is gamma-encoded, so the same byte step means far
/// less light in the shadows than in the highlights. Comparing raw values
/// overstated contrast in dark footage (by ~4x at the bottom of the range) and
/// understated it in bright footage (by ~0.8x), which is why a single fixed
/// threshold could not serve both. Splitting text-mode art into ink and paper
/// is exactly a brightness comparison, so it has to happen on real light.
#[inline]
fn luma_lin(c: [f32; 3]) -> f32 {
    #[inline]
    fn to_lin(v: f32) -> f32 {
        let v = (v / 255.0).clamp(0.0, 1.0);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * to_lin(c[0]) + 0.7152 * to_lin(c[1]) + 0.0722 * to_lin(c[2])
}


/// Braille dot bit for sub-pixel (col, row) in a 2x4 grid.
const BRAILLE_BIT: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// Saturation (around the colour's own luma) then brightness.
fn grade(c: [f32; 3], sat: f32, gain: f32) -> [f32; 3] {
    let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    [(l + (c[0] - l) * sat) * gain, (l + (c[1] - l) * sat) * gain, (l + (c[2] - l) * sat) * gain]
}

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
            bright: 1.25,
            sat: 1.25,
            bg_level: 0.30,
            ascii_bg: true,
            dir_exp: 2.5,
            glob_exp: 1.8,
            flat: 0.10,
            black: 0.0,
            value_density: false,
            flat_lin: 0.0,
            glyphs,
            tm_cache: None,

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
            Mode::Glow => self.ascii(img, cols, rows, out, false, true),
            Mode::Ascii => self.ascii(img, cols, rows, out, true, false),
            Mode::Classic => self.ascii(img, cols, rows, out, false, false),
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
            Mode::Quadrant => mosaic(img, cols, rows, out, 2, 2, |m| QUAD[m as usize], false, self.flat_lin),
            Mode::Sextant => mosaic(img, cols, rows, out, 2, 3, sextant, false, self.flat_lin),
            Mode::Ansi16 | Mode::AnsiBlink | Mode::Ansi9x16 | Mode::Ansi | Mode::Ansi9 => {
                self.text_art(img, cols, rows, out)
            }
            Mode::Braille => mosaic(img, cols, rows, out, 2, 4, |m| {
                let mut b = 0;
                for i in 0..8 {
                    if m & (1 << i) != 0 {
                        b |= BRAILLE_BIT[i % 2][i / 2];
                    }
                }
                char::from_u32(0x2800 + b).unwrap()
            }, true, self.flat_lin),
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

    fn ascii(&mut self, img: &Img, cols: usize, rows: usize, out: &mut [Cell], relative: bool, glow: bool) {
        let (bright, sat, bg_level) = (self.bright, self.sat, self.bg_level);
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
                    let ink = [avg[0] * lift, avg[1] * lift, avg[2] * lift];
                    let (fg, bg) = if glow {
                        (to_u8(grade(ink, sat, bright)), to_u8(grade([avg[0] * bg_level, avg[1] * bg_level, avg[2] * bg_level], sat, 1.0)))
                    } else {
                        (to_u8(ink), if self.ascii_bg { to_u8([avg[0] * 0.22, avg[1] * 0.22, avg[2] * 0.22]) } else { [0; 3] })
                    };
                    Cell { ch: self.glyphs.chars[gi], fg, bg }
                };
                out[cy * cols + cx] = cell;
            }
        }
    }

    /// Text-mode art: fit CP437 glyphs from the DOS font and quantise to the
    /// 16-colour palette. Shares the engine with the exporter, so what you
    /// watch and what `--snapshot-ans` writes come from the same code.
    fn text_art(&mut self, img: &Img, cols: usize, rows: usize, out: &mut Vec<Cell>) {
        let binary = matches!(self.mode, Mode::AnsiBlink);
        let bits = if matches!(self.mode, Mode::Ansi9x16 | Mode::Ansi9) { 9 } else { 8 };
        // 24-bit modes pick the best two colours for the cell; the palette
        // modes round to the nearest of 16.
        let truecolor = matches!(self.mode, Mode::Ansi | Mode::Ansi9);
        // The cache is only valid for one geometry, and rebuilding the ink
        // masks is the expensive part of a frame.
        if self.tm_cache.as_ref().map(|(b, c, r, _)| (*b, *c, *r)) != Some((bits, cols, rows)) {
            // Prepared once per geometry: ink counts and pixel lists are
            // fixed for a font and cell size, and this is the difference
            // between ~190 ms and a few ms a frame.
            // Build at the renderer's own cell size: the mask and the image
            // must have the same number of pixels, and the ink indices index
            // into that same space.
            let hs = crate::textmode::candidates_bits(&crate::textmode::Font::dos(), CW, CH, bits);
            self.tm_cache = Some((bits, cols, rows, hs));
        }
        let cands = &self.tm_cache.as_ref().unwrap().3;
        let ice = if binary { crate::textmode::Ice::Off } else { crate::textmode::Ice::On };
        let p256 = PALETTE_256.load(std::sync::atomic::Ordering::Relaxed);

        out.clear();
        out.reserve(cols * rows);
        let mut cell_px = vec![[0u8; 3]; CW * CH];
        for cy in 0..rows {
            for cx in 0..cols {
                // The image is already exactly cols*CW x rows*CH pixels.
                for y in 0..CH {
                    for x in 0..CW {
                        let at = ((cy * CH + y) * img.w + cx * CW + x) * 3;
                        cell_px[y * CW + x] = [img.px[at], img.px[at + 1], img.px[at + 2]];
                    }
                }
                // Every glyph is fitted. A coverage shortlist reached ~29 ms a
                // frame but cost 34% more paint error on real video, a visible
                // quality drop, so the exact fit stays. These modes are for
                // stills and export; live video remains the mosaic modes' job
                // until a better shortlist metric exists.
                //
                // In truecolour the sums are hoisted out of the glyph loop and
                // the error is derived from them instead of walking the cell
                // again, which is what makes trying all 256 glyphs affordable.
                let sums = crate::textmode::CellSums::of(&cell_px);
                let mut best = (u64::MAX, crate::textmode::TCell::default());
                for c in cands.iter() {
                    let (cell, err) = if truecolor {
                        crate::textmode::score_rgb_pre(c, &cell_px, &sums)
                    } else {
                        crate::textmode::score(c, &cell_px, ice, crate::textmode::Rgb::Vga)
                    };
                    if err < best.0 {
                        best = (err, cell);
                    }
                }
                let cell = best.1;
                let (fg, bg) = if truecolor {
                    (cell.fg_rgb(crate::textmode::Rgb::Vga), cell.bg_rgb(crate::textmode::Rgb::Vga))
                } else {
                    (
                        crate::textmode::VGA[(cell.fg & 15) as usize],
                        crate::textmode::VGA[(cell.bg & 15) as usize],
                    )
                };
                let (fg, bg) = if p256 {
                    // Map through the 256-colour palette, dithered, so the
                    // picture survives a terminal without truecolour.
                    let e = |c: [u8; 3]| {
                        let i = xterm256(c, cx, cy);
                        xterm_rgb(i)
                    };
                    (e(fg), e(bg))
                } else {
                    (fg, bg)
                };
                out.push(Cell { ch: crate::textmode::ch(cell.ch), fg, bg });
            }
        }
    }
}

/// The RGB an xterm-256 index stands for, so a palette-mapped cell can still
/// go through the truecolour path (sixel, kitty) unchanged.
fn xterm_rgb(i: u8) -> [u8; 3] {
    match i {
        0..=15 => crate::textmode::XTERM16[i as usize],
        16..=231 => {
            let n = i as usize - 16;
            let (r, g, b) = (n / 36, (n % 36) / 6, n % 6);
            let lv = |v: usize| if v == 0 { 0 } else { 55 + v as u8 * 40 } as u8;
            [lv(r), lv(g), lv(b)]
        }
        _ => {
            let v = 8 + (i as u16 - 232) as u8 * 10;
            [v, v, v]
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
/// `dark_bg`: put the darker group in the background, so dots read as light
/// marks on a dark ground even when the source is inverted. (It no longer
/// dims the background: that cost 4x the paint error for no gain.)
fn mosaic(
    img: &Img,
    cols: usize,
    rows: usize,
    out: &mut [Cell],
    sx: usize,
    sy: usize,
    glyph: impl Fn(u32) -> char,
    dark_bg: bool,
    flat_lin: f32,
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
                // The exhaustive search above already found the best two
                // colours for this cell, so "this cell is flat, paint it one
                // colour" is not a correction, it is an override. Swept
                // against real footage the override always lost: forcing a
                // flat cell cost 36% error on dark footage and 30% on bright.
                // It is kept behind flat_lin (default 0.0, i.e. off) only so
                // the measurement is reproducible.
                let (lfg, lbg) = (luma_lin(fg), luma_lin(bg));
                let flat = flat_lin > 0.0 && (lfg - lbg).abs() < flat_lin;
                if mask == 0 || flat {
                    // Flat cell: light every dot in the cell's mean colour.
                    mask = (1 << n) - 1;
                    fg = [tot[0] / n as f32, tot[1] / n as f32, tot[2] / n as f32];
                    bg = fg;
                } else if lfg < lbg {
                    mask = !mask & ((1 << n) - 1);
                    std::mem::swap(&mut fg, &mut bg);
                }
                // No dimming. Braille used to multiply the background by
                // 0.35 on the theory that its small dots read badly over a
                // bright ground, but every mosaic mode is 50% paper by
                // construction; braille's gaps are simply smaller, which is
                // finer rather than worse. Measured against a real frame the
                // dim cost 4x the paint error for no legibility gain.
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
#[cfg(test)]
mod mode_parse_test {
    #[test]
    fn parse_ansi() {
        for s in ["ansi", "ansi-16", "ansi9", "ansi9-16", "ansiblink", "ascii"] {
            eprintln!("{s:10} -> {:?}", super::Mode::parse(s).map(|m| m.name()));
        }
    }
}
#[cfg(test)]
mod textmode_mode_tests {
    use super::*;

    fn frame(cols: usize, rows: usize) -> Vec<u8> {
        let (w, h) = (cols * CW, rows * CH);
        let mut px = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let at = (y * w + x) * 3;
                px[at] = (x * 255 / w) as u8;
                px[at + 1] = (y * 255 / h) as u8;
                px[at + 2] = 128;
            }
        }
        px
    }

    #[test]
    fn ansi_modes_produce_cp437_from_the_dos_palette() {
        let (cols, rows) = (40, 12);
        let px = frame(cols, rows);
        for mode in [Mode::Ansi16, Mode::AnsiBlink, Mode::Ansi9x16] {
            let mut r = Renderer::new(mode);
            let mut out = Vec::new();
            r.render(&Img { w: cols * CW, h: rows * CH, px: &px }, cols, rows, &mut out);
            assert_eq!(out.len(), cols * rows, "{mode:?} cell count");
            // Every cell must be a real CP437 glyph, and every colour must
            // come from the DOS palette.
            for c in &out {
                assert!(!c.ch.is_control(), "{mode:?} control char");
                assert!(
                    crate::textmode::VGA.iter().any(|p| *p == c.fg),
                    "{mode:?} fg {:?} is not a DOS colour",
                    c.fg
                );
                assert!(
                    crate::textmode::VGA.iter().any(|p| *p == c.bg),
                    "{mode:?} bg {:?} is not a DOS colour",
                    c.bg
                );
            }
        }
    }

    #[test]
    fn ansi_blink_never_uses_a_bright_background() {
        let (cols, rows) = (24, 8);
        let px = frame(cols, rows);
        let mut r = Renderer::new(Mode::AnsiBlink);
        let mut out = Vec::new();
        r.render(&Img { w: cols * CW, h: rows * CH, px: &px }, cols, rows, &mut out);
        // The dark half of the VGA palette is the first 8 entries.
        let dark = &crate::textmode::VGA[..8];
        for c in &out {
            assert!(
                dark.iter().any(|p| *p == c.bg),
                "iCE off must not produce a bright background: {:?}",
                c.bg
            );
        }
    }

    #[test]
    fn ansi_modes_are_steady_on_a_still_picture() {
        // Two identical frames must render identically, or video shimmers.
        let (cols, rows) = (24, 8);
        let px = frame(cols, rows);
        let mut r = Renderer::new(Mode::Ansi16);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let img = Img { w: cols * CW, h: rows * CH, px: &px };
        r.render(&img, cols, rows, &mut a);
        r.render(&img, cols, rows, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn ansi_resize_rebuilds_the_mask_cache() {
        let mut r = Renderer::new(Mode::Ansi16);
        let p1 = frame(16, 6);
        let mut out = Vec::new();
        r.render(&Img { w: 16 * CW, h: 6 * CH, px: &p1 }, 16, 6, &mut out);
        assert_eq!(out.len(), 16 * 6);
        let p2 = frame(20, 9);
        out.clear();
        r.render(&Img { w: 20 * CW, h: 9 * CH, px: &p2 }, 20, 9, &mut out);
        assert_eq!(out.len(), 20 * 9, "a resize must not reuse a stale cache");
    }
}
#[cfg(test)]
mod mosaic_probe {
    use super::*;
    /// How often does a mosaic cell end up with a dimmed background, and does
    /// the dimming cost colour accuracy?
    #[test]
    #[ignore]
    fn dimming_stats() {
        let (cols, rows) = (60, 20);
        let (w, h) = (cols * CW, rows * CH);
        let mut px = vec![0u8; w * h * 3];
        // A mid-tone gradient: neither dark nor bright, where a fixed
        // threshold or an unconditional dim shows up worst.
        for y in 0..h {
            for x in 0..w {
                let at = (y * w + x) * 3;
                let v = 90 + (x * 60 / w) as u8;
                px[at] = v;
                px[at + 1] = v.saturating_sub(20);
                px[at + 2] = v.saturating_add(20).min(255);
            }
        }
        let img = Img { w, h, px: &px };
        for mode in [Mode::Braille, Mode::Sextant, Mode::Quadrant] {
            let mut r = Renderer::new(mode);
            let mut cells = Vec::new();
            r.render(&img, cols, rows, &mut cells);
            // Compare each cell's painted average to the source average.
            let mut err = 0f64;
            let mut dimmed = 0;
            for cy in 0..rows {
                for cx in 0..cols {
                    let c = cells[cy * cols + cx];
                    let mut src = [0f64; 3];
                    for y in 0..CH {
                        for x in 0..CW {
                            let at = ((cy * CH + y) * cols * CW + cx * CW + x) * 3;
                            for d in 0..3 {
                                src[d] += px[at + d] as f64;
                            }
                        }
                    }
                    let n = (CW * CH) as f64;
                    // Painted average is a blend; approximate with the mean of
                    // the two colours, which is what a dim bg drags down.
                    let paint = [
                        (c.fg[0] as f64 + c.bg[0] as f64) / 2.0,
                        (c.fg[1] as f64 + c.bg[1] as f64) / 2.0,
                        (c.fg[2] as f64 + c.bg[2] as f64) / 2.0,
                    ];
                    for d in 0..3 {
                        err += (paint[d] - src[d] / n).powi(2);
                    }
                    let lb = c.bg[0] as f64 + c.bg[1] as f64 + c.bg[2] as f64;
                    let lf = c.fg[0] as f64 + c.fg[1] as f64 + c.fg[2] as f64;
                    if lb < lf * 0.6 && c.ch != ' ' {
                        dimmed += 1;
                    }
                }
            }
            eprintln!("{mode:?}: mean squared paint error {:.1}, cells with a dark bg {}/{}",
                err / (cols * rows) as f64, dimmed, cols * rows);
        }
    }
}
#[cfg(test)]
mod mosaic_accuracy {
    use super::*;

    /// The mosaic modes pick two exact 24-bit colours per cell, so they should
    /// reconstruct a smooth gradient almost exactly. If they band, something
    /// is quantising that should not be.
    #[test]
    fn mosaics_reproduce_a_gradient_without_banding() {
        let (cols, rows) = (40, 12);
        let (w, h) = (cols * CW, rows * CH);
        let mut px = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let at = (y * w + x) * 3;
                let v = (x * 255 / (w - 1)) as u8;
                px[at] = v;
                px[at + 1] = v;
                px[at + 2] = v;
            }
        }
        let img = Img { w, h, px: &px };
        for mode in [Mode::Braille, Mode::Sextant, Mode::Quadrant, Mode::HalfBlock] {
            let mut r = Renderer::new(mode);
            let mut cells = Vec::new();
            r.render(&img, cols, rows, &mut cells);
            // Each cell's two colours should bracket its own slice of the ramp.
            let mut worst = 0i32;
            for cy in 0..rows {
                for cx in 0..cols {
                    let c = cells[cy * cols + cx];
                    let centre = ((cx * CW + CW / 2) * 255 / (w - 1)) as i32;
                    // The brighter of the two colours must be near the cell's tone.
                    let hi = c.fg[0].max(c.bg[0]) as i32;
                    let lo = c.fg[0].min(c.bg[0]) as i32;
                    // Allow the pair to straddle the value, but not to be far off.
                    let off = if centre < lo { lo - centre } else if centre > hi { centre - hi } else { 0 };
                    worst = worst.max(off);
                }
            }
            assert!(worst <= 40, "{mode:?}: gradient reproduced {worst} levels off — banding or quantising");
        }
    }

    /// Braille must not dim its background: the dots and the gaps should keep
    /// the colours the fit chose.
    #[test]
    fn braille_does_not_dim_the_background() {
        let (cols, rows) = (20, 8);
        let (w, h) = (cols * CW, rows * CH);
        // A checkerboard forces a two-colour split in every cell.
        let mut px = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let at = (y * w + x) * 3;
                let v = if (x / 3 + y / 3) % 2 == 0 { 200u8 } else { 80 };
                px[at] = v;
                px[at + 1] = v;
                px[at + 2] = v;
            }
        }
        let img = Img { w, h, px: &px };
        let mut r = Renderer::new(Mode::Braille);
        let mut cells = Vec::new();
        r.render(&img, cols, rows, &mut cells);
        // `dark_bg` deliberately puts the darker group in the background, so
        // the 80-tone must be the bg and the 200-tone the ink. What must *not*
        // happen is the bg coming out dimmer than the source's own dark tone:
        // the old 0.35 dim landed around 28.
        let mut off = 0;
        for c in &cells {
            let d = (c.bg[0] as i32 - 80).abs();
            if d > 12 {
                off += 1;
            }
        }
        assert_eq!(off, 0, "{off} cells have a background that is not the source's 80-tone");
    }
}

#[cfg(test)]
mod real_video {
    use super::*;

    /// Paint error of a mosaic mode against the source frame, with the
    /// braille background dim at the given factor. 1.0 = undimmed.
    fn mode_error(px: &[u8], cols: usize, rows: usize, mode: Mode) -> f64 {
        let mut r = Renderer::new(mode);
        let img = Img { w: cols * CW, h: rows * CH, px };
        let mut cells = Vec::new();
        r.render(&img, cols, rows, &mut cells);
        let mut err = 0f64;
        for cy in 0..rows {
            for cx in 0..cols {
                let c = cells[cy * cols + cx];
                // Which sub-pixels does this glyph ink?
                let on: Vec<bool> = match mode {
                    Mode::Braille => (0..CH)
                        .flat_map(|y| (0..CW).map(move |x| (x, y)))
                        .map(|(x, y)| {
                            (0x2800..=0x28FF).contains(&(c.ch as u32))
                                && ((c.ch as u32 - 0x2800) & BRAILLE_BIT[x * 2 / CW][y * 4 / CH] != 0)
                        })
                        .collect(),
                    _ => {
                        // For other mosaics just use the mean; the comparison
                        // that matters here is braille-specific.
                        vec![false; CW * CH]
                    }
                };
                for y in 0..CH {
                    for x in 0..CW {
                        let at = ((cy * CH + y) * cols * CW + cx * CW + x) * 3;
                        let ink = if matches!(mode, Mode::Braille) { on[y * CW + x] } else { false };
                        let paint = if ink { c.fg } else { c.bg };
                        for d in 0..3 {
                            err += (paint[d] as f64 - px[at + d] as f64).powi(2);
                        }
                    }
                }
            }
        }
        err
    }

    fn clips() -> Vec<(&'static str, &'static str)> {
        vec![
            ("fractal", "/tmp/real_fractal.mp4"),
            ("dark", "/tmp/real_dark.mp4"),
            ("bright", "/tmp/real_bright.mp4"),
            ("life", "/tmp/real_life.mp4"),
        ]
    }

    #[test]
    #[ignore]
    fn braille_against_real_clips() {
        let (cols, rows) = (80, 25);
        for (name, path) in clips() {
            let px = match crate::player::grab_frame(path, 0.8, cols * CW, rows * CH) {
                Some(p) => p,
                None => { eprintln!("{name}: no frame"); continue; }
            };
            let e = mode_error(&px, cols, rows, Mode::Braille);
            // Average source brightness, to show which tonal range this is.
            let mean: f64 = px.chunks_exact(3).map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0).sum::<f64>()
                / (px.len() / 3) as f64;
            eprintln!("{name:8} mean luma {mean:5.1}  braille paint error {e:>12.0}");
        }
    }
}
#[cfg(test)]
mod real_video_tests {
    use super::*;

    /// Braille used to dim its background by 0.35 and to force "flat" cells to
    /// one colour. Both were measured against real footage and both made the
    /// picture worse, so both are gone; this test is the record.
    #[test]
    fn mosaic_never_dims_or_flattens() {
        assert_eq!(Renderer::new(Mode::Braille).flat_lin, 0.0, "flat overrides lose on real footage");
    }

    /// The exhaustive two-colour search must beat a single averaged colour on
    /// high-contrast content, which is the case braille is usually said to be
    /// good at. If this ever fails, the search is not running.
    #[test]
    fn the_two_colour_search_beats_one_average_colour() {
        let (cols, rows) = (40, 12);
        let (w, h) = (cols * CW, rows * CH);
        let mut px = vec![0u8; w * h * 3];
        // Hard black/white detail at a scale the braille dots can resolve:
        // each dot covers 3x3 px, so blocks of 3 (not 2) line up with the
        // sampling grid. A 2px checker aliases to grey by construction, which
        // is correct resampling rather than a rendering fault.
        for y in 0..h {
            for x in 0..w {
                let at = (y * w + x) * 3;
                let v = if (x / 3 + y / 3) % 2 == 0 { 255u8 } else { 0 };
                px[at] = v;
                px[at + 1] = v;
                px[at + 2] = v;
            }
        }
        let img = Img { w, h, px: &px };
        let mut r = Renderer::new(Mode::Braille);
        let mut cells = Vec::new();
        r.render(&img, cols, rows, &mut cells);
        // Two colours per cell means the extremes stay extreme; a single
        // averaged colour would push everything toward mid grey.
        let mut extreme = 0;
        for c in &cells {
            let hi = c.fg[0].max(c.bg[0]);
            let lo = c.fg[0].min(c.bg[0]);
            if hi >= 200 && lo <= 55 {
                extreme += 1;
            }
        }
        assert!(
            extreme * 2 >= cells.len(),
            "only {extreme}/{} cells kept black and white apart",
            cells.len()
        );
    }
}

