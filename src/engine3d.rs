//! Small real-time 3D software renderer. It draws triangle meshes into an
//! RGB image (which the glyph renderer then turns into ASCII / mosaics):
//! perspective-correct rasterisation with a depth buffer, sun + ambient +
//! rim lighting, Blinn-Phong highlights, shadow mapping with PCF, distance
//! fog, a sky gradient, and an ink-outline pass on depth / normal edges that
//! gives the glyph matcher crisp silhouettes. Work is split into horizontal
//! bands across all cores.

use std::ops::{Add, Mul, Neg, Sub};
use std::sync::Arc;

// ------------------------------------------------------------------ math

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, s: f32) -> V3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
}
impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        v3(-self.x, -self.y, -self.z)
    }
}

impl V3 {
    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 1e-12 {
            self * (1.0 / l)
        } else {
            v3(0.0, 1.0, 0.0)
        }
    }
    /// Component-wise product (colours).
    pub fn mul(self, o: V3) -> V3 {
        v3(self.x * o.x, self.y * o.y, self.z * o.z)
    }
    pub fn lerp(self, o: V3, t: f32) -> V3 {
        self + (o - self) * t
    }
}

/// Row-major 4x4 matrix.
#[derive(Clone, Copy, Debug)]
pub struct M4(pub [[f32; 4]; 4]);

impl M4 {
    pub const I: M4 = M4([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);

    pub fn mul(&self, o: &M4) -> M4 {
        let mut r = [[0.0; 4]; 4];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..4).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        M4(r)
    }

    pub fn translate(t: V3) -> M4 {
        let mut m = M4::I;
        m.0[0][3] = t.x;
        m.0[1][3] = t.y;
        m.0[2][3] = t.z;
        m
    }

    pub fn scale(s: V3) -> M4 {
        let mut m = M4::I;
        m.0[0][0] = s.x;
        m.0[1][1] = s.y;
        m.0[2][2] = s.z;
        m
    }

    pub fn rot_x(a: f32) -> M4 {
        let (s, c) = a.sin_cos();
        M4([[1.0, 0.0, 0.0, 0.0], [0.0, c, -s, 0.0], [0.0, s, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    pub fn rot_y(a: f32) -> M4 {
        let (s, c) = a.sin_cos();
        M4([[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 0.0], [-s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    pub fn rot_z(a: f32) -> M4 {
        let (s, c) = a.sin_cos();
        M4([[c, -s, 0.0, 0.0], [s, c, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]])
    }

    pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> M4 {
        let f = 1.0 / (fovy / 2.0).tan();
        M4([
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, (far + near) / (near - far), 2.0 * far * near / (near - far)],
            [0.0, 0.0, -1.0, 0.0],
        ])
    }

    pub fn ortho(r: f32, near: f32, far: f32) -> M4 {
        M4([
            [1.0 / r, 0.0, 0.0, 0.0],
            [0.0, 1.0 / r, 0.0, 0.0],
            [0.0, 0.0, -2.0 / (far - near), -(far + near) / (far - near)],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    pub fn look_at(eye: V3, target: V3, up: V3) -> M4 {
        let f = (target - eye).norm();
        let s = f.cross(up).norm();
        let u = s.cross(f);
        M4([
            [s.x, s.y, s.z, -s.dot(eye)],
            [u.x, u.y, u.z, -u.dot(eye)],
            [-f.x, -f.y, -f.z, f.dot(eye)],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    pub fn point(&self, p: V3) -> [f32; 4] {
        let m = &self.0;
        let mut o = [0.0; 4];
        for (i, v) in o.iter_mut().enumerate() {
            *v = m[i][0] * p.x + m[i][1] * p.y + m[i][2] * p.z + m[i][3];
        }
        o
    }

    pub fn point3(&self, p: V3) -> V3 {
        let o = self.point(p);
        v3(o[0], o[1], o[2])
    }

    /// Direction (normals; assumes no shear).
    pub fn dir(&self, d: V3) -> V3 {
        let m = &self.0;
        v3(
            m[0][0] * d.x + m[0][1] * d.y + m[0][2] * d.z,
            m[1][0] * d.x + m[1][1] * d.y + m[1][2] * d.z,
            m[2][0] * d.x + m[2][1] * d.y + m[2][2] * d.z,
        )
    }
}

pub fn hsv(h: f32, s: f32, v: f32) -> V3 {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 {
        0 => v3(v, t, p),
        1 => v3(q, v, p),
        2 => v3(p, v, t),
        3 => v3(p, q, v),
        4 => v3(t, p, v),
        _ => v3(v, p, q),
    }
}

// ------------------------------------------------------------------ meshes

#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub p: V3,
    pub n: V3,
    /// Linear colour 0..1
    pub c: V3,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub v: Vec<Vertex>,
    pub t: Vec<[u32; 3]>,
}

impl Mesh {
    fn push(&mut self, p: V3, n: V3, c: V3) -> u32 {
        self.v.push(Vertex { p, n, c });
        (self.v.len() - 1) as u32
    }

    /// Flat-shaded triangle (own vertices), counter-clockwise = front.
    pub fn tri(&mut self, a: V3, b: V3, c: V3, col: V3) {
        let n = (b - a).cross(c - a).norm();
        let i = self.push(a, n, col);
        self.push(b, n, col);
        self.push(c, n, col);
        self.t.push([i, i + 1, i + 2]);
    }

    pub fn quad(&mut self, a: V3, b: V3, c: V3, d: V3, col: V3) {
        self.tri(a, b, c, col);
        self.tri(a, c, d, col);
    }

    pub fn cube(col: V3) -> Mesh {
        let mut m = Mesh::default();
        let p = |x: f32, y: f32, z: f32| v3(x - 0.5, y, z - 0.5); // base on y=0
        let (a, b, c, d) = (p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(1.0, 0.0, 1.0), p(0.0, 0.0, 1.0));
        let (e, f, g, h) = (p(0.0, 1.0, 0.0), p(1.0, 1.0, 0.0), p(1.0, 1.0, 1.0), p(0.0, 1.0, 1.0));
        m.quad(e, h, g, f, col); // top
        m.quad(a, b, c, d, col); // bottom
        m.quad(d, c, g, h, col); // front (+z)
        m.quad(b, a, e, f, col); // back
        m.quad(c, b, f, g, col); // right
        m.quad(a, d, h, e, col); // left
        m
    }

    pub fn sphere(r: f32, seg: usize, rings: usize, col: V3) -> Mesh {
        let mut m = Mesh::default();
        for j in 0..=rings {
            let th = std::f32::consts::PI * j as f32 / rings as f32;
            for i in 0..=seg {
                let ph = std::f32::consts::TAU * i as f32 / seg as f32;
                let n = v3(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin());
                m.push(n * r, n, col);
            }
        }
        let w = seg as u32 + 1;
        for j in 0..rings as u32 {
            for i in 0..seg as u32 {
                let (a, b) = (j * w + i, (j + 1) * w + i);
                m.t.push([a, a + 1, b]);
                m.t.push([a + 1, b + 1, b]);
            }
        }
        m
    }

    /// Tube of radius `r` swept along a closed curve.
    pub fn tube(curve: impl Fn(f32) -> V3, r: f32, seg: usize, sides: usize, col: impl Fn(f32) -> V3) -> Mesh {
        let mut m = Mesh::default();
        let mut prev_n = v3(0.0, 1.0, 0.0);
        for i in 0..=seg {
            let t = i as f32 / seg as f32;
            let p = curve(t);
            let tan = (curve(t + 0.001) - curve(t - 0.001)).norm();
            // Parallel transport keeps the tube from twisting.
            let mut n = (prev_n - tan * prev_n.dot(tan)).norm();
            if i == 0 {
                n = tan.cross(v3(0.3, 1.0, 0.2)).norm();
            }
            prev_n = n;
            let b = tan.cross(n);
            for k in 0..=sides {
                let a = std::f32::consts::TAU * k as f32 / sides as f32;
                let dir = n * a.cos() + b * a.sin();
                m.push(p + dir * r, dir, col(t));
            }
        }
        let w = sides as u32 + 1;
        for i in 0..seg as u32 {
            for k in 0..sides as u32 {
                let (a, b) = (i * w + k, (i + 1) * w + k);
                m.t.push([a, b, a + 1]);
                m.t.push([a + 1, b, b + 1]);
            }
        }
        m
    }

    pub fn torus(big: f32, r: f32, seg: usize, sides: usize, col: V3) -> Mesh {
        let tau = std::f32::consts::TAU;
        Mesh::tube(|t| v3((t * tau).cos() * big, 0.0, (t * tau).sin() * big), r, seg, sides, |_| col)
    }

    pub fn torus_knot(p: f32, q: f32, r: f32, seg: usize, sides: usize, col: impl Fn(f32) -> V3) -> Mesh {
        let tau = std::f32::consts::TAU;
        let curve = move |t: f32| {
            let a = t * tau;
            let rr = 1.0 + 0.45 * (q * a).cos();
            v3(rr * (p * a).cos(), 0.45 * (q * a).sin(), rr * (p * a).sin())
        };
        Mesh::tube(curve, r, seg, sides, col)
    }

    /// Extrude a convex CCW polygon (x, y) along z, centred on z = 0.
    pub fn prism(poly: &[(f32, f32)], depth: f32, col: V3, side_col: V3) -> Mesh {
        let mut m = Mesh::default();
        let z = depth / 2.0;
        let f = |i: usize, z: f32| v3(poly[i].0, poly[i].1, z);
        for i in 1..poly.len() - 1 {
            m.tri(f(0, z), f(i, z), f(i + 1, z), col);
            m.tri(f(0, -z), f(i + 1, -z), f(i, -z), col);
        }
        for i in 0..poly.len() {
            let j = (i + 1) % poly.len();
            m.quad(f(i, -z), f(j, -z), f(j, z), f(i, z), side_col);
        }
        m
    }

    /// Height-field grid over [-size/2, size/2]^2; `f(x, z) -> (y, colour)`.
    pub fn grid(n: usize, size: f32, f: impl Fn(f32, f32) -> (f32, V3)) -> Mesh {
        let mut m = Mesh::default();
        let step = size / n as f32;
        let at = |i: usize| -size / 2.0 + i as f32 * step;
        let mut h = vec![0f32; (n + 1) * (n + 1)];
        let mut cols = vec![V3::default(); (n + 1) * (n + 1)];
        for j in 0..=n {
            for i in 0..=n {
                let (y, c) = f(at(i), at(j));
                h[j * (n + 1) + i] = y;
                cols[j * (n + 1) + i] = c;
            }
        }
        let hy = |i: usize, j: usize| h[j.min(n) * (n + 1) + i.min(n)];
        for j in 0..=n {
            for i in 0..=n {
                let dx = hy(i + 1, j) - hy(i.saturating_sub(1), j);
                let dz = hy(i, j + 1) - hy(i, j.saturating_sub(1));
                let nrm = v3(-dx, 2.0 * step, -dz).norm();
                m.push(v3(at(i), hy(i, j), at(j)), nrm, cols[j * (n + 1) + i]);
            }
        }
        let w = n as u32 + 1;
        for j in 0..n as u32 {
            for i in 0..n as u32 {
                let (a, b) = (j * w + i, (j + 1) * w + i);
                m.t.push([a, b, a + 1]);
                m.t.push([a + 1, b, b + 1]);
            }
        }
        m
    }
}

// ------------------------------------------------------------------ scene

#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub spec: f32,
    pub shine: f32,
    pub emit: f32,
    /// Glowing grid lines on the surface: (colour, spacing in world units).
    /// Thin bright lines on a dark floor read very well as glyphs.
    pub grid: Option<(V3, f32)>,
}

impl Default for Material {
    fn default() -> Self {
        Material { spec: 0.25, shine: 24.0, emit: 0.0, grid: None }
    }
}

#[derive(Clone)]
pub struct Object {
    pub mesh: Arc<Mesh>,
    pub model: M4,
    pub mat: Material,
    pub casts_shadow: bool,
}

pub struct Scene {
    pub objects: Vec<Object>,
    /// Direction *towards* the sun.
    pub sun: V3,
    pub sun_col: V3,
    pub ambient: V3,
    pub sky_top: V3,
    pub sky_bottom: V3,
    /// (start, end) distance of linear fog towards sky_bottom.
    pub fog: Option<(f32, f32)>,
    /// Ink outline strength 0..1.
    pub outline: f32,
    /// Area the shadow map covers.
    pub shadow_center: V3,
    pub shadow_radius: f32,
}

pub struct Camera {
    pub eye: V3,
    pub target: V3,
    pub fov: f32,
}

// ------------------------------------------------------------------ raster

#[derive(Clone, Copy)]
struct CV {
    clip: [f32; 4],
    world: V3,
    n: V3,
    c: V3,
}

fn lerp_cv(a: &CV, b: &CV, t: f32) -> CV {
    let mut clip = [0.0; 4];
    for (i, v) in clip.iter_mut().enumerate() {
        *v = a.clip[i] + (b.clip[i] - a.clip[i]) * t;
    }
    CV { clip, world: a.world.lerp(b.world, t), n: a.n.lerp(b.n, t), c: a.c.lerp(b.c, t) }
}

/// Screen-space triangle ready to rasterise (attributes pre-divided by w).
#[derive(Clone, Copy)]
struct STri {
    x: [f32; 3],
    y: [f32; 3],
    z: [f32; 3],
    iw: [f32; 3],
    world: [V3; 3],
    n: [V3; 3],
    c: [V3; 3],
    mat: Material,
}

const NEAR_W: f32 = 0.05;

/// Clip a triangle against w >= NEAR_W and emit screen triangles.
fn setup(a: CV, b: CV, c: CV, w: f32, h: f32, cull: bool, mat: Material, out: &mut Vec<STri>) {
    let inside = |v: &CV| v.clip[3] >= NEAR_W;
    let mut poly: Vec<CV> = Vec::with_capacity(4);
    let src = [a, b, c];
    for i in 0..3 {
        let (p, q) = (&src[i], &src[(i + 1) % 3]);
        if inside(p) {
            poly.push(*p);
        }
        if inside(p) != inside(q) {
            let t = (NEAR_W - p.clip[3]) / (q.clip[3] - p.clip[3]);
            poly.push(lerp_cv(p, q, t));
        }
    }
    if poly.len() < 3 {
        return;
    }
    let scr = |v: &CV| {
        let iw = 1.0 / v.clip[3];
        ((v.clip[0] * iw * 0.5 + 0.5) * w, (0.5 - v.clip[1] * iw * 0.5) * h, v.clip[2] * iw, iw)
    };
    let s: Vec<_> = poly.iter().map(scr).collect();
    for i in 1..poly.len() - 1 {
        let idx = [0, i, i + 1];
        let (x0, y0) = (s[0].0, s[0].1);
        let area = (s[i].0 - x0) * (s[i + 1].1 - y0) - (s[i + 1].0 - x0) * (s[i].1 - y0);
        // Screen y points down, so front faces (CCW in world) come out negative.
        if area.abs() < 1e-9 || (cull && area > 0.0) {
            continue;
        }
        let mut t = STri {
            x: [0.0; 3],
            y: [0.0; 3],
            z: [0.0; 3],
            iw: [0.0; 3],
            world: [V3::default(); 3],
            n: [V3::default(); 3],
            c: [V3::default(); 3],
            mat,
        };
        for (k, &vi) in idx.iter().enumerate() {
            let (sx, sy, sz, iw) = s[vi];
            t.x[k] = sx;
            t.y[k] = sy;
            t.z[k] = sz;
            t.iw[k] = iw;
            t.world[k] = poly[vi].world * iw;
            t.n[k] = poly[vi].n * iw;
            t.c[k] = poly[vi].c * iw;
        }
        out.push(t);
    }
}

/// Rows [y0, y1) of a w-wide band, as mutable slices of each buffer.
struct Band<'a> {
    y0: usize,
    y1: usize,
    color: &'a mut [u8],
    depth: &'a mut [f32],
    dist: &'a mut [f32],
    nrm: &'a mut [V3],
}

/// Split the frame buffers into horizontal bands, one per work item.
fn bands<'a>(color: &'a mut [u8], depth: &'a mut [f32], dist: &'a mut [f32], nrm: &'a mut [V3], w: usize, h: usize, threads: usize) -> Vec<Band<'a>> {
    let rows = h.div_ceil(threads * 2).max(1);
    let mut out = Vec::new();
    let it = color.chunks_mut(rows * w * 3).zip(depth.chunks_mut(rows * w)).zip(dist.chunks_mut(rows * w)).zip(nrm.chunks_mut(rows * w));
    let mut y0 = 0;
    for (((color, depth), dist), nrm) in it {
        let y1 = (y0 + rows).min(h);
        out.push(Band { y0, y1, color, depth, dist, nrm });
        y0 = y1;
    }
    out
}

struct ShadowMap {
    size: usize,
    depth: Vec<f32>,
    vp: M4,
}

pub struct Engine {
    w: usize,
    h: usize,
    color: Vec<u8>,
    depth: Vec<f32>,
    dist: Vec<f32>,
    nrm: Vec<V3>,
    out: Vec<u8>,
    shadow: ShadowMap,
    threads: usize,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 32);
        Engine {
            w: 0,
            h: 0,
            color: Vec::new(),
            depth: Vec::new(),
            dist: Vec::new(),
            nrm: Vec::new(),
            out: Vec::new(),
            shadow: ShadowMap { size: 768, depth: vec![f32::MAX; 768 * 768], vp: M4::I },
            threads,
        }
    }

    fn render_shadow(&mut self, scene: &Scene) {
        let s = self.shadow.size;
        let r = scene.shadow_radius;
        let eye = scene.shadow_center + scene.sun.norm() * (r * 2.0);
        let up = if scene.sun.norm().y.abs() > 0.95 { v3(0.0, 0.0, 1.0) } else { v3(0.0, 1.0, 0.0) };
        let vp = M4::ortho(r, 0.1, r * 4.0).mul(&M4::look_at(eye, scene.shadow_center, up));
        self.shadow.vp = vp;
        let mut tris = Vec::new();
        for o in scene.objects.iter().filter(|o| o.casts_shadow) {
            let m = vp.mul(&o.model);
            let cv: Vec<CV> = o
                .mesh
                .v
                .iter()
                .map(|v| CV { clip: m.point(v.p), world: V3::default(), n: V3::default(), c: V3::default() })
                .collect();
            for t in &o.mesh.t {
                setup(cv[t[0] as usize], cv[t[1] as usize], cv[t[2] as usize], s as f32, s as f32, false, o.mat, &mut tris);
            }
        }
        let depth = &mut self.shadow.depth;
        depth.iter_mut().for_each(|d| *d = f32::MAX);
        let rows = s.div_ceil(self.threads * 2);
        std::thread::scope(|sc| {
            for (bi, band) in depth.chunks_mut(rows * s).enumerate() {
                let tris = &tris;
                sc.spawn(move || {
                    let (y0, y1) = (bi * rows, (bi * rows + band.len() / s).min(s));
                    for t in tris {
                        raster(t, s, y0, y1, |x, y, z, _b| {
                            let d = &mut band[(y - y0) * s + x];
                            if z < *d {
                                *d = z;
                            }
                        });
                    }
                });
            }
        });
    }

    /// Render `scene` at w x h pixels; returns RGB24.
    pub fn render(&mut self, scene: &Scene, cam: &Camera, w: usize, h: usize) -> &[u8] {
        if (w, h) != (self.w, self.h) {
            self.w = w;
            self.h = h;
            self.color = vec![0; w * h * 3];
            self.depth = vec![0.0; w * h];
            self.dist = vec![0.0; w * h];
            self.nrm = vec![V3::default(); w * h];
            self.out = vec![0; w * h * 3];
        }
        if w == 0 || h == 0 {
            return &self.out;
        }
        self.render_shadow(scene);
        let view = M4::look_at(cam.eye, cam.target, v3(0.0, 1.0, 0.0));
        let proj = M4::perspective(cam.fov, w as f32 / h as f32, 0.05, 200.0);
        let vp = proj.mul(&view);
        let mut tris = Vec::new();
        for o in &scene.objects {
            let mvp = vp.mul(&o.model);
            let cv: Vec<CV> = o
                .mesh
                .v
                .iter()
                .map(|v| CV { clip: mvp.point(v.p), world: o.model.point3(v.p), n: o.model.dir(v.n).norm(), c: v.c })
                .collect();
            for t in &o.mesh.t {
                setup(cv[t[0] as usize], cv[t[1] as usize], cv[t[2] as usize], w as f32, h as f32, true, o.mat, &mut tris);
            }
        }
        let shadow = &self.shadow;
        let eye = cam.eye;
        let bands = bands(&mut self.color, &mut self.depth, &mut self.dist, &mut self.nrm, w, h, self.threads);
        std::thread::scope(|sc| {
            for band in bands {
                let tris = &tris;
                sc.spawn(move || shade_band(band, tris, scene, shadow, eye, w, h));
            }
        });
        // Ink outlines where depth or surface direction jumps.
        if scene.outline > 0.0 {
            let (color, dist, nrm) = (&self.color, &self.dist, &self.nrm);
            let rows = h.div_ceil(self.threads * 2);
            let k = 1.0 - scene.outline;
            let fog = scene.fog;
            std::thread::scope(|sc| {
                for (bi, out) in self.out.chunks_mut(rows * w * 3).enumerate() {
                    sc.spawn(move || {
                        for (ly, row) in out.chunks_mut(w * 3).enumerate() {
                            let y = bi * rows + ly;
                            for x in 0..w {
                                let i = y * w + x;
                                let mut edge = false;
                                let (d, n) = (dist[i], nrm[i]);
                                for (dx, dy) in [(1i32, 0i32), (0, 1), (-1, 0), (0, -1)] {
                                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                                        continue;
                                    }
                                    let j = ny as usize * w + nx as usize;
                                    let d2 = dist[j];
                                    // Only the nearer side of a silhouette gets the ink.
                                    if d < f32::MAX && (d2 - d) > 0.06 * d {
                                        edge = true;
                                    } else if d < f32::MAX && d2 < f32::MAX && (d2 - d).abs() < 0.06 * d && n.dot(nrm[j]) < 0.55 {
                                        edge = true;
                                    }
                                }
                                let o = &mut row[x * 3..x * 3 + 3];
                                let c = &color[i * 3..i * 3 + 3];
                                // Outlines fade into the fog with the surface.
                                let f = if edge {
                                    let fade = fog.map_or(0.0, |(f0, f1)| ((d - f0) / (f1 - f0)).clamp(0.0, 1.0));
                                    k + (1.0 - k) * fade
                                } else {
                                    1.0
                                };
                                o[0] = (c[0] as f32 * f) as u8;
                                o[1] = (c[1] as f32 * f) as u8;
                                o[2] = (c[2] as f32 * f) as u8;
                            }
                        }
                    });
                }
            });
            &self.out
        } else {
            &self.color
        }
    }
}

/// Visit pixel centres covered by `t` in rows [y0, y1): f(x, y, z, bary).
#[inline]
fn raster(t: &STri, w: usize, y0: usize, y1: usize, mut f: impl FnMut(usize, usize, f32, [f32; 3])) {
    let minx = t.x.iter().cloned().fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxx = (t.x.iter().cloned().fold(f32::MIN, f32::max).ceil() as isize).min(w as isize - 1);
    let miny = (t.y.iter().cloned().fold(f32::MAX, f32::min).floor().max(y0 as f32)) as usize;
    let maxy = (t.y.iter().cloned().fold(f32::MIN, f32::max).ceil() as isize).min(y1 as isize - 1);
    if maxx < minx as isize || maxy < miny as isize {
        return;
    }
    let area = (t.x[1] - t.x[0]) * (t.y[2] - t.y[0]) - (t.x[2] - t.x[0]) * (t.y[1] - t.y[0]);
    let inv = 1.0 / area;
    for y in miny..=maxy as usize {
        let py = y as f32 + 0.5;
        for x in minx..=maxx as usize {
            let px = x as f32 + 0.5;
            let w0 = ((t.x[1] - px) * (t.y[2] - py) - (t.x[2] - px) * (t.y[1] - py)) * inv;
            let w1 = ((t.x[2] - px) * (t.y[0] - py) - (t.x[0] - px) * (t.y[2] - py)) * inv;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * t.z[0] + w1 * t.z[1] + w2 * t.z[2];
            f(x, y, z, [w0, w1, w2]);
        }
    }
}

fn sky(scene: &Scene, y: usize, h: usize) -> V3 {
    let t = y as f32 / h.max(1) as f32;
    scene.sky_top.lerp(scene.sky_bottom, t.powf(0.8))
}

fn shadow_at(sm: &ShadowMap, p: V3, n: V3, sun: V3) -> f32 {
    let q = sm.vp.point(p + n * 0.02);
    let (u, v, z) = ((q[0] * 0.5 + 0.5) * sm.size as f32, (0.5 - q[1] * 0.5) * sm.size as f32, q[2]);
    if u < 1.0 || v < 1.0 || u >= sm.size as f32 - 1.0 || v >= sm.size as f32 - 1.0 {
        return 1.0;
    }
    let bias = 0.004 + 0.01 * (1.0 - n.dot(sun).clamp(0.0, 1.0));
    let mut lit = 0.0;
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let idx = (v as i32 + dy) as usize * sm.size + (u as i32 + dx) as usize;
            if z - bias <= sm.depth[idx] {
                lit += 1.0;
            }
        }
    }
    lit / 9.0
}

fn shade_band(band: Band, tris: &[STri], scene: &Scene, sm: &ShadowMap, eye: V3, w: usize, h: usize) {
    let Band { y0, y1, color, depth, dist, nrm } = band;
    // Clear to sky.
    for y in y0..y1 {
        let s = sky(scene, y, h);
        let (r, g, b) = (gamma(s.x), gamma(s.y), gamma(s.z));
        for x in 0..w {
            let i = (y - y0) * w + x;
            depth[i] = f32::MAX;
            dist[i] = f32::MAX;
            nrm[i] = V3::default();
            color[i * 3] = r;
            color[i * 3 + 1] = g;
            color[i * 3 + 2] = b;
        }
    }
    let sun = scene.sun.norm();
    for t in tris {
        raster(t, w, y0, y1, |x, y, z, b| {
            let i = (y - y0) * w + x;
            if z >= depth[i] || z < -1.0 {
                return;
            }
            depth[i] = z;
            let iw = b[0] * t.iw[0] + b[1] * t.iw[1] + b[2] * t.iw[2];
            let k = 1.0 / iw;
            let wp = (t.world[0] * b[0] + t.world[1] * b[1] + t.world[2] * b[2]) * k;
            let n = ((t.n[0] * b[0] + t.n[1] * b[1] + t.n[2] * b[2]) * k).norm();
            let base = (t.c[0] * b[0] + t.c[1] * b[1] + t.c[2] * b[2]) * k;
            let to_eye = eye - wp;
            let d = to_eye.len();
            let v = to_eye * (1.0 / d.max(1e-6));
            // Light whichever side faces the viewer (thin / open meshes).
            let n = if n.dot(v) < 0.0 { -n } else { n };
            let diff = n.dot(sun).max(0.0);
            let sh = if diff > 0.0 { shadow_at(sm, wp, n, sun) } else { 0.0 };
            let half = (sun + v).norm();
            let spec = t.mat.spec * n.dot(half).max(0.0).powf(t.mat.shine);
            let rim = (1.0 - n.dot(v).max(0.0)).powi(3) * 0.35;
            let mut c = base.mul(scene.ambient + scene.sun_col * (diff * sh)) + scene.sun_col * (spec * sh) + scene.sky_top * rim + base * t.mat.emit;
            if let Some((gc, sp)) = t.mat.grid {
                let fx = (wp.x / sp).rem_euclid(1.0);
                let fz = (wp.z / sp).rem_euclid(1.0);
                let dl = fx.min(1.0 - fx).min(fz.min(1.0 - fz));
                // Widen with distance so far lines don't shimmer away.
                let lw = 0.05 + d * 0.006;
                let line = (1.0 - dl / lw).clamp(0.0, 1.0);
                c = c + gc * (line * (0.35 + 0.65 * sh));
            }
            if let Some((f0, f1)) = scene.fog {
                let f = ((d - f0) / (f1 - f0)).clamp(0.0, 1.0);
                c = c.lerp(sky(scene, y, h), f * f);
            }
            dist[i] = d;
            nrm[i] = n;
            color[i * 3] = gamma(c.x);
            color[i * 3 + 1] = gamma(c.y);
            color[i * 3 + 2] = gamma(c.z);
        });
    }
}

#[inline]
fn gamma(v: f32) -> u8 {
    // Reinhard-ish shoulder, then gamma 2.0.
    let v = v.max(0.0);
    let v = v / (1.0 + 0.25 * v);
    (v.sqrt().min(1.0) * 255.0) as u8
}
