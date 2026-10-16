//! Pixel graphics backends: the kitty graphics protocol (drawn with Unicode
//! placeholders, so images live in ordinary text cells, scroll and clip with
//! the UI, and work inside tmux) and sixel (encoded with a fixed 252-colour
//! palette and ordered dithering, drawn after each frame's text).
//!
//! Callers describe image regions in terminal cells while drawing the
//! ratatui buffer; `finish` returns the escape bytes to write right after
//! the frame.

use crate::diacritics::DIACRITICS;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::collections::{HashMap, VecDeque};
use std::io::Write;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Proto {
    Kitty,
    Sixel,
}

impl Proto {
    pub fn name(self) -> &'static str {
        match self {
            Proto::Kitty => "kitty",
            Proto::Sixel => "sixel",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Caps {
    pub kitty: bool,
    pub sixel: bool,
    /// Terminal name from XTVERSION (or TERM_PROGRAM / TERM).
    pub name: String,
    /// Cell size in pixels, if the terminal told us.
    pub cell: Option<(u16, u16)>,
    pub tmux: bool,
    /// Inside tmux: kitty passthrough is enabled.
    pub passthrough: bool,
}

/// Terminal cell sizes outside this range are bogus reports.
fn plausible(c: (u16, u16)) -> bool {
    let (w, h) = (c.0 as f32, c.1 as f32);
    (5.0..=64.0).contains(&w) && (9.0..=128.0).contains(&h) && (1.3..=3.2).contains(&(h / w))
}

/// Height is far more reliable than width: some terminals (and tmux's
/// `client_cell_width`) report the advance gap between glyphs, which can be a
/// few pixels wide. If the reported size is not believable but the height is,
/// rebuild the width from the usual cell aspect (~2.2:1).
fn sane_cell(c: Option<(u16, u16)>) -> Option<(u16, u16)> {
    let (w, h) = c?;
    if plausible((w, h)) {
        return Some((w, h));
    }
    let est = ((h as f32 / 2.2).round() as u16).max(1);
    ((9.0..=128.0).contains(&(h as f32)) && plausible((est, h))).then_some((est, h))
}

/// Inside tmux, whether the attached terminal lacks truecolor as far as tmux
/// knows, so tmux squeezes colours into its 256-colour palette.
pub fn tmux_lacks_rgb() -> bool {
    std::env::var_os("TMUX").is_some()
        && tmux_query("#{client_termfeatures}").is_some_and(|f| !f.is_empty() && !f.split(',').any(|x| x == "RGB"))
}

/// Ask tmux about the client terminal it is drawing to.
fn tmux_query(fmt: &str) -> Option<String> {
    let out = std::process::Command::new("tmux").args(["display-message", "-p", fmt]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl Caps {
    /// Which protocol to use. Kitty placeholders are only assumed where
    /// they're known to work (kitty, Ghostty, or behind tmux); otherwise
    /// sixel is preferred when available.
    pub fn choose(&self) -> Option<Proto> {
        let n = self.name.to_lowercase();
        let kitty = self.kitty && (!self.tmux || self.passthrough);
        let placeholders = n.contains("kitty") || n.contains("ghostty") || self.tmux;
        // Sixel can't scale, so it needs a believable cell size (estimated
        // from the height when the terminal reports a bogus width).
        let sixel = self.sixel && sane_cell(self.cell).is_some();
        if kitty && placeholders {
            Some(Proto::Kitty)
        } else if sixel {
            Some(Proto::Sixel)
        } else if kitty {
            Some(Proto::Kitty)
        } else {
            None
        }
    }

    /// Why pixel graphics weren't chosen, for a hint in the UI.
    pub fn why_not(&self) -> Option<String> {
        if self.choose().is_some() {
            return None;
        }
        if self.sixel && sane_cell(self.cell).is_none() {
            let c = self.cell.map(|c| format!("{}x{}", c.0, c.1)).unwrap_or_else(|| "no".into());
            return Some(format!("sixel off: the terminal reported a {c} pixel cell size"));
        }
        if self.tmux && self.kitty && !self.passthrough {
            return Some("kitty graphics need `set -g allow-passthrough on` in tmux".into());
        }
        None
    }
}

fn tmux_wrap(seq: &[u8]) -> Vec<u8> {
    let mut o = b"\x1bPtmux;".to_vec();
    for &b in seq {
        if b == 0x1b {
            o.push(0x1b);
        }
        o.push(b);
    }
    o.extend_from_slice(b"\x1b\\");
    o
}

/// Ask the terminal what it supports. Must run before the UI takes over the
/// terminal; puts it in raw mode briefly.
pub fn detect() -> Caps {
    let mut caps = Caps { tmux: std::env::var("TMUX").is_ok(), ..Default::default() };
    caps.cell = crate::sys::cell_px();
    let mut q = Vec::new();
    let kitty_q = b"_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\\";
    if caps.tmux {
        q.extend(tmux_wrap(kitty_q));
    } else {
        q.extend_from_slice(kitty_q);
    }
    q.extend_from_slice(b"[>0q[16t[c");
    let got = crate::sys::query_tty(&q, |b| da1_end(b).is_some(), 700);
    let s = String::from_utf8_lossy(&got).to_string();
    if caps.tmux {
        // tmux answers DA1 itself; ask it what the real (outer) terminal can do.
        if let Some(f) = tmux_query("#{client_termfeatures}") {
            let feats: Vec<&str> = f.split(',').collect();
            caps.sixel = feats.contains(&"sixel");
        }
        if let (Some(w), Some(h)) = (tmux_query("#{client_cell_width}"), tmux_query("#{client_cell_height}")) {
            if let (Ok(w), Ok(h)) = (w.parse::<u16>(), h.parse::<u16>()) {
                caps.cell = Some((w, h));
            }
        }
        caps.passthrough = tmux_query("#{allow-passthrough}").or_else(|| {
            std::process::Command::new("tmux").args(["show", "-gv", "allow-passthrough"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        }).is_some_and(|v| v == "on" || v == "all");
    }
    caps.kitty = s.contains("\x1b_Gi=31;OK");
    if !caps.tmux {
        if let Some((a, b)) = da1_end(&got) {
            let params = &s[a..b];
            caps.sixel = params.split(';').any(|p| p == "4");
        }
    }
    if let Some(i) = s.find("\x1bP>|") {
        let rest = &s[i + 4..];
        caps.name = rest[..rest.find('\x1b').unwrap_or(rest.len())].to_string();
    }
    if caps.name.is_empty() {
        caps.name = std::env::var("TERM_PROGRAM").or_else(|_| std::env::var("TERM")).unwrap_or_default();
    }
    if caps.cell.is_none() {
        if let Some(i) = s.find("\x1b[6;") {
            let rest = &s[i + 4..];
            if let Some(end) = rest.find('t') {
                let v: Vec<u16> = rest[..end].split(';').filter_map(|x| x.parse().ok()).collect();
                if v.len() == 2 && v[0] > 0 && v[1] > 0 {
                    caps.cell = Some((v[1], v[0]));
                }
            }
        }
    }
    caps
}

/// Byte range of the DA1 parameters ("\e[?" .. "c"), if present.
fn da1_end(b: &[u8]) -> Option<(usize, usize)> {
    let s = b.windows(3).position(|w| w == b"\x1b[?")?;
    let start = s + 3;
    let end = b[start..].iter().position(|&c| c == b'c')? + start;
    b[start..end].iter().all(|c| c.is_ascii_digit() || *c == b';').then_some((start, end))
}

// ------------------------------------------------------------------ sixel

const LEVELS: (usize, usize, usize) = (6, 7, 6);
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// How a sixel image picks its colours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Palette {
    /// Fixed 6x7x6 colour cube with ordered dithering: works everywhere,
    /// grainy, and the noise defeats run-length compression.
    Fixed,
    /// The `colors` colours that best fit this picture (median cut over a
    /// 15-bit histogram), with an ordered dither of `dither` (0-16) to hide
    /// banding.
    Adaptive { colors: usize, dither: i32 },
}

/// 255 adaptive colours with a light dither: by eye (`--bench-render`
/// with YTUI_BENCH_PNG) fewer colours posterise skin and soft shading even
/// where PSNR looks fine. Video uses `Gfx::video_colors` (--sixel-colors).
pub const VIDEO_PALETTE: Palette = Palette::Adaptive { colors: 255, dither: 8 };
pub const STILL_PALETTE: Palette = VIDEO_PALETTE;

/// Encode RGB24 as a sixel image. `aspect`: pixel aspect ratio
/// (height : width); 2 draws each row twice as tall, so a moving picture
/// can send half the rows.
pub fn sixel_encode_with(rgb: &[u8], w: usize, h: usize, aspect: usize, pal: Palette, out: &mut Vec<u8>) {
    let (colors, idx) = quantize(rgb, w, h, pal);
    sixel_emit(&colors, &idx, w, h, aspect, out);
}

/// Map an image to palette indices: (palette, index per pixel).
pub fn quantize(rgb: &[u8], w: usize, h: usize, pal: Palette) -> (Vec<[u8; 3]>, Vec<u8>) {
    match pal {
        Palette::Fixed => quantize_fixed(rgb, w, h),
        Palette::Adaptive { colors, dither } => quantize_adaptive(rgb, w, h, colors.clamp(2, 255), dither),
    }
}

fn quantize_fixed(rgb: &[u8], w: usize, h: usize) -> (Vec<[u8; 3]>, Vec<u8>) {
    let (lr, lg, lb) = LEVELS;
    let colors = (0..lr * lg * lb)
        .map(|i| {
            let (r, g, b) = (i / (lg * lb), (i / lb) % lg, i % lb);
            [(r * 255 / (lr - 1)) as u8, (g * 255 / (lg - 1)) as u8, (b * 255 / (lb - 1)) as u8]
        })
        .collect();
    let q = |v: u8, levels: usize, t: f32| -> usize {
        let step = 255.0 / (levels - 1) as f32;
        (((v as f32) / step + t).floor() as isize).clamp(0, levels as isize - 1) as usize
    };
    let mut idx = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = &rgb[(y * w + x) * 3..];
            let t = (BAYER[y & 3][x & 3] as f32 + 0.5) / 16.0;
            idx[y * w + x] = (q(p[0], lr, t) * lg * lb + q(p[1], lg, t) * lb + q(p[2], lb, t)) as u8;
        }
    }
    (colors, idx)
}

/// 15-bit colour bin of a pixel (5 bits per channel).
#[inline]
fn bin(r: u8, g: u8, b: u8) -> usize {
    ((r as usize >> 3) << 10) | ((g as usize >> 3) << 5) | (b as usize >> 3)
}

fn quantize_adaptive(rgb: &[u8], w: usize, h: usize, max_colors: usize, dither: i32) -> (Vec<[u8; 3]>, Vec<u8>) {
    let mut m = PalMap::fit(rgb, max_colors, dither);
    let (idx, _) = m.map(rgb, w, h, 0);
    (m.colors, idx)
}

/// A palette and the 15-bit-bin lookup that maps pixels onto it. Kept with
/// a moving picture so later bands can be drawn in the colours already on
/// screen.
pub struct PalMap {
    pub colors: Vec<[u8; 3]>,
    lut: Vec<u8>,
    /// Bins `lut` has an answer for; any other bin is a colour this palette
    /// was not built from.
    filled: Vec<bool>,
    dither: i32,
}

impl PalMap {
    /// Median cut over `rgb` to at most `max_colors` colours.
    fn fit(rgb: &[u8], max_colors: usize, dither: i32) -> PalMap {
        // Histogram of 15-bit bins (count and channel sums for the mean).
        let mut count = vec![0u32; 1 << 15];
        let mut sum = vec![[0u32; 3]; 1 << 15];
        for p in rgb.chunks_exact(3) {
            let b = bin(p[0], p[1], p[2]);
            count[b] += 1;
            sum[b][0] += p[0] as u32;
            sum[b][1] += p[1] as u32;
            sum[b][2] += p[2] as u32;
        }
        let mut bins: Vec<u16> = (0..1u32 << 15).filter(|&b| count[b as usize] > 0).map(|b| b as u16).collect();
        // Median cut: split the box with the most pixels x widest channel range
        // along that channel at the pixel median, until there are 255 boxes.
        let ch = |b: u16, c: usize| -> u32 { ((b as u32) >> (10 - 5 * c as u32)) & 31 };
        struct Box {
            lo: usize,
            hi: usize,
            n: u64,
        }
        let n_of = |bins: &[u16]| bins.iter().map(|&b| count[b as usize] as u64).sum::<u64>();
        let mut boxes = vec![Box { lo: 0, hi: bins.len(), n: n_of(&bins) }];
        while boxes.len() < max_colors {
            // Split where the error is: count x variance of the widest channel
            // (so smooth but populous areas like skin get colours too, not just
            // the most common texture).
            let score = |bx: &Box, bins: &[u16]| -> (u64, usize) {
                let s = &bins[bx.lo..bx.hi];
                if s.len() < 2 {
                    return (0, 0);
                }
                let mut best = (0u64, 0usize);
                for c in 0..3 {
                    let (mut n, mut sv, mut sq) = (0u64, 0u64, 0u64);
                    for &b in s {
                        let k = count[b as usize] as u64;
                        let v = ch(b, c) as u64;
                        n += k;
                        sv += k * v;
                        sq += k * v * v;
                    }
                    // n * variance = sum(v^2) - (sum v)^2 / n
                    let sse = sq.saturating_sub(sv * sv / n.max(1));
                    if sse > best.0 {
                        best = (sse, c);
                    }
                }
                best
            };
            let Some((i, c)) = boxes
                .iter()
                .enumerate()
                .map(|(i, bx)| (i, score(bx, &bins)))
                .filter(|(_, (s, _))| *s > 0)
                .max_by_key(|(_, (s, _))| *s)
                .map(|(i, (_, c))| (i, c))
            else {
                break;
            };
            let (lo, hi) = (boxes[i].lo, boxes[i].hi);
            bins[lo..hi].sort_unstable_by_key(|&b| ch(b, c));
            let half = boxes[i].n / 2;
            let mut acc = 0u64;
            let mut mid = lo + 1;
            for (k, &b) in bins[lo..hi].iter().enumerate() {
                acc += count[b as usize] as u64;
                if acc >= half {
                    mid = (lo + k + 1).clamp(lo + 1, hi - 1);
                    break;
                }
            }
            let (a, b) = (n_of(&bins[lo..mid]), n_of(&bins[mid..hi]));
            boxes[i] = Box { lo, hi: mid, n: a };
            boxes.push(Box { lo: mid, hi, n: b });
        }
        // Palette = mean colour of each box; every bin maps to its box.
        let mut lut = vec![0u8; 1 << 15];
        let mut colors = Vec::with_capacity(boxes.len());
        for (k, bx) in boxes.iter().enumerate() {
            let mut s = [0u64; 3];
            for &b in &bins[bx.lo..bx.hi] {
                lut[b as usize] = k as u8;
                for c in 0..3 {
                    s[c] += sum[b as usize][c] as u64;
                }
            }
            let n = bx.n.max(1);
            colors.push([(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]);
        }
        let mut filled = vec![false; 1 << 15];
        for &b in &bins {
            filled[b as usize] = true;
        }
        PalMap { colors, lut, filled, dither }
    }

    /// Palette index per pixel for `rgb` (w x h), whose first row is row
    /// `y0` of the picture (keeps the dither pattern aligned). Also returns
    /// how many pixels fell in bins the palette had no answer for before
    /// this call (colours it was not built from).
    fn map(&mut self, rgb: &[u8], w: usize, h: usize, y0: usize) -> (Vec<u8>, usize) {
        let colors = &self.colors;
        // Bins no pixel used (reachable through dithering) take the nearest colour.
        let near = |r: u32, g: u32, b: u32| -> u8 {
            let mut best = (u32::MAX, 0);
            for (k, c) in colors.iter().enumerate() {
                let d = |a: u32, b: u8| (a as i32 - b as i32).pow(2) as u32;
                let e = d(r, c[0]) + d(g, c[1]) + d(b, c[2]);
                if e < best.0 {
                    best = (e, k);
                }
            }
            best.1 as u8
        };
        // Light ordered dither: +-half a 5-bit step before binning.
        let mut idx = vec![0u8; w * h];
        let (mut misses, mut new_bin) = (0, vec![false; 1 << 15]);
        for y in 0..h {
            for x in 0..w {
                let p = &rgb[(y * w + x) * 3..];
                // Bayer offset scaled to +-dither/2 levels.
                let t = (BAYER[(y0 + y) & 3][x & 3] as i32 * 2 - 15) * self.dither / 32;
                let d = |v: u8| (v as i32 + t).clamp(0, 255) as u8;
                let (r, g, b) = (d(p[0]), d(p[1]), d(p[2]));
                let k = bin(r, g, b);
                if !self.filled[k] {
                    self.lut[k] = near((r as u32 & !7) + 4, (g as u32 & !7) + 4, (b as u32 & !7) + 4);
                    self.filled[k] = true;
                    new_bin[k] = true;
                }
                misses += new_bin[k] as usize;
                idx[y * w + x] = self.lut[k];
            }
        }
        (idx, misses)
    }
}

/// Write palette + indices as a sixel DCS string.
pub fn sixel_emit(colors: &[[u8; 3]], idx: &[u8], w: usize, h: usize, aspect: usize, out: &mut Vec<u8>) {
    let _ = write!(out, "\x1bP0;1;0q\"{aspect};1;{w};{h}");
    let mut used_any = [false; 256];
    for &i in idx {
        used_any[i as usize] = true;
    }
    for (i, c) in colors.iter().enumerate() {
        if used_any[i] {
            let p = |v: u8| (v as u32 * 100 + 127) / 255;
            let _ = write!(out, "#{i};2;{};{};{}", p(c[0]), p(c[1]), p(c[2]));
        }
    }
    // Bands of 6 rows are independent: encode runs of them in parallel and
    // join them in order.
    let bands = h.div_ceil(6);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8).min(bands.max(1));
    let per = bands.div_ceil(threads.max(1)).max(1);
    let parts: Vec<Vec<u8>> = std::thread::scope(|sc| {
        let jobs: Vec<_> = (0..threads)
            .map(|t| {
                let (y0, y1) = ((t * per * 6).min(h), ((t + 1) * per * 6).min(h));
                sc.spawn(move || {
                    let mut o = Vec::new();
                    sixel_bands(idx, w, h, y0, y1, &mut o);
                    o
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap_or_default()).collect()
    });
    for p in parts {
        out.extend_from_slice(&p);
    }
    out.extend_from_slice(b"\x1b\\");
}

/// Sixel data for pixel rows y_start..y_end (y_start a multiple of 6).
fn sixel_bands(idx: &[u8], w: usize, h: usize, y_start: usize, y_end: usize, out: &mut Vec<u8>) {
    let mut slot = [usize::MAX; 256];
    let mut masks: Vec<Vec<u8>> = Vec::new();
    let mut used: Vec<u8> = Vec::new();
    for y0 in (y_start..y_end).step_by(6) {
        for &c in &used {
            slot[c as usize] = usize::MAX;
        }
        used.clear();
        masks.iter_mut().for_each(|m| m.iter_mut().for_each(|b| *b = 0));
        for k in 0..6.min(h - y0) {
            let row = &idx[(y0 + k) * w..(y0 + k + 1) * w];
            for (x, &c) in row.iter().enumerate() {
                let s = if slot[c as usize] == usize::MAX {
                    let s = used.len();
                    if masks.len() <= s {
                        masks.push(vec![0u8; w]);
                    }
                    slot[c as usize] = s;
                    used.push(c);
                    s
                } else {
                    slot[c as usize]
                };
                masks[s][x] |= 1 << k;
            }
        }
        for (si, &c) in used.iter().enumerate() {
            let m = &masks[si];
            // Skip leading blank columns with a run of empty sixels, and
            // stop at the last inked column.
            let last = m.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
            let _ = write!(out, "#{c}");
            let mut x = 0;
            while x < last {
                let v = m[x];
                let mut n = 1;
                while x + n < last && m[x + n] == v {
                    n += 1;
                }
                let ch = 63 + v;
                if n > 3 {
                    let _ = write!(out, "!{n}");
                    out.push(ch);
                } else {
                    out.extend(std::iter::repeat_n(ch, n));
                }
                x += n;
            }
            out.push(b'$');
        }
        out.push(b'-');
    }
}

/// Runs of cell rows (each `ch` pixel rows of `stride` bytes) whose pixels
/// differ between `a` and `b`, as [start, end) cell-row ranges. A row
/// changed when any pixel moved by more than `strong` in a channel (text, a
/// cursor), or more than `max_px` pixels moved by more than `tol` (all 0:
/// any difference counts).
fn changed_cell_rows(a: &[u8], b: &[u8], stride: usize, ch: usize, h: usize, (tol, max_px, strong): (u8, usize, u8)) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for r in 0..h.div_ceil(ch) {
        let (y0, y1) = (r * ch, ((r + 1) * ch).min(h));
        let (pa, pb) = (&a[y0 * stride..y1 * stride], &b[y0 * stride..y1 * stride]);
        let changed = pa != pb && {
            let (mut n, mut hit) = (0, tol == 0 && max_px == 0);
            for (p, q) in pa.chunks_exact(3).zip(pb.chunks_exact(3)) {
                if hit {
                    break;
                }
                let d = p[0].abs_diff(q[0]).max(p[1].abs_diff(q[1])).max(p[2].abs_diff(q[2]));
                n += (d > tol) as usize;
                hit = d > strong || n > max_px;
            }
            hit
        };
        if changed {
            match runs.last_mut() {
                Some(last) if last.1 == r => last.1 = r + 1,
                _ => runs.push((r, r + 1)),
            }
        }
    }
    runs
}

/// Default `Gfx::sixel_skip`. On real 720p clips at 960x270 px it halved a
/// talking head's bytes at 30 fps (-1 dB on screen) and cut a screen
/// recording's 63 -> 17 KB per frame (-0.25 dB), never leaving a pixel more
/// than 48 levels off.
const SIXEL_SKIP: (u8, usize, u8) = (12, 50, 48);

// ------------------------------------------------------------------ state

/// Identifies an image for caching / change detection.
pub struct Key<'a> {
    pub name: &'a str,
    /// Frame number for moving pictures (video, 3D); None for still images.
    pub seq: Option<u64>,
}

struct SixelPlace {
    rect: Rect,
    sig: u64,
    /// Identifies a moving picture's box (name, size, crop, aspect), so a
    /// frame can be diffed against the one already on screen there.
    gkey: u64,
    /// Sixel images to draw, each at a cell-row offset within `rect`.
    data: Option<Vec<(u16, Vec<u8>)>>,
    /// The pixels just sent for a moving picture, for the next frame's diff,
    /// and the palette they are drawn in.
    px: Option<(Vec<u8>, Option<PalMap>)>,
}

pub struct Gfx {
    pub proto: Proto,
    pub cell: (u16, u16),
    pub tmux: bool,
    /// Longest edge, in pixels, of images we send (bandwidth cap).
    pub max_px: usize,
    /// Most frames per second sent for a moving picture (0 = unlimited).
    pub max_fps: f32,
    /// Moving pictures: name -> (frame number last sent, when).
    sent: HashMap<String, (u64, std::time::Instant)>,
    /// Previous frame's pixels per moving picture, for band diffing.
    prev_px: HashMap<String, Vec<u8>>,
    next_id: u32,
    statics: HashMap<String, (u32, u16, u16)>,
    static_order: VecDeque<String>,
    dynamic: HashMap<String, (u32, u64, u16, u16)>,
    out: Vec<u8>,
    sixel_prev: Vec<(Rect, u64)>,
    sixel_now: Vec<SixelPlace>,
    /// Moving pictures on screen now: gkey -> (where, the pixels shown, the
    /// palette of their last whole frame).
    sixel_shown: HashMap<u64, (Rect, Vec<u8>, Option<PalMap>)>,
    /// Re-emit every sixel next frame (after a clear / resize).
    pub force: bool,
    /// Send moving pictures at half vertical resolution (2:1 pixels).
    pub sixel_tall: bool,
    /// Palette size for moving pictures (fewer = smaller, faster, blotchier).
    pub video_colors: usize,
    /// Sixel video: a cell row is resent only when one of its pixels moved
    /// by more than `sixel_skip.2` from what is on screen, or more than 1 in
    /// `sixel_skip.1` moved by more than `sixel_skip.0`, so compression noise
    /// (talking heads) is not resent. (0, 0, 0) resends any change.
    pub sixel_skip: (u8, usize, u8),
}

const SENTINEL: &str = "\u{2800}";

fn hash(parts: &[u64], name: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.bytes().chain(parts.iter().flat_map(|p| p.to_le_bytes())) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Gfx {
    pub fn new(proto: Proto, caps: &Caps) -> Gfx {
        let remote = std::env::var("SSH_CONNECTION").is_ok();
        Gfx {
            proto,
            cell: sane_cell(caps.cell).unwrap_or((10, 20)),
            tmux: caps.tmux,
            max_px: if remote { 960 } else { 1920 },
            sixel_tall: std::env::var_os("YTUI_SIXEL_FULL").is_none(),
            video_colors: 255,
            sixel_skip: if std::env::var_os("YTUI_SIXEL_EXACT").is_some() { (0, 0, 0) } else { SIXEL_SKIP },
            max_fps: if remote { 12.0 } else { 0.0 },
            sent: HashMap::new(),
            prev_px: HashMap::new(),
            next_id: 1,
            statics: HashMap::new(),
            static_order: VecDeque::new(),
            dynamic: HashMap::new(),
            out: Vec::new(),
            sixel_prev: Vec::new(),
            sixel_now: Vec::new(),
            sixel_shown: HashMap::new(),
            force: true,
        }
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = if self.next_id >= 0xFF_FFFF { 1 } else { self.next_id + 1 };
        id
    }

    fn apc(&mut self, seq: &[u8]) {
        if self.tmux {
            let w = tmux_wrap(seq);
            self.out.extend_from_slice(&w);
        } else {
            self.out.extend_from_slice(seq);
        }
    }

    /// Transmit `rgb` (w x h) as kitty image `id` with a virtual placement of
    /// cols x rows cells (zlib-compressed, chunked).
    fn kitty_send(&mut self, id: u32, rgb: &[u8], w: usize, h: usize, cols: u16, rows: u16) {
        self.kitty_send_at(id, rgb, w, h, cols, rows, 0, h)
    }

    /// Transmit `rgb` rows `y0..y1` into image `id`'s frame buffer at the
    /// matching offset. Kitty composes `a=f` updates onto the last transmitted
    /// frame, so a moving picture only pays for the rows that changed.
    #[allow(clippy::too_many_arguments)]
    fn kitty_send_at(&mut self, id: u32, rgb: &[u8], w: usize, h: usize, cols: u16, rows: u16, y0: usize, y1: usize) {
        let (y0, y1) = (y0.min(h), y1.min(h));
        if y1 <= y0 {
            return;
        }
        // A partial update addresses rows the image already knows about.
        let partial = y0 != 0 || y1 != h;
        let (bw, bh) = (w, y1 - y0);
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        let _ = z.write_all(&rgb[y0 * w * 3..y1 * w * 3]);
        let data = B64.encode(z.finish().unwrap_or_default());
        let bytes = data.as_bytes();
        let mut first = true;
        let mut i = 0;
        while i < bytes.len() || first {
            let end = (i + 4096).min(bytes.len());
            let more = (end < bytes.len()) as u8;
            let mut seq = Vec::with_capacity(end - i + 96);
            if first {
                if partial {
                    let _ = write!(seq, "\x1b_Ga=f,U=1,q=2,f=24,o=z,i={id},s={bw},v={bh},x=0,y={y0},m={more};");
                } else {
                    let _ = write!(seq, "\x1b_Ga=T,U=1,q=2,f=24,o=z,i={id},s={bw},v={bh},c={cols},r={rows},m={more};");
                }
            } else {
                let _ = write!(seq, "\x1b_Gm={more},q=2;");
            }
            seq.extend_from_slice(&bytes[i..end]);
            seq.extend_from_slice(b"\x1b\\");
            self.apc(&seq);
            first = false;
            i = end;
        }
    }

    fn kitty_delete(&mut self, id: u32) {
        let seq = format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
        self.apc(seq.as_bytes());
    }

    /// Remove every image (when switching back to text rendering / exiting).
    pub fn clear_all(&mut self) -> Vec<u8> {
        if self.proto == Proto::Kitty {
            self.apc(b"\x1b_Ga=d,d=A,q=2\x1b\\");
        }
        self.statics.clear();
        self.static_order.clear();
        self.dynamic.clear();
        self.sent.clear();
        self.prev_px.clear();
        self.sixel_prev.clear();
        self.force = true;
        std::mem::take(&mut self.out)
    }

    /// Pixel size to render a cols x rows region at (aspect kept, capped).
    pub fn px_for(&self, cols: u16, rows: u16) -> (usize, usize) {
        let (mut w, mut h) = (cols as usize * self.cell.0 as usize, rows as usize * self.cell.1 as usize);
        if self.proto == Proto::Kitty {
            let s = (self.max_px as f64 / w.max(h) as f64).min(1.0);
            w = ((w as f64 * s) as usize).max(1);
            h = ((h as f64 * s) as usize).max(1);
        }
        (w.max(1), h.max(1))
    }

    /// Draw an image into the cols x rows cell box whose top-left is screen
    /// column `x` and (possibly off-screen) row `sy` relative to `clip`.
    /// `make(w, h)` renders the pixels when they're actually needed.
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        buf: &mut Buffer,
        clip: Rect,
        x: u16,
        sy: i64,
        cols: u16,
        rows: u16,
        key: Key,
        make: &mut dyn FnMut(usize, usize) -> Option<Vec<u8>>,
    ) -> bool {
        if cols == 0 || rows == 0 || x >= clip.x + clip.width {
            return false;
        }
        let r0 = (-sy).max(0) as u16;
        let r1 = ((clip.height as i64 - sy).max(0) as u16).min(rows);
        let c1 = cols.min(clip.x + clip.width - x);
        if r1 <= r0 || c1 == 0 {
            return false;
        }
        let top = (clip.y as i64 + sy + r0 as i64) as u16;
        // Bandwidth: hold a moving picture's last frame until its slot is due.
        let key = match key.seq {
            Some(seq) => {
                let now = std::time::Instant::now();
                let due = self.max_fps <= 0.0
                    || self.sent.get(key.name).is_none_or(|(_, t)| now.duration_since(*t).as_secs_f32() >= 1.0 / self.max_fps);
                let seq = match self.sent.get(key.name) {
                    Some(&(last, _)) if !due => last,
                    _ => {
                        self.sent.insert(key.name.to_string(), (seq, now));
                        seq
                    }
                };
                Key { name: key.name, seq: Some(seq) }
            }
            None => key,
        };
        match self.proto {
            Proto::Kitty => {
                if cols as usize >= DIACRITICS.len() || rows as usize >= DIACRITICS.len() {
                    return false;
                }
                let id = match key.seq {
                    None => match self.statics.get(key.name) {
                        Some(&(id, c, r)) if (c, r) == (cols, rows) => id,
                        _ => {
                            let (pw, ph) = self.px_for(cols, rows);
                            let Some(px) = make(pw, ph) else { return false };
                            let id = self.alloc_id();
                            self.kitty_send(id, &px, pw, ph, cols, rows);
                            if let Some((old, _, _)) = self.statics.insert(key.name.to_string(), (id, cols, rows)) {
                                self.kitty_delete(old);
                            } else {
                                self.static_order.push_back(key.name.to_string());
                            }
                            while self.static_order.len() > 400 {
                                if let Some(k) = self.static_order.pop_front() {
                                    if let Some((old, _, _)) = self.statics.remove(&k) {
                                        self.kitty_delete(old);
                                    }
                                }
                            }
                            id
                        }
                    },
                    Some(seq) => {
                        let cur = self.dynamic.get(key.name).copied();
                        match cur {
                            Some((id, s, c, r)) if s == seq && (c, r) == (cols, rows) => id,
                            _ => {
                                let (pw, ph) = self.px_for(cols, rows);
                                let Some(px) = make(pw, ph) else { return false };
                                let id = cur.map(|c| c.0).unwrap_or_else(|| self.alloc_id());
                                // Same pixel size as the last frame? Then rows
                                // that did not change need not be resent.
                                let reusable = cur.is_some_and(|(_, _, c, r)| (c, r) == (cols, rows))
                                    && self.prev_px.get(key.name).is_some_and(|p| p.len() == px.len());
                                if reusable {
                                    let prev = self.prev_px.remove(key.name).unwrap_or_default();
                                    let stride = pw * 3;
                                    // Collect the changed row runs.
                                    let mut runs: Vec<(usize, usize)> = Vec::new();
                                    let (mut y, mut band, mut y0) = (0, false, 0);
                                    while y < ph {
                                        let a = y * stride;
                                        // Always test the row: `band || ..` would
                                        // keep the band open forever and turn
                                        // every frame into a full transmit.
                                        let diff = prev[a..a + stride] != px[a..a + stride];
                                        if diff && !band {
                                            band = true;
                                            y0 = y;
                                        } else if !diff && band {
                                            runs.push((y0, y));
                                            band = false;
                                        }
                                        y += 1;
                                    }
                                    if band {
                                        runs.push((y0, ph));
                                    }
                                    // Splitting a frame into bands loses the
                                    // cross-row redundancy zlib would have
                                    // exploited, so measure both and send
                                    // whichever is actually smaller.
                                    let banded: usize = runs
                                        .iter()
                                        .map(|&(a, b)| {
                                            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                                            let _ = z.write_all(&px[a * stride..b * stride]);
                                            B64.encode(z.finish().unwrap_or_default()).len()
                                        })
                                        .sum();
                                    let whole = {
                                        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                                        let _ = z.write_all(&px);
                                        B64.encode(z.finish().unwrap_or_default()).len()
                                    };
                                    // Only a genuine partial frame can beat a
                                    // full one; an identical frame sends nothing.
                                    if runs.is_empty() {
                                        // nothing changed
                                    } else if banded < whole {
                                        for &(a, b) in &runs {
                                            self.kitty_send_at(id, &px, pw, ph, cols, rows, a, b);
                                        }
                                    } else {
                                        self.kitty_send(id, &px, pw, ph, cols, rows);
                                    }
                                } else {
                                    self.kitty_send(id, &px, pw, ph, cols, rows);
                                }
                                self.prev_px.insert(key.name.to_string(), px);
                                self.dynamic.insert(key.name.to_string(), (id, seq, cols, rows));
                                id
                            }
                        }
                    }
                };
                let fg = Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8);
                let mut sym = String::with_capacity(12);
                for r in r0..r1 {
                    let y = (clip.y as i64 + sy + r as i64) as u16;
                    for c in 0..c1 {
                        sym.clear();
                        sym.push('\u{10EEEE}');
                        sym.push(DIACRITICS[r as usize]);
                        sym.push(DIACRITICS[c as usize]);
                        buf[(x + c, y)].set_symbol(&sym).set_fg(fg).set_bg(Color::Black);
                    }
                }
                true
            }
            Proto::Sixel => {
                let rect = Rect { x, y: top, width: c1, height: r1 - r0 };
                let sig = hash(&[key.seq.unwrap_or(0), r0 as u64, r1 as u64, c1 as u64, cols as u64, rows as u64], key.name);
                for yy in rect.y..rect.y + rect.height {
                    for xx in rect.x..rect.x + rect.width {
                        buf[(xx, yy)].set_symbol(SENTINEL).set_skip(true);
                    }
                }
                let (cw, ch) = (self.cell.0 as usize, self.cell.1 as usize);
                // Moving pictures: half the rows at 2:1 pixel aspect,
                // half the bytes for the terminal to parse.
                let aspect = if key.seq.is_some() && self.sixel_tall && ch % 2 == 0 { 2 } else { 1 };
                let ch = ch / aspect;
                let gkey = hash(&[r0 as u64, r1 as u64, c1 as u64, cols as u64, rows as u64, aspect as u64, cw as u64, ch as u64], key.name);
                let fresh = self.force || !self.sixel_prev.contains(&(rect, sig));
                let (data, shown) = if fresh {
                    let (pw, ph) = (cols as usize * cw, rows as usize * ch);
                    let Some(px) = make(pw, ph) else {
                        for yy in rect.y..rect.y + rect.height {
                            for xx in rect.x..rect.x + rect.width {
                                buf[(xx, yy)].set_symbol(" ").set_skip(false);
                            }
                        }
                        return false;
                    };
                    // Crop to the visible part of the box.
                    let (vx, vy0, vy1) = (c1 as usize * cw, r0 as usize * ch, r1 as usize * ch);
                    let mut crop = Vec::with_capacity(vx * (vy1 - vy0) * 3);
                    for yy in vy0..vy1.min(ph) {
                        crop.extend_from_slice(&px[(yy * pw) * 3..(yy * pw + vx.min(pw)) * 3]);
                    }
                    let (w, h) = (vx.min(pw), vy1.min(ph) - vy0);
                    let pal = if key.seq.is_some() { Palette::Adaptive { colors: self.video_colors, dither: 8 } } else { STILL_PALETTE };
                    // The same moving picture is already on screen in this
                    // spot: resend only the cell rows whose pixels changed.
                    // Rows are compared with what is on screen (not the last
                    // frame), so small changes cannot pile up unseen.
                    let (mut kept, mut on_screen) = (None, Vec::new());
                    let (tol, frac, strong) = self.sixel_skip;
                    let max_px = if frac == 0 { 0 } else { w * ch / frac };
                    let runs = match key.seq {
                        Some(_) if !self.force => match self.sixel_shown.get_mut(&gkey) {
                            Some((r, p, m)) if *r == rect && p.len() == crop.len() => {
                                kept = m.take();
                                on_screen = std::mem::take(p);
                                Some(changed_cell_rows(&on_screen, &crop, w * 3, ch, h, (tol, max_px, strong)))
                            }
                            _ => None,
                        },
                        _ => None,
                    };
                    let nrows = h.div_ceil(ch);
                    // Mostly changed: send it whole.
                    let runs = runs.filter(|runs| runs.iter().map(|(a, b)| b - a).sum::<usize>() * 5 < nrows * 4);
                    let mut parts = Vec::new();
                    let mut keep = None;
                    let mut whole = true;
                    let sent_runs = runs.clone().unwrap_or_default();
                    if let Some(runs) = runs {
                        let band = |a: usize, b: usize| (a * ch, (b * ch).min(h));
                        match (pal, kept) {
                            // Draw the bands in the palette of the last whole
                            // frame, so they match the rows left on screen.
                            (Palette::Adaptive { .. }, Some(mut m)) => {
                                let (mut maps, mut miss, mut n) = (Vec::new(), 0, 0);
                                for &(a, b) in &runs {
                                    let (y0, y1) = band(a, b);
                                    let (idx, k) = m.map(&crop[y0 * w * 3..y1 * w * 3], w, y1 - y0, y0);
                                    miss += k;
                                    n += idx.len();
                                    maps.push((a, y1 - y0, idx));
                                }
                                // Over 5% of the pixels in colours that palette
                                // was not built from: refit on a whole frame.
                                if miss * 20 <= n {
                                    for (a, bh, idx) in maps {
                                        let mut enc = Vec::new();
                                        sixel_emit(&m.colors, &idx, w, bh, aspect, &mut enc);
                                        parts.push((a as u16, enc));
                                    }
                                    keep = Some(m);
                                    whole = false;
                                }
                            }
                            // A fixed palette never changes: cut the bands
                            // from this frame.
                            _ => {
                                let (colors, idx) = quantize(&crop, w, h, pal);
                                for (a, b) in runs {
                                    let (y0, y1) = band(a, b);
                                    let mut enc = Vec::new();
                                    sixel_emit(&colors, &idx[y0 * w..y1 * w], w, y1 - y0, aspect, &mut enc);
                                    parts.push((a as u16, enc));
                                }
                                whole = false;
                            }
                        }
                    }
                    if whole {
                        let mut enc = Vec::new();
                        match pal {
                            Palette::Adaptive { colors, dither } if key.seq.is_some() => {
                                let mut m = PalMap::fit(&crop, colors.clamp(2, 255), dither);
                                let (idx, _) = m.map(&crop, w, h, 0);
                                sixel_emit(&m.colors, &idx, w, h, aspect, &mut enc);
                                keep = Some(m);
                            }
                            _ => sixel_encode_with(&crop, w, h, aspect, pal, &mut enc),
                        }
                        parts.push((0, enc));
                    }
                    // Remember what the screen now shows: the bands sent
                    // over the rows left in place, or the whole frame.
                    let shown = if whole {
                        crop
                    } else {
                        for (a, b) in sent_runs {
                            let (y0, y1) = (a * ch * w * 3, (b * ch).min(h) * w * 3);
                            on_screen[y0..y1].copy_from_slice(&crop[y0..y1]);
                        }
                        on_screen
                    };
                    (Some(parts), key.seq.map(|_| (shown, keep)))
                } else {
                    (None, None)
                };
                self.sixel_now.push(SixelPlace { rect, sig, gkey, data, px: shown });
                true
            }
        }
    }

    /// Call after everything is drawn into `buf`; returns the bytes to write
    /// after the frame. Sixel images whose cells were drawn over (dialogs,
    /// the miniplayer) are dropped so they don't paint over the overlay.
    pub fn finish(&mut self, buf: &mut Buffer) -> Vec<u8> {
        if self.proto == Proto::Sixel {
            let mut prev = Vec::new();
            let mut shown = HashMap::new();
            for p in std::mem::take(&mut self.sixel_now) {
                let r = p.rect;
                let mut covered = false;
                for yy in r.y..r.y + r.height {
                    for xx in r.x..r.x + r.width {
                        let cell = &mut buf[(xx, yy)];
                        if cell.symbol() != SENTINEL {
                            cell.set_skip(false);
                            covered = true;
                        }
                    }
                }
                if covered {
                    for yy in r.y..r.y + r.height {
                        for xx in r.x..r.x + r.width {
                            let cell = &mut buf[(xx, yy)];
                            if cell.symbol() == SENTINEL {
                                cell.set_symbol(" ").set_skip(false);
                            }
                        }
                    }
                    continue;
                }
                for (dy, d) in p.data.iter().flatten() {
                    let _ = write!(self.out, "\x1b[{};{}H", r.y + dy + 1, r.x + 1);
                    self.out.extend_from_slice(d);
                }
                // Pictures dropped above (covered) leave `shown`, so their
                // next frame is sent whole.
                match p.px {
                    Some((px, pal)) => {
                        shown.insert(p.gkey, (r, px, pal));
                    }
                    None => {
                        if let Some(s) = self.sixel_shown.remove(&p.gkey) {
                            shown.insert(p.gkey, s);
                        }
                    }
                }
                prev.push((r, p.sig));
            }
            self.sixel_prev = prev;
            self.sixel_shown = shown;
            self.force = false;
        }
        std::mem::take(&mut self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_is_one_cell_wide() {
        use unicode_width::UnicodeWidthStr;
        let s: String = ['\u{10EEEE}', DIACRITICS[5], DIACRITICS[9]].iter().collect();
        assert_eq!(s.width(), 1);
    }

    #[test]
    fn sixel_header_and_terminator() {
        let rgb = vec![200u8; 8 * 7 * 3];
        let mut out = Vec::new();
        sixel_encode_with(&rgb, 8, 7, 1, Palette::Fixed, &mut out);
        assert!(out.starts_with(b"\x1bP0;1;0q\"1;1;8;7"));
        assert!(out.ends_with(b"\x1b\\"));
    }

    #[test]
    fn bogus_cell_width_is_rebuilt_from_height() {
        // tmux reports 3x12 on some boxes: the glyph gap, not a cell width.
        assert_eq!(sane_cell(Some((3, 12))), Some((5, 12)));
        assert_eq!(sane_cell(Some((9, 20))), Some((9, 20)));
        assert_eq!(sane_cell(Some((3, 2))), None);
        assert_eq!(sane_cell(None), None);
    }

    #[test]
    fn kitty_band_transmit_encodes_offsets() {
        let mut g = Gfx::new(Proto::Kitty, &Caps::default());
        let (w, h) = (4usize, 6usize);
        let rgb = vec![7u8; w * h * 3];
        g.out.clear();
        g.kitty_send_at(9, &rgb, w, h, 2, 3, 2, 5);
        let s = String::from_utf8_lossy(&g.out).to_string();
        assert!(s.contains("a=f"), "band update must use a=f: {s:?}");
        assert!(s.contains("y=2"), "band update must carry its row offset: {s:?}");
        assert!(s.contains("v=3"), "band height must be 3 rows: {s:?}");
        assert!(!s.contains("c=2"), "a=f must not resend placement fields: {s:?}");
        g.out.clear();
        g.kitty_send(9, &rgb, w, h, 2, 3);
        let s = String::from_utf8_lossy(&g.out).to_string();
        assert!(s.contains("a=T") && s.contains("c=2") && s.contains("r=3"), "full transmit keeps geometry: {s:?}");
    }

    #[test]
    fn empty_band_is_not_sent() {
        let mut g = Gfx::new(Proto::Kitty, &Caps::default());
        g.out.clear();
        g.kitty_send_at(1, &[0u8; 12], 2, 2, 1, 1, 2, 2);
        assert!(g.out.is_empty(), "a zero-height band must transmit nothing");
    }

    /// Consecutive frames the way video drives them: the first is a full
    /// transmit, later ones reuse the image and send only what moved.
    #[test]
    fn consecutive_frames_send_bands_not_whole_frames() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut g = Gfx::new(Proto::Kitty, &Caps::default());
        // Unlimited: the default is 12 fps over SSH, which would hold one frame
        // across these back-to-back calls.
        g.max_fps = 0.0;
        let (cols, rows) = (4u16, 3u16);
        let mut buf = Buffer::empty(Rect { x: 0, y: 0, width: cols, height: rows });
        let clip = Rect { x: 0, y: 0, width: cols, height: rows };
        let (mut full, mut band) = (0, 0);
        for seq in 1..=4u64 {
            g.out.clear();
            assert!(g.place(&mut buf, clip, 0, 0, cols, rows, Key { name: "v", seq: Some(seq) }, &mut |w, h| {
                let mut p = vec![9u8; w * h * 3];
                for b in p.iter_mut().take(w * 3) {
                    *b = (seq * 20) as u8;
                }
                Some(p)
            }), "place must succeed for frame {seq}");
            let s = String::from_utf8_lossy(&g.out).to_string();
            if seq == 1 {
                assert!(s.contains("a=T"), "the first frame is a full transmit");
            }
            if seq > 1 {
                assert!(!s.is_empty(), "frame {seq} changed rows and must transmit");
            }
            full += s.contains("a=T") as i32;
            band += s.contains("a=f") as i32;
        }
        assert_eq!(full, 1, "only the first frame is a full transmit");
        assert_eq!(band, 3, "later frames reuse the image via band updates");
    }

    /// An identical frame must transmit nothing at all.
    #[test]
    fn unchanged_frame_sends_nothing() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut g = Gfx::new(Proto::Kitty, &Caps::default());
        g.max_fps = 0.0;
        let (cols, rows) = (4u16, 3u16);
        let mut buf = Buffer::empty(Rect { x: 0, y: 0, width: cols, height: rows });
        let clip = Rect { x: 0, y: 0, width: cols, height: rows };
        for seq in 1..=3u64 {
            g.out.clear();
            assert!(g.place(&mut buf, clip, 0, 0, cols, rows, Key { name: "v", seq: Some(seq) }, &mut |w, h| {
                Some(vec![5u8; w * h * 3])
            }));
            if seq > 1 {
                assert!(g.out.is_empty(), "frame {seq} is identical and must not be resent");
            }
        }
    }

    /// Banding wins big on a mostly-static scene (the video common case).
    #[test]
    fn banding_beats_full_frame_on_static_scene() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut g = Gfx::new(Proto::Kitty, &Caps::default());
        g.max_fps = 0.0;
        let (cols, rows) = (24u16, 16u16);
        let (pw, ph) = g.px_for(cols, rows);
        let mut buf = Buffer::empty(Rect { x: 0, y: 0, width: cols, height: rows });
        let clip = Rect { x: 0, y: 0, width: cols, height: rows };
        // Frame 1: full transmit. Frame 2: only the middle band moves.
        for seq in 1..=2u64 {
            g.out.clear();
            assert!(g.place(&mut buf, clip, 0, 0, cols, rows, Key { name: "v", seq: Some(seq) }, &mut |w, h| {
                let mut p: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
                if seq == 2 {
                    let (a, b) = (h / 3, 2 * h / 3);
                    for i in a * w * 3..b * w * 3 {
                        p[i] = p[i].wrapping_add(60);
                    }
                }
                Some(p)
            }));
            if seq == 2 {
                let s = String::from_utf8_lossy(&g.out).to_string();
                assert!(s.contains("a=f"), "a partial change should use a band update");
            }
        }
        let _ = (pw, ph);
    }

    /// A changed box size must fall back to a full transmit, not a band update.
    #[test]
    fn resize_forces_full_transmit() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut g = Gfx::new(Proto::Kitty, &Caps::default());
        g.max_fps = 0.0;
        let mut seq = 0u64;
        let run = |cols: u16, rows: u16, g: &mut Gfx, seq: &mut u64| {
            *seq += 1;
            let s = *seq;
            let mut buf = Buffer::empty(Rect { x: 0, y: 0, width: cols, height: rows });
            let clip = Rect { x: 0, y: 0, width: cols, height: rows };
            g.out.clear();
            assert!(g.place(&mut buf, clip, 0, 0, cols, rows, Key { name: "v", seq: Some(s) }, &mut |w, h| {
                Some(vec![(s * 7) as u8; w * h * 3])
            }));
            String::from_utf8_lossy(&g.out).to_string()
        };
        let _ = run(4, 3, &mut g, &mut seq);
        let s = run(6, 4, &mut g, &mut seq);
        assert!(s.contains("a=T"), "a size change must send a full frame: {s:?}");
    }

    /// The fps throttle holds a moving picture's frame instead of sending
    /// back-to-back updates, which is what keeps SSH bandwidth in check.
    #[test]
    fn throttle_holds_frames_that_are_not_due() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut g = Gfx::new(Proto::Kitty, &Caps::default());
        g.max_fps = 12.0;
        let (cols, rows) = (4u16, 3u16);
        let mut buf = Buffer::empty(Rect { x: 0, y: 0, width: cols, height: rows });
        let clip = Rect { x: 0, y: 0, width: cols, height: rows };
        let mut drawn = 0;
        for seq in 1..=5u64 {
            g.out.clear();
            assert!(g.place(&mut buf, clip, 0, 0, cols, rows, Key { name: "v", seq: Some(seq) }, &mut |w, h| {
                drawn += 1;
                Some(vec![(seq * 20) as u8; w * h * 3])
            }));
        }
        assert_eq!(drawn, 1, "only the frame that was due should render pixels");
    }

    /// One sixel video frame through place() + finish(); returns the bytes.
    fn sixel_frame(g: &mut Gfx, seq: u64, cols: u16, rows: u16, px: &mut dyn FnMut(usize, usize) -> Vec<u8>) -> String {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let area = Rect { x: 0, y: 0, width: cols, height: rows + 2 };
        let mut buf = Buffer::empty(area);
        assert!(g.place(&mut buf, area, 0, 1, cols, rows, Key { name: "v", seq: Some(seq) }, &mut |w, h| Some(px(w, h))));
        String::from_utf8_lossy(&g.finish(&mut buf)).to_string()
    }

    fn sixel_gfx() -> Gfx {
        let mut g = Gfx::new(Proto::Sixel, &Caps::default());
        g.max_fps = 0.0;
        g
    }

    #[test]
    fn sixel_unchanged_frame_sends_nothing() {
        let mut g = sixel_gfx();
        let first = sixel_frame(&mut g, 1, 6, 4, &mut |w, h| vec![90u8; w * h * 3]);
        assert!(first.contains("\x1bP"), "the first frame is sent whole");
        let second = sixel_frame(&mut g, 2, 6, 4, &mut |w, h| vec![90u8; w * h * 3]);
        assert!(!second.contains("\x1bP"), "an identical frame must not be resent: {second:?}");
    }

    #[test]
    fn sixel_sends_only_the_changed_cell_row() {
        let mut g = sixel_gfx();
        let (cols, rows) = (6u16, 4u16);
        // Both colours are in the first frame, so its palette covers them.
        let mut frame = |seq: u64, w: usize, h: usize| {
            let mut p = vec![90u8; w * h * 3];
            let ch = h / rows as usize;
            p[..ch * w * 3].fill(200);
            if seq == 2 {
                // Change pixels inside the third cell row only.
                p[(2 * ch + 1) * w * 3..(2 * ch + 2) * w * 3].fill(200);
            }
            p
        };
        let _ = sixel_frame(&mut g, 1, cols, rows, &mut |w, h| frame(1, w, h));
        let s = sixel_frame(&mut g, 2, cols, rows, &mut |w, h| frame(2, w, h));
        assert_eq!(s.matches("\x1bP").count(), 1, "one band: {s:?}");
        // The picture's top is screen row 2 (1-based), so its third cell row is 4.
        assert!(s.contains("\x1b[4;1H\x1bP"), "band placed at its cell row: {s:?}");
    }

    #[test]
    fn sixel_mostly_changed_frame_is_sent_whole() {
        let mut g = sixel_gfx();
        let _ = sixel_frame(&mut g, 1, 6, 4, &mut |w, h| vec![90u8; w * h * 3]);
        let s = sixel_frame(&mut g, 2, 6, 4, &mut |w, h| vec![120u8; w * h * 3]);
        assert_eq!(s.matches("\x1bP").count(), 1);
        assert!(s.contains("\x1b[2;1H\x1bP"), "whole frame from the top: {s:?}");
    }

    #[test]
    fn sixel_after_force_sends_whole_frame() {
        let mut g = sixel_gfx();
        let _ = sixel_frame(&mut g, 1, 6, 4, &mut |w, h| vec![90u8; w * h * 3]);
        g.force = true;
        let s = sixel_frame(&mut g, 2, 6, 4, &mut |w, h| vec![90u8; w * h * 3]);
        assert!(s.contains("\x1b[2;1H\x1bP"), "a cleared screen needs the whole picture: {s:?}");
    }

    #[test]
    fn sixel_covered_picture_is_sent_whole_next_time() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut g = sixel_gfx();
        let _ = sixel_frame(&mut g, 1, 6, 4, &mut |w, h| vec![90u8; w * h * 3]);
        // Frame 2 is drawn over (a dialog), so it is dropped.
        let area = Rect { x: 0, y: 0, width: 6, height: 6 };
        let mut buf = Buffer::empty(area);
        assert!(g.place(&mut buf, area, 0, 1, 6, 4, Key { name: "v", seq: Some(2) }, &mut |w, h| Some(vec![90u8; w * h * 3])));
        buf[(2, 2)].set_symbol("x");
        let _ = g.finish(&mut buf);
        let s = sixel_frame(&mut g, 3, 6, 4, &mut |w, h| vec![90u8; w * h * 3]);
        assert!(s.contains("\x1b[2;1H\x1bP"), "the overlay erased part of it, so send it all: {s:?}");
    }

    /// Colour registers defined in a sixel stream: register -> "r,g,b".
    fn sixel_registers(s: &str) -> Vec<(String, String)> {
        let mut defs = Vec::new();
        for d in s.split('#').skip(1) {
            let mut f = d.split(';');
            let (Some(n), Some("2")) = (f.next(), f.next()) else { continue };
            let rgb: Vec<String> = f.take(3).map(|v| v.chars().take_while(|c| c.is_ascii_digit()).collect()).collect();
            defs.push((n.to_string(), rgb.join(",")));
        }
        defs
    }

    /// Bands are drawn in the palette of the last whole frame: every colour
    /// register they define means what it meant in that frame.
    #[test]
    fn sixel_bands_reuse_the_whole_frames_palette() {
        let mut g = sixel_gfx();
        let (cols, rows) = (6u16, 6u16);
        let paint = |p: &mut [u8], w: usize, ch: usize, row: usize, rgb: [u8; 3]| {
            for px in p[row * ch * w * 3..(row + 1) * ch * w * 3].chunks_exact_mut(3) {
                px.copy_from_slice(&rgb);
            }
        };
        let (red, blue) = ([220u8, 20, 20], [20u8, 20, 220]);
        // Frame 1: red top row, blue bottom row. Frame 2: rows 1 and 4 too.
        let mut frame = |seq: u64, w: usize, h: usize| {
            let mut p = vec![90u8; w * h * 3];
            let ch = h / rows as usize;
            paint(&mut p, w, ch, 0, red);
            paint(&mut p, w, ch, 5, blue);
            if seq == 2 {
                paint(&mut p, w, ch, 1, red);
                paint(&mut p, w, ch, 4, blue);
            }
            p
        };
        let first = sixel_frame(&mut g, 1, cols, rows, &mut |w, h| frame(1, w, h));
        let s = sixel_frame(&mut g, 2, cols, rows, &mut |w, h| frame(2, w, h));
        assert_eq!(s.matches("\x1bP").count(), 2, "two bands: {s:?}");
        let defs: HashMap<String, String> = sixel_registers(&first).into_iter().collect();
        let used = sixel_registers(&s);
        assert!(used.len() >= 2);
        for (n, rgb) in used {
            assert_eq!(defs.get(&n), Some(&rgb), "register {n} differs from the whole frame's");
        }
    }

    /// Changed rows in colours the kept palette never saw: refit and send a
    /// whole frame instead of drawing them in the wrong colours.
    #[test]
    fn sixel_new_colours_send_a_whole_frame() {
        let mut g = sixel_gfx();
        let (cols, rows) = (6u16, 6u16);
        let _ = sixel_frame(&mut g, 1, cols, rows, &mut |w, h| vec![90u8; w * h * 3]);
        let s = sixel_frame(&mut g, 2, cols, rows, &mut |w, h| {
            let mut p = vec![90u8; w * h * 3];
            let ch = h / rows as usize;
            for px in p[2 * ch * w * 3..3 * ch * w * 3].chunks_exact_mut(3) {
                px.copy_from_slice(&[230, 40, 200]);
            }
            p
        });
        assert_eq!(s.matches("\x1bP").count(), 1);
        assert!(s.contains("\x1b[2;1H\x1bP"), "whole frame from the top: {s:?}");
    }

    /// Compression-noise-sized changes are not resent; one strongly
    /// changed pixel (a typed character) is.
    #[test]
    fn sixel_skips_noise_but_not_a_strong_change() {
        let mut g = sixel_gfx();
        assert_eq!(g.sixel_skip, SIXEL_SKIP);
        let (cols, rows) = (6u16, 4u16);
        let base = |w: usize, h: usize| vec![90u8; w * h * 3];
        let _ = sixel_frame(&mut g, 1, cols, rows, &mut |w, h| base(w, h));
        // Every pixel off by 10, under the tolerance of 12: nothing to send.
        let s = sixel_frame(&mut g, 2, cols, rows, &mut |w, h| vec![100u8; w * h * 3]);
        assert!(!s.contains("\x1bP"), "noise must not be resent: {s:?}");
        // Back to the grey on screen, but one pixel in cell row 1 jumps by
        // 100: that row goes out.
        let s = sixel_frame(&mut g, 3, cols, rows, &mut |w, h| {
            let mut p = base(w, h);
            let ch = h / rows as usize;
            p[(ch + 2) * w * 3 + 9..][..3].fill(190);
            p
        });
        assert_eq!(s.matches("\x1bP").count(), 1, "one band: {s:?}");
        assert!(s.contains("\x1b[3;1H\x1bP"), "the strong change's row: {s:?}");
    }

    /// Skipped rows are compared with what the screen shows, so a slow
    /// drift is sent once it adds up, not lost frame by frame.
    #[test]
    fn sixel_slow_drift_is_eventually_sent() {
        let mut g = sixel_gfx();
        let (cols, rows) = (6u16, 4u16);
        let _ = sixel_frame(&mut g, 1, cols, rows, &mut |w, h| vec![90u8; w * h * 3]);
        let mut sent = false;
        for k in 1..=4u64 {
            // +5 per frame: each step is under the tolerance on its own.
            let s = sixel_frame(&mut g, 1 + k, cols, rows, &mut |w, h| vec![90 + 5 * k as u8; w * h * 3]);
            sent |= s.contains("\x1bP");
        }
        assert!(sent, "a 20-level drift must reach the screen");
    }
}
