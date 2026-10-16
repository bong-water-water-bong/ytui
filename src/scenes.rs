//! Built-in 3D scenes for the engine: the ytui logo, a torus knot, a
//! procedural mountain flythrough, and an audio visualiser. Each frame is
//! built from cached meshes plus a few per-frame ones (waves, terrain).

use crate::engine3d::{hsv, v3, Camera, Material, Mesh, Object, Scene, M4, V3};
use std::f32::consts::{PI, TAU};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Logo,
    Knot,
    Terrain,
    Visualizer,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Logo, Kind::Knot, Kind::Terrain, Kind::Visualizer];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Logo => "logo",
            Kind::Knot => "knot",
            Kind::Terrain => "terrain",
            Kind::Visualizer => "visualizer",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.name().starts_with(&s.to_ascii_lowercase()))
    }
}

/// User camera adjustments on top of each scene's own motion.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Orbit { yaw: 0.0, pitch: 0.0, zoom: 1.0 }
    }
}

pub struct Scenes {
    logo: Arc<Mesh>,
    ring: Arc<Mesh>,
    knot: Arc<Mesh>,
    cube: Arc<Mesh>,
    ball: Arc<Mesh>,
}

impl Default for Scenes {
    fn default() -> Self {
        Scenes::new()
    }
}

fn obj(mesh: Arc<Mesh>, model: M4, mat: Material) -> Object {
    Object { mesh, model, mat, casts_shadow: true }
}

/// Smooth value noise in [-1, 1] and a few octaves of it.
fn hash(x: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374761393) ^ (z as u32).wrapping_mul(668265263);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 32767.5 - 1.0
}

fn noise(x: f32, z: f32) -> f32 {
    let (xi, zi) = (x.floor() as i32, z.floor() as i32);
    let (fx, fz) = (x - x.floor(), z - z.floor());
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (u, v) = (s(fx), s(fz));
    let a = hash(xi, zi) + (hash(xi + 1, zi) - hash(xi, zi)) * u;
    let b = hash(xi, zi + 1) + (hash(xi + 1, zi + 1) - hash(xi, zi + 1)) * u;
    a + (b - a) * v
}

fn fbm(x: f32, z: f32) -> f32 {
    let (mut sum, mut amp, mut f) = (0.0, 1.0, 1.0);
    for _ in 0..5 {
        sum += noise(x * f, z * f) * amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum
}

fn terrain_height(x: f32, z: f32) -> f32 {
    let h = fbm(x * 0.08, z * 0.08);
    // Ridged peaks, flattened valleys, sea level at 0.
    let ridge = 1.0 - (fbm(x * 0.05 + 40.0, z * 0.05) * 1.3).abs();
    (h * 3.2 + ridge * ridge * 4.0 - 0.6).max(-0.3)
}

impl Scenes {
    pub fn new() -> Scenes {
        // Original ytui mark: a bevelled play arrow inside an orbiting ring.
        let arrow = [(-0.55, -0.70), (-0.40, -0.80), (0.80, -0.10), (0.80, 0.10), (-0.40, 0.80), (-0.55, 0.70)];
        Scenes {
            logo: Arc::new(Mesh::prism(&arrow, 0.38, v3(0.95, 0.12, 0.10), v3(0.55, 0.05, 0.05))),
            ring: Arc::new(Mesh::torus(1.35, 0.09, 96, 12, v3(0.85, 0.85, 0.9))),
            knot: Arc::new(Mesh::torus_knot(2.0, 3.0, 0.22, 360, 18, |t| hsv(0.95 + t * 0.35, 0.8, 1.0))),
            cube: Arc::new(Mesh::cube(v3(1.0, 1.0, 1.0))),
            ball: Arc::new(Mesh::sphere(1.0, 40, 24, v3(1.0, 1.0, 1.0))),
        }
    }

    /// Build the scene for time `t` (seconds). `spectrum` (0..1 per band)
    /// drives the visualiser; empty means no audio.
    pub fn build(&self, kind: Kind, t: f32, spectrum: &[f32], orbit: Orbit) -> (Scene, Camera) {
        let mut sc = Scene {
            objects: Vec::new(),
            sun: v3(-0.5, 0.9, 0.35).norm(),
            sun_col: v3(1.6, 1.45, 1.3),
            ambient: v3(0.10, 0.11, 0.16),
            sky_top: v3(0.0, 0.0, 0.01),
            sky_bottom: v3(0.04, 0.04, 0.08),
            fog: Some((7.0, 18.0)),
            outline: 0.85,
            shadow_center: v3(0.0, 0.0, 0.0),
            shadow_radius: 4.5,
        };
        let cam_at = |dist: f32, height: f32, speed: f32, target: V3| {
            let yaw = t * speed + orbit.yaw;
            let pitch = (height / dist).atan() + orbit.pitch;
            let d = dist * orbit.zoom;
            Camera {
                eye: target + v3(yaw.sin() * pitch.cos() * d, pitch.sin() * d, yaw.cos() * pitch.cos() * d),
                target,
                fov: 0.9,
            }
        };
        let cam = match kind {
            Kind::Logo => {
                let spin = M4::rot_y(t * 0.9).mul(&M4::rot_x((t * 0.7).sin() * 0.25));
                let bob = M4::translate(v3(0.0, 0.25 + (t * 1.3).sin() * 0.12, 0.0));
                sc.objects.push(obj(self.logo.clone(), bob.mul(&spin), Material { spec: 0.6, shine: 40.0, emit: 0.15, grid: None }));
                let ring = bob.mul(&M4::rot_z(0.35)).mul(&M4::rot_x(PI / 2.0 + (t * 0.8).sin() * 0.5)).mul(&M4::rot_y(t * 1.7));
                sc.objects.push(obj(self.ring.clone(), ring, Material { spec: 0.9, shine: 60.0, emit: 0.05, grid: None }));
                let floor = Mesh::grid(90, 40.0, |x, z| {
                    let r = (x * x + z * z).sqrt();
                    let y = -1.25 + (r * 2.4 - t * 2.2).sin() * 0.07 / (1.0 + r * 0.25);
                    (y, v3(0.02, 0.025, 0.05))
                });
                let lines = Some((v3(0.3, 0.65, 1.4), 1.0));
                sc.objects.push(Object { mesh: Arc::new(floor), model: M4::I, mat: Material { spec: 0.2, shine: 30.0, emit: 0.0, grid: lines }, casts_shadow: false });
                cam_at(4.6, 1.4, 0.25, v3(0.0, 0.0, 0.0))
            }
            Kind::Knot => {
                sc.ambient = v3(0.12, 0.10, 0.10);
                let m = M4::translate(v3(0.0, 0.35, 0.0)).mul(&M4::rot_y(t * 0.6)).mul(&M4::rot_x(t * 0.37));
                sc.objects.push(obj(self.knot.clone(), m, Material { spec: 1.1, shine: 70.0, emit: 0.12, grid: None }));
                let floor = Mesh::grid(4, 40.0, |_, _| (-1.35, v3(0.03, 0.025, 0.025)));
                let lines = Some((v3(1.1, 0.55, 0.3), 1.0));
                sc.objects.push(Object { mesh: Arc::new(floor), model: M4::I, mat: Material { spec: 0.1, shine: 8.0, emit: 0.0, grid: lines }, casts_shadow: false });
                cam_at(4.4, 1.2, 0.18, v3(0.0, 0.0, 0.0))
            }
            Kind::Terrain => {
                sc.sky_top = v3(0.0, 0.0, 0.02);
                sc.sky_bottom = v3(0.16, 0.07, 0.10);
                // Light from behind the camera so the slopes facing us are lit.
                sc.sun = v3(0.35, 0.75, 1.0).norm();
                sc.sun_col = v3(1.7, 1.35, 1.0);
                sc.ambient = v3(0.20, 0.20, 0.28);
                sc.fog = Some((10.0, 34.0));
                sc.outline = 0.6;
                let speed = 2.2;
                let oz = t * speed;
                let size = 44.0;
                let mesh = Mesh::grid(110, size, |x, z| {
                    let wz = z - oz;
                    let y = terrain_height(x, wz);
                    let c = if y <= -0.29 {
                        v3(0.03, 0.08, 0.18)
                    } else if y < 0.15 {
                        v3(0.76, 0.70, 0.48)
                    } else if y < 1.6 {
                        v3(0.22, 0.48, 0.18)
                    } else if y < 3.0 {
                        v3(0.42, 0.38, 0.34)
                    } else {
                        v3(0.95, 0.95, 1.0)
                    };
                    (y, c)
                });
                // Re-centre so the camera stays near the middle of the grid.
                sc.objects.push(Object { mesh: Arc::new(mesh), model: M4::translate(v3(0.0, 0.0, -size * 0.25)), mat: Material { spec: 0.05, shine: 16.0, emit: 0.0, grid: None }, casts_shadow: true });
                sc.shadow_center = v3(0.0, 0.0, -size * 0.25 - 6.0);
                sc.shadow_radius = 14.0;
                let ground = terrain_height(orbit.yaw * 4.0, -(oz - size * 0.25) + size * 0.25).max(0.0);
                let eye = v3(orbit.yaw * 4.0, ground + 3.2 + orbit.pitch * 4.0, 6.0 * orbit.zoom);
                Camera { eye, target: eye + v3((t * 0.3).sin() * 0.6, -0.9, -4.0), fov: 1.0 }
            }
            Kind::Visualizer => {
                sc.ambient = v3(0.10, 0.09, 0.14);
                sc.sun = v3(0.3, 1.0, 0.4).norm();
                sc.fog = Some((9.0, 22.0));
                let n = 48;
                let bands: Vec<f32> = if spectrum.is_empty() {
                    // No audio: a gentle idle wave.
                    (0..n).map(|i| 0.15 + 0.12 * ((i as f32 * 0.4 + t * 2.0).sin() * 0.5 + 0.5)).collect()
                } else {
                    (0..n).map(|i| spectrum[i * spectrum.len() / n]).collect()
                };
                let bass = bands[..4].iter().sum::<f32>() / 4.0;
                for (i, &lvl) in bands.iter().enumerate() {
                    let a = i as f32 / n as f32 * TAU + t * 0.15;
                    let r = 2.6;
                    let hgt = 0.08 + lvl * 2.2;
                    let col = hsv(i as f32 / n as f32 * 0.85 + 0.55, 0.85, 1.0);
                    let m = M4::translate(v3(a.cos() * r, -1.0, a.sin() * r))
                        .mul(&M4::rot_y(-a))
                        .mul(&M4::scale(v3(0.2, hgt, 0.2)));
                    let mesh = Arc::new(recolor(&self.cube, col));
                    sc.objects.push(obj(mesh, m, Material { spec: 0.5, shine: 30.0, emit: 0.25 + lvl * 0.9, grid: None }));
                }
                let s = 0.55 + bass * 0.55;
                let core = M4::translate(v3(0.0, 0.0, 0.0)).mul(&M4::rot_y(t)).mul(&M4::scale(v3(s, s, s)));
                let core_col = hsv(0.92 + bass * 0.2, 0.7, 1.0);
                sc.objects.push(obj(Arc::new(recolor(&self.ball, core_col)), core, Material { spec: 1.0, shine: 50.0, emit: 0.3 + bass, grid: None }));
                let floor = Mesh::grid(80, 40.0, |x, z| {
                    let r = (x * x + z * z).sqrt();
                    let y = -1.0 + (r * 3.0 - t * 4.0).sin() * 0.12 * bass / (1.0 + r * 0.3);
                    (y, v3(0.02, 0.015, 0.035))
                });
                // Plain dark floor: it catches the bars' glow and shadows
                // without competing with them.
                sc.objects.push(Object { mesh: Arc::new(floor), model: M4::I, mat: Material { spec: 0.6, shine: 40.0, emit: 0.0, grid: None }, casts_shadow: false });
                sc.shadow_radius = 4.2;
                cam_at(7.2, 4.6, 0.2, v3(0.0, -0.2, 0.0))
            }
        };
        (sc, cam)
    }
}

fn recolor(m: &Mesh, c: V3) -> Mesh {
    let mut out = m.clone();
    out.v.iter_mut().for_each(|v| v.c = c);
    out
}

/// Frequency analyser for the visualiser: Hann-windowed FFT of the newest
/// samples, folded into log-spaced bands with fast attack / slow decay.
pub struct Analyzer {
    pub bands: Vec<f32>,
    peak: f32,
}

impl Default for Analyzer {
    fn default() -> Self {
        Analyzer { bands: vec![0.0; 64], peak: 1e-3 }
    }
}

impl Analyzer {
    /// `samples`: mono at 48 kHz, newest last.
    pub fn update(&mut self, samples: &[f32], dt: f32) {
        const N: usize = 2048;
        if samples.len() < N {
            self.bands.iter_mut().for_each(|b| *b *= (1.0 - dt * 3.0).max(0.0));
            return;
        }
        let s = &samples[samples.len() - N..];
        let mut re: Vec<f32> = s.iter().enumerate().map(|(i, x)| x * (0.5 - 0.5 * (TAU * i as f32 / N as f32).cos())).collect();
        let mut im = vec![0f32; N];
        fft(&mut re, &mut im);
        let nb = self.bands.len();
        let (f_lo, f_hi, sr) = (40.0f32, 16000.0f32, 48000.0f32);
        let mut raw = vec![0f32; nb];
        for (b, r) in raw.iter_mut().enumerate() {
            let lo = f_lo * (f_hi / f_lo).powf(b as f32 / nb as f32);
            let hi = f_lo * (f_hi / f_lo).powf((b + 1) as f32 / nb as f32);
            let (k0, k1) = ((lo / sr * N as f32) as usize, ((hi / sr * N as f32) as usize).max((lo / sr * N as f32) as usize + 1));
            let mut m = 0f32;
            for k in k0..k1.min(N / 2) {
                m = m.max((re[k] * re[k] + im[k] * im[k]).sqrt());
            }
            *r = m;
        }
        let frame_peak = raw.iter().cloned().fold(0f32, f32::max);
        self.peak = self.peak.max(frame_peak) * (1.0 - dt * 0.15) + frame_peak * dt * 0.15;
        for (b, &r) in self.bands.iter_mut().zip(&raw) {
            let v = ((r / self.peak.max(1e-3)).max(1e-4).log10() / 2.0 + 1.0).clamp(0.0, 1.0);
            *b = if v > *b { v } else { (*b - dt * 1.6).max(v) };
        }
    }
}

/// In-place iterative radix-2 FFT (len must be a power of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}
