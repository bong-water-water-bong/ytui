// Copyright (c) 2026 bong-water-water-bong
// SPDX-License-Identifier: MIT

//! Frame -> terminal cells: truecolour half blocks. Each cell is `▀` with
//! the top pixel as its foreground and the bottom pixel as its background,
//! so a frame is decoded at exactly `cols` x `2*rows` pixels.
//!
//! What limits the frame rate is how many bytes the terminal can parse, not
//! drawing: a full truecolour frame is ~40 bytes a cell (VTE reads about
//! 16 MB/s, so a big window at 60 fps can't be rewritten whole). ratatui
//! already sends only the cells that differ from the last frame, with
//! colour escapes only where the colour changes; this renderer makes that
//! diff small. A cell keeps the colours it is already showing until the
//! picture there moves further than a tolerance, and the tolerance is
//! chosen per frame as the smallest one whose changed cells fit the byte
//! budget the terminal can take. A still or slow picture is sent exactly;
//! fast motion gets coarser instead of dropping frames.

/// Pixels per cell: one wide, two high.
pub const CW: usize = 1;
pub const CH: usize = 2;

/// Estimated output bytes for one changed cell (two colour escapes and the
/// glyph, cursor moves amortised).
pub const CELL_BYTES: usize = 40;

/// Colour noise below this (per channel, 0-255) never counts as a change.
const MIN_TOL: u8 = 4;

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

pub struct Img<'a> {
    pub w: usize,
    pub h: usize,
    pub px: &'a [u8],
}

#[derive(Default)]
pub struct Renderer {
    /// Byte budget for the next frame's changed cells (None: no limit).
    pub budget: Option<usize>,
    /// Tolerance used for the last frame, for YTUI_STATS.
    pub tol: u8,
    /// Cells the last frame showed, and their grid size.
    shown: Vec<Cell>,
    dims: (usize, usize),
    fresh: Vec<Cell>,
    dist: Vec<u8>,
}

/// Largest per-channel difference between two colours.
#[inline]
fn diff(a: [u8; 3], b: [u8; 3]) -> u8 {
    a[0].abs_diff(b[0]).max(a[1].abs_diff(b[1])).max(a[2].abs_diff(b[2]))
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer::default()
    }

    /// Render `img` (`cols` x `2*rows` pixels) into `out`.
    pub fn render(&mut self, img: &Img, cols: usize, rows: usize, out: &mut Vec<Cell>) {
        Renderer::cells(img, cols, rows, &mut self.fresh);
        let n = cols * rows;
        if self.dims != (cols, rows) || self.shown.len() != n {
            self.dims = (cols, rows);
            self.shown.clone_from(&self.fresh);
            self.tol = 0;
            out.clone_from(&self.fresh);
            return;
        }
        let mut hist = [0usize; 256];
        self.dist.clear();
        for (a, b) in self.fresh.iter().zip(&self.shown) {
            let d = diff(a.fg, b.fg).max(diff(a.bg, b.bg));
            hist[d as usize] += 1;
            self.dist.push(d);
        }
        // Smallest tolerance whose changed cells (distance above it) fit.
        let cap = self.budget.map_or(usize::MAX, |b| b / CELL_BYTES);
        let mut tol = MIN_TOL;
        let mut changed: usize = hist[tol as usize + 1..].iter().sum();
        while changed > cap && tol < 255 {
            tol += 1;
            changed -= hist[tol as usize];
        }
        self.tol = tol;
        for i in 0..n {
            if self.dist[i] > tol {
                self.shown[i] = self.fresh[i];
            }
        }
        out.clone_from(&self.shown);
    }

    /// Plain half blocks with no history (thumbnails, snapshots).
    pub fn cells(img: &Img, cols: usize, rows: usize, out: &mut Vec<Cell>) {
        out.clear();
        out.resize(cols * rows, Cell::default());
        if img.w < cols * CW || img.h < rows * CH || img.px.len() < img.w * img.h * 3 {
            return;
        }
        let px = |x: usize, y: usize| {
            let i = (y * img.w + x) * 3;
            [img.px[i], img.px[i + 1], img.px[i + 2]]
        };
        for cy in 0..rows {
            for cx in 0..cols {
                out[cy * cols + cx] = Cell { ch: '▀', fg: px(cx, cy * 2), bg: px(cx, cy * 2 + 1) };
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

    fn frame(cols: usize, rows: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..rows * CH {
            for x in 0..cols * CW {
                v.extend_from_slice(&f(x, y));
            }
        }
        v
    }

    fn run(r: &mut Renderer, px: &[u8], cols: usize, rows: usize) -> Vec<Cell> {
        let mut out = Vec::new();
        r.render(&Img { w: cols * CW, h: rows * CH, px }, cols, rows, &mut out);
        out
    }

    #[test]
    fn half_blocks_take_top_and_bottom_pixels() {
        let px = frame(2, 1, |x, y| [x as u8 * 100, y as u8 * 200, 7]);
        let out = run(&mut Renderer::new(), &px, 2, 1);
        assert_eq!(out[0], Cell { ch: '▀', fg: [0, 0, 7], bg: [0, 200, 7] });
        assert_eq!(out[1], Cell { ch: '▀', fg: [100, 0, 7], bg: [100, 200, 7] });
    }

    #[test]
    fn noise_below_the_floor_is_not_a_change() {
        let mut r = Renderer::new();
        let a = run(&mut r, &frame(8, 4, |_, _| [100; 3]), 8, 4);
        let b = run(&mut r, &frame(8, 4, |x, _| [100 + (x % 3) as u8; 3]), 8, 4);
        assert_eq!(a, b);
        let c = run(&mut r, &frame(8, 4, |_, _| [180; 3]), 8, 4);
        assert!(c.iter().all(|c| c.fg == [180; 3]));
    }

    #[test]
    fn budget_sends_the_biggest_changes_first() {
        let mut r = Renderer::new();
        run(&mut r, &frame(10, 1, |_, _| [0; 3]), 10, 1);
        // Cell x changes by 20*x; room for three cells.
        r.budget = Some(3 * CELL_BYTES);
        let out = run(&mut r, &frame(10, 1, |x, _| [20 * x as u8; 3]), 10, 1);
        let sent: Vec<usize> = (0..10).filter(|&x| out[x].fg != [0; 3]).collect();
        assert_eq!(sent, vec![7, 8, 9]);
        // Next frame, unlimited: everything catches up exactly.
        r.budget = None;
        let out = run(&mut r, &frame(10, 1, |x, _| [20 * x as u8; 3]), 10, 1);
        assert!((0..10).all(|x| out[x].fg == [20 * x as u8; 3] || x == 0));
    }

    #[test]
    fn a_new_size_is_sent_whole() {
        let mut r = Renderer::new();
        r.budget = Some(0);
        run(&mut r, &frame(4, 2, |_, _| [0; 3]), 4, 2);
        let out = run(&mut r, &frame(5, 2, |_, _| [90; 3]), 5, 2);
        assert!(out.iter().all(|c| c.fg == [90; 3]));
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
