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
        // Sixel can't scale, so it needs a believable cell size.
        let sixel = self.sixel && self.cell.is_some_and(plausible);
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
        if self.sixel && !self.cell.is_some_and(plausible) {
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
    let mut filled = vec![false; 1 << 15];
    for &b in &bins {
        filled[b as usize] = true;
    }
    // Light ordered dither: +-half a 5-bit step before binning.
    let mut idx = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = &rgb[(y * w + x) * 3..];
            // Bayer offset scaled to +-dither/2 levels.
            let t = (BAYER[y & 3][x & 3] as i32 * 2 - 15) * dither / 32;
            let d = |v: u8| (v as i32 + t).clamp(0, 255) as u8;
            let (r, g, b) = (d(p[0]), d(p[1]), d(p[2]));
            let k = bin(r, g, b);
            if !filled[k] {
                lut[k] = near((r as u32 & !7) + 4, (g as u32 & !7) + 4, (b as u32 & !7) + 4);
                filled[k] = true;
            }
            idx[y * w + x] = lut[k];
        }
    }
    (colors, idx)
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
    data: Option<Vec<u8>>,
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
    next_id: u32,
    statics: HashMap<String, (u32, u16, u16)>,
    static_order: VecDeque<String>,
    dynamic: HashMap<String, (u32, u64, u16, u16)>,
    out: Vec<u8>,
    sixel_prev: Vec<(Rect, u64)>,
    sixel_now: Vec<SixelPlace>,
    /// Re-emit every sixel next frame (after a clear / resize).
    pub force: bool,
    /// Send moving pictures at half vertical resolution (2:1 pixels).
    pub sixel_tall: bool,
    /// Palette size for moving pictures (fewer = smaller, faster, blotchier).
    pub video_colors: usize,
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
            cell: caps.cell.unwrap_or((10, 20)),
            tmux: caps.tmux,
            max_px: if remote { 640 } else { 1280 },
            sixel_tall: std::env::var_os("YTUI_SIXEL_FULL").is_none(),
            video_colors: 255,
            max_fps: if remote { 12.0 } else { 0.0 },
            sent: HashMap::new(),
            next_id: 1,
            statics: HashMap::new(),
            static_order: VecDeque::new(),
            dynamic: HashMap::new(),
            out: Vec::new(),
            sixel_prev: Vec::new(),
            sixel_now: Vec::new(),
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
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        let _ = z.write_all(rgb);
        let data = B64.encode(z.finish().unwrap_or_default());
        let bytes = data.as_bytes();
        let mut first = true;
        let mut i = 0;
        while i < bytes.len() || first {
            let end = (i + 4096).min(bytes.len());
            let more = (end < bytes.len()) as u8;
            let mut seq = Vec::with_capacity(end - i + 96);
            if first {
                let _ = write!(seq, "\x1b_Ga=T,U=1,q=2,f=24,o=z,i={id},s={w},v={h},c={cols},r={rows},m={more};");
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
                                self.kitty_send(id, &px, pw, ph, cols, rows);
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
                let fresh = self.force || !self.sixel_prev.contains(&(rect, sig));
                let data = if fresh {
                    let (cw, ch) = (self.cell.0 as usize, self.cell.1 as usize);
                    // Moving pictures: half the rows at 2:1 pixel aspect,
                    // half the bytes for the terminal to parse.
                    let aspect = if key.seq.is_some() && self.sixel_tall && ch % 2 == 0 { 2 } else { 1 };
                    let ch = ch / aspect;
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
                    let mut enc = Vec::new();
                    let pal = if key.seq.is_some() { Palette::Adaptive { colors: self.video_colors, dither: 8 } } else { STILL_PALETTE };
                    sixel_encode_with(&crop, vx.min(pw), vy1.min(ph) - vy0, aspect, pal, &mut enc);
                    Some(enc)
                } else {
                    None
                };
                self.sixel_now.push(SixelPlace { rect, sig, data });
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
                if let Some(d) = p.data {
                    let _ = write!(self.out, "\x1b[{};{}H", r.y + 1, r.x + 1);
                    self.out.extend_from_slice(&d);
                }
                prev.push((r, p.sig));
            }
            self.sixel_prev = prev;
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
}
