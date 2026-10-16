//! Shape vectors for printable ASCII glyphs.
//!
//! Each glyph is rasterised from a real monospace font and described by how
//! much ink falls inside six sampling circles laid out in a staggered 2x3
//! grid. Image cells are sampled with the same circles, so picking the glyph
//! with the nearest vector matches *shape* (edges, slants, top/bottom weight)
//! rather than just brightness. Technique after Alex Harri, "ASCII characters
//! are not pixels" (2026).

use fontdue::{Font, FontSettings};

/// Decoded pixels per terminal cell. 6x12 keeps the 1:2 cell aspect and
/// divides evenly into every mosaic mode (half 6x6, quad 3x6, sext 3x4,
/// braille 3x3).
pub const CW: usize = 6;
pub const CH: usize = 12;

/// Circle radius in cell widths.
pub const RADIUS: f32 = 0.31;

/// Inner circles (u, v) in cell units: left column lowered, right raised.
/// Order: TL, TR, ML, MR, BL, BR.
pub const INNER: [(f32, f32); 6] = [
    (0.28, 0.20),
    (0.72, 0.15),
    (0.28, 0.52),
    (0.72, 0.47),
    (0.28, 0.84),
    (0.72, 0.79),
];

/// Outer circles reaching into neighbouring cells.
/// 0,1 above; 2,4,6 left side; 3,5,7 right side; 8,9 below.
pub const OUTER: [(f32, f32); 10] = [
    (0.28, -0.12),
    (0.72, -0.17),
    (-0.25, 0.20),
    (1.25, 0.15),
    (-0.25, 0.52),
    (1.25, 0.47),
    (-0.25, 0.84),
    (1.25, 0.79),
    (0.28, 1.16),
    (0.72, 1.11),
];

/// Which outer circles influence each inner circle (directional contrast).
pub const AFFECT: [&[usize]; 6] = [
    &[0, 1, 2, 4],
    &[0, 1, 3, 5],
    &[2, 4, 6],
    &[3, 5, 7],
    &[4, 6, 8, 9],
    &[5, 7, 8, 9],
];

const FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/System/Library/Fonts/Menlo.ttc",
    "C:\\Windows\\Fonts\\CascadiaMono.ttf",
    "C:\\Windows\\Fonts\\consola.ttf",
];

pub struct Glyphs {
    pub chars: Vec<char>,
    pub vecs: Vec<[f32; 6]>,
}

/// Pixel offsets (relative to a cell origin) whose centres lie inside a
/// circle at (u, v) for a cell of `w` x `h` pixels.
pub fn circle_offsets(u: f32, v: f32, w: usize, h: usize) -> Vec<(i32, i32)> {
    let (cx, cy, r) = (u * w as f32, v * h as f32, RADIUS * w as f32);
    let mut pts = Vec::new();
    let (x0, x1) = ((cx - r).floor() as i32, (cx + r).ceil() as i32);
    let (y0, y1) = ((cy - r).floor() as i32, (cy + r).ceil() as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= r * r {
                pts.push((x, y));
            }
        }
    }
    if pts.is_empty() {
        pts.push((cx as i32, cy as i32));
    }
    pts
}

impl Glyphs {
    pub fn load() -> Glyphs {
        let env = std::env::var("YTUI_FONT").ok();
        for p in env.iter().map(|s| s.as_str()).chain(FONT_PATHS.iter().copied()) {
            if let Ok(bytes) = std::fs::read(p) {
                if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                    return Self::from_font(&font);
                }
            }
        }
        Self::ramp()
    }

    /// Brightness-ramp fallback when no font file is available.
    fn ramp() -> Glyphs {
        let chars: Vec<char> = " .:-=+*#%@".chars().collect();
        let n = chars.len() as f32 - 1.0;
        let vecs = (0..chars.len()).map(|i| [i as f32 / n; 6]).collect();
        Glyphs { chars, vecs }
    }

    fn from_font(font: &Font) -> Glyphs {
        const S: usize = 8;
        let (w, h) = (CW * S, CH * S);
        let adv = font.metrics('M', 100.0).advance_width / 100.0;
        let px = w as f32 / adv;
        let (ascent, descent) = font
            .horizontal_line_metrics(px)
            .map(|m| (m.ascent, m.descent))
            .unwrap_or((px * 0.8, -px * 0.2));
        let baseline = ((h as f32 - (ascent - descent)) / 2.0 + ascent).round() as i32;
        let circles: Vec<Vec<(i32, i32)>> =
            INNER.iter().map(|&(u, v)| circle_offsets(u, v, w, h)).collect();

        let mut chars = Vec::new();
        let mut vecs = Vec::new();
        let mut cov = vec![0f32; w * h];
        for c in ' '..='~' {
            if c == '`' {
                continue; // reads as noise next to '\''
            }
            cov.iter_mut().for_each(|x| *x = 0.0);
            let (m, bmp) = font.rasterize(c, px);
            let x0 = m.xmin;
            let y0 = baseline - (m.height as i32 + m.ymin);
            for gy in 0..m.height {
                for gx in 0..m.width {
                    let (x, y) = (x0 + gx as i32, y0 + gy as i32);
                    if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                        cov[y as usize * w + x as usize] = bmp[gy * m.width + gx] as f32 / 255.0;
                    }
                }
            }
            let mut v = [0f32; 6];
            for (i, pts) in circles.iter().enumerate() {
                let mut s = 0.0;
                for &(x, y) in pts {
                    if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                        s += cov[y as usize * w + x as usize];
                    }
                }
                v[i] = s / pts.len() as f32;
            }
            chars.push(c);
            vecs.push(v);
        }
        // Normalise each dimension so the densest glyph reaches 1.0; otherwise
        // every vector huddles near the origin and only a few glyphs win.
        for d in 0..6 {
            let mx = vecs.iter().map(|v| v[d]).fold(0f32, f32::max);
            if mx > 0.0 {
                vecs.iter_mut().for_each(|v| v[d] /= mx);
            }
        }
        Glyphs { chars, vecs }
    }

    pub fn nearest(&self, s: &[f32; 6]) -> usize {
        let mut best = (f32::MAX, 0);
        for (i, v) in self.vecs.iter().enumerate() {
            let mut d = 0.0;
            for k in 0..6 {
                let t = v[k] - s[k];
                d += t * t;
            }
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }
}
