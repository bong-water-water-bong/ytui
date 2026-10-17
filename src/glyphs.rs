//! Shape vectors for the glow mode's characters.
//!
//! Ported from react-video-ascii by Lucas Jin (MIT, see NOTICE):
//! `computeShapeVectors` in src/lib/ascii-utils.ts. Each character is
//! rasterised from a real monospace font and described by how much ink falls
//! inside six circles in a 2x3 grid (centres at 1/4 and 3/4 of the width,
//! 1/4, 1/2 and 3/4 of the height, radius a fifth of the width). Each
//! dimension is then normalised by its maximum over the whole set. Picture
//! cells are sampled with the same circles, so the nearest vector matches
//! shape (edges, slants, top or bottom weight), not just brightness.

use fontdue::{Font, FontSettings};

/// Decoded pixels per terminal cell. 6x12 keeps the 1:2 cell aspect and
/// splits evenly into braille's 2x4 dots (3x3 pixels each).
pub const CW: usize = 6;
pub const CH: usize = 12;

/// Circle centres (u, v) in cell units, in the reference's order:
/// TL, TR, ML, MR, BL, BR.
pub const CIRCLES: [(f32, f32); 6] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.50), (0.75, 0.50), (0.25, 0.75), (0.75, 0.75)];

/// Circle radius in cell widths.
pub const RADIUS: f32 = 0.2;

/// The reference's default character set, darkest to brightest.
pub const CHARS: &str = " `.',-_:!;|\"~+^lr[](\\/L)>t<v=Tz?icf1{sIxY*jJno}CZyVwmSXRqM$O%#9&NW0Q@";

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

/// Pixel offsets (relative to a cell origin) whose centres lie inside the
/// circle at (u, v) for a cell of `w` x `h` pixels. Never empty: a circle
/// smaller than a pixel keeps the pixel under its centre.
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
        // Rasterise well above cell size so the small circles see real ink.
        const S: usize = 8;
        let (w, h) = (CW * S, CH * S);
        let adv = font.metrics('M', 100.0).advance_width / 100.0;
        let px = w as f32 / adv;
        let (ascent, descent) = font.horizontal_line_metrics(px).map(|m| (m.ascent, m.descent)).unwrap_or((px * 0.8, -px * 0.2));
        let baseline = ((h as f32 - (ascent - descent)) / 2.0 + ascent).round() as i32;
        let circles: Vec<Vec<(i32, i32)>> = CIRCLES.iter().map(|&(u, v)| circle_offsets(u, v, w, h)).collect();

        let mut chars = Vec::new();
        let mut vecs = Vec::new();
        let mut cov = vec![0f32; w * h];
        for c in CHARS.chars() {
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
        // Normalise each dimension by its maximum over the set.
        for d in 0..6 {
            let mx = vecs.iter().map(|v| v[d]).fold(0f32, f32::max);
            if mx > 0.0 {
                vecs.iter_mut().for_each(|v| v[d] /= mx);
            }
        }
        Glyphs { chars, vecs }
    }

    /// Index of the character whose shape vector is nearest to `s`.
    pub fn nearest(&self, s: &[f32; 6]) -> usize {
        let mut best = (f32::MAX, 0);
        for (i, v) in self.vecs.iter().enumerate() {
            let mut d = 0.0;
            for k in 0..6 {
                let t = s[k] - v[k];
                d += t * t;
            }
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_is_empty_and_full_glyphs_are_dense() {
        let g = Glyphs::load();
        assert_eq!(g.chars[0], ' ');
        assert_eq!(g.nearest(&[0.0; 6]), 0, "no ink matches the space");
        let full = g.chars[g.nearest(&[1.0; 6])];
        assert!("@#%&$0QNWM8".contains(full), "all ink matches a dense glyph, got {full:?}");
    }
}
