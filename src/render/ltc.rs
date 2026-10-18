//! Rectangle lights (softboxes), lit by linearly transformed cosines (Heitz,
//! Dupuy, Hill and Neubelt, "Real-Time Polygonal-Light Shading with Linearly
//! Transformed Cosines", 2016). That is what gives gloss the long, soft
//! studio highlight of a real softbox instead of a pinpoint.
//!
//! The idea: the GGX highlight for a given roughness and view angle is very
//! nearly a plain cosine lobe put through one 3x3 matrix. Run the light's
//! corners through that matrix's inverse and the highlight's integral over
//! the light becomes the integral of a cosine over a polygon, which has an
//! exact closed form (`integral` below, `ltc_integral` in sculpt_pbr.wgsl).
//! Diffuse light is the same with no matrix at all.
//!
//! The matrices are fitted once, numerically, to the surface shader's own
//! GGX (height-correlated Smith visibility, roughness clamped as it clamps
//! it), by the paper's own procedure: `write_the_table` below, an ignored
//! test, wrote `assets/ltc.bin`. A table of 64 roughnesses by 64 view
//! angles; each entry the inverse matrix's four varying terms, scaled so its
//! middle term is 1. How much of the light a surface reflects in all comes
//! from the split-sum table (dfg.rs), as for the surroundings.

use glam::{Mat3, Vec3};

use super::Gpu;

/// Entries along each side: roughness across, view angle down.
pub const SIZE: usize = 64;
/// The fitted table, as the ignored test wrote it: SIZE x SIZE entries of
/// four little-endian f32s, roughness across.
static TABLE: &[u8] = include_bytes!("../../assets/ltc.bin");
/// The smallest linear roughness the surface shader lights with
/// (`physically_shade` in sculpt_pbr.wgsl): the table stops there too.
const MIN_ALPHA: f32 = 0.0078125;

/// The table: per entry the inverse matrix's terms (0,0), (0,2), (2,0), (2,2)
/// by columns, its (1,1) being 1.
pub fn table() -> Vec<[f32; 4]> {
    TABLE
        .chunks_exact(16)
        .map(|e| std::array::from_fn(|i| f32::from_le_bytes([e[i * 4], e[i * 4 + 1], e[i * 4 + 2], e[i * 4 + 3]])))
        .collect()
}

/// The table as a texture for the surface shader.
pub fn texture(gpu: &Gpu) -> wgpu::TextureView {
    // Half floats: the terms stay well inside their range because the
    // roughness stops at MIN_ALPHA.
    let texels: Vec<u16> = table().iter().flatten().map(|&v| super::env::f32_to_f16(v)).collect();
    let size = wgpu::Extent3d { width: SIZE as u32, height: SIZE as u32, depth_or_array_layers: 1 };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("softbox table"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&texels),
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8 * SIZE as u32), rows_per_image: Some(SIZE as u32) },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The inverse matrix of an entry, as the shader builds it.
pub fn inverse_of(entry: [f32; 4]) -> Mat3 {
    Mat3::from_cols(Vec3::new(entry[0], 0.0, entry[1]), Vec3::Y, Vec3::new(entry[2], 0.0, entry[3]))
}

/// What a rectangle light of `corners` (in order round it) gives a surface
/// at `p` facing `n` seen from `v`, through the lobe `minv` (the identity
/// for diffuse): the integral over the light of the lobe, times 2 pi. The
/// CPU twin of `ltc_integral` in sculpt_pbr.wgsl, for the tests. Light from
/// behind the rectangle's front (the side its corners turn anticlockwise
/// seen from) counts nothing.
pub fn integral(n: Vec3, v: Vec3, p: Vec3, minv: Mat3, corners: [Vec3; 4]) -> f32 {
    // A frame about the normal, with the view in its first plane (any
    // frame, seen straight on: the shader picks the same way).
    let mut t1 = v - n * v.dot(n);
    if t1.length_squared() < 1e-10 {
        t1 = if n.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
        t1 -= n * t1.dot(n);
    }
    let t1 = t1.normalize();
    let t2 = n.cross(t1);
    let to_frame = Mat3::from_cols(t1, t2, n).transpose();
    let m = minv * to_frame;
    let mut l: Vec<Vec3> = corners.iter().map(|&c| m * (c - p)).collect();
    // Only what is above the surface.
    l = clip_to_horizon(&l);
    if l.len() < 3 {
        return 0.0;
    }
    let l: Vec<Vec3> = l.iter().map(|v| v.normalize()).collect();
    let mut sum = 0.0;
    for i in 0..l.len() {
        let (a, b) = (l[i], l[(i + 1) % l.len()]);
        let cos = a.dot(b).clamp(-1.0, 1.0);
        let theta = cos.acos();
        let factor = if theta > 1e-4 { theta / theta.sin() } else { 1.0 };
        sum += a.cross(b).z * factor;
    }
    sum.max(0.0)
}

/// The part of a polygon above z = 0.
fn clip_to_horizon(points: &[Vec3]) -> Vec<Vec3> {
    let mut out = Vec::new();
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        if a.z > 0.0 {
            out.push(a);
        }
        if (a.z > 0.0) != (b.z > 0.0) {
            // Where the edge crosses the horizon (a direction, so the two
            // ends' weights need only be both positive).
            let (above, below) = if a.z > 0.0 { (a, b) } else { (b, a) };
            out.push(-below.z * above + above.z * below);
        }
    }
    out
}

// ---- fitting the table -------------------------------------------------------
//
// The paper's own procedure (its fitLTC.cpp), against this renderer's GGX.

/// GGX's highlight times the cosine, and how often `ggx_sample` draws `l`:
/// both for view `v` and light `l` in the surface's frame (normal +z).
fn ggx(v: Vec3, l: Vec3, alpha: f32) -> (f32, f32) {
    if v.z <= 0.0 {
        return (0.0, 0.0);
    }
    let lambda = |cos: f32| {
        if cos >= 1.0 {
            return 0.0;
        }
        let tan = (1.0 - cos * cos).max(0.0).sqrt() / cos;
        let a = 1.0 / (alpha * tan);
        0.5 * (-1.0 + (1.0 + 1.0 / (a * a)).sqrt())
    };
    let g2 = if l.z <= 0.0 { 0.0 } else { 1.0 / (1.0 + lambda(v.z) + lambda(l.z)) };
    let h = (v + l).normalize();
    let (sx, sy) = (h.x / h.z, h.y / h.z);
    let mut d = 1.0 / (1.0 + (sx * sx + sy * sy) / (alpha * alpha));
    d = d * d / (std::f32::consts::PI * alpha * alpha * h.z.powi(4));
    let pdf = (d * h.z / 4.0 / v.dot(h)).abs();
    (d * g2 / 4.0 / v.z, pdf)
}

/// A light direction GGX reflects `v` into, drawn as often as it does.
fn ggx_sample(v: Vec3, alpha: f32, u1: f32, u2: f32) -> Vec3 {
    let phi = std::f32::consts::TAU * u1;
    let r = alpha * (u2 / (1.0 - u2)).sqrt();
    let n = Vec3::new(r * phi.cos(), r * phi.sin(), 1.0).normalize();
    -v + 2.0 * n * n.dot(v)
}

/// A cosine lobe put through a matrix: the distribution being fitted.
#[derive(Clone, Copy)]
struct Lobe {
    magnitude: f32,
    m11: f32,
    m22: f32,
    m13: f32,
    x: Vec3,
    y: Vec3,
    z: Vec3,
    m: Mat3,
    inv_m: Mat3,
    det_m: f32,
}

impl Lobe {
    fn new() -> Lobe {
        let mut lobe = Lobe {
            magnitude: 1.0,
            m11: 1.0,
            m22: 1.0,
            m13: 0.0,
            x: Vec3::X,
            y: Vec3::Y,
            z: Vec3::Z,
            m: Mat3::IDENTITY,
            inv_m: Mat3::IDENTITY,
            det_m: 1.0,
        };
        lobe.update();
        lobe
    }

    fn update(&mut self) {
        let params = Mat3::from_cols(Vec3::new(self.m11, 0.0, 0.0), Vec3::new(0.0, self.m22, 0.0), Vec3::new(self.m13, 0.0, 1.0));
        self.m = Mat3::from_cols(self.x, self.y, self.z) * params;
        self.inv_m = self.m.inverse();
        self.det_m = self.m.determinant().abs();
    }

    fn eval(&self, l: Vec3) -> f32 {
        let original = (self.inv_m * l).normalize();
        let back = self.m * original;
        let len = back.length();
        let jacobian = self.det_m / (len * len * len);
        self.magnitude * original.z.max(0.0) / std::f32::consts::PI / jacobian
    }

    fn sample(&self, u1: f32, u2: f32) -> Vec3 {
        let theta = u1.sqrt().acos();
        let phi = std::f32::consts::TAU * u2;
        (self.m * Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos())).normalize()
    }

    /// Takes the three fitted numbers; an isotropic lobe has the first only.
    fn set(&mut self, params: [f32; 3], isotropic: bool) {
        let m11 = params[0].max(1e-7);
        (self.m11, self.m22, self.m13) = if isotropic { (m11, m11, 0.0) } else { (m11, params[1].max(1e-7), params[2]) };
        self.update();
    }
}

const SAMPLES: usize = 32;

/// How far the lobe is from GGX: the cube of the difference, integrated by
/// drawing from both and weighing each draw by both (multiple importance).
fn error(lobe: &Lobe, v: Vec3, alpha: f32) -> f32 {
    use rayon::prelude::*;
    let total: f64 = (0..SAMPLES)
        .into_par_iter()
        .map(|j| {
            let mut row = 0.0f64;
            for i in 0..SAMPLES {
                let (u1, u2) = ((i as f32 + 0.5) / SAMPLES as f32, (j as f32 + 0.5) / SAMPLES as f32);
                for l in [lobe.sample(u1, u2), ggx_sample(v, alpha, u1, u2)] {
                    let (brdf, pdf_brdf) = ggx(v, l, alpha);
                    let fitted = lobe.eval(l);
                    let pdf_lobe = fitted / lobe.magnitude;
                    let e = (brdf - fitted).abs() as f64;
                    row += e * e * e / (pdf_lobe + pdf_brdf) as f64;
                }
            }
            row
        })
        .sum();
    (total / (SAMPLES * SAMPLES) as f64) as f32
}

/// GGX's whole reflected amount for `v` and the way it mostly goes.
fn average(v: Vec3, alpha: f32) -> (f32, Vec3) {
    let (mut norm, mut dir) = (0.0f32, Vec3::ZERO);
    for j in 0..SAMPLES {
        for i in 0..SAMPLES {
            let (u1, u2) = ((i as f32 + 0.5) / SAMPLES as f32, (j as f32 + 0.5) / SAMPLES as f32);
            let l = ggx_sample(v, alpha, u1, u2);
            let (brdf, pdf) = ggx(v, l, alpha);
            if pdf > 0.0 {
                norm += brdf / pdf;
                dir += brdf / pdf * l;
            }
        }
    }
    dir.y = 0.0;
    (norm / (SAMPLES * SAMPLES) as f32, dir.normalize_or(Vec3::Z))
}

/// Nelder and Mead's downhill simplex: the three numbers near `start`
/// (first steps `delta`) that make `f` least.
fn nelder_mead(start: [f32; 3], delta: f32, tolerance: f32, iterations: usize, f: impl Fn([f32; 3]) -> f32) -> [f32; 3] {
    let mut s = [start; 4];
    for i in 1..4 {
        s[i][i - 1] += delta;
    }
    let mut fs: [f32; 4] = std::array::from_fn(|i| f(s[i]));
    let mut lo = 0;
    for _ in 0..iterations {
        lo = 0;
        let (mut hi, mut nh) = (0, 0);
        for i in 1..4 {
            if fs[i] < fs[lo] {
                lo = i;
            }
            if fs[i] > fs[hi] {
                nh = hi;
                hi = i;
            } else if fs[i] > fs[nh] {
                nh = i;
            }
        }
        let (a, b) = (fs[lo].abs(), fs[hi].abs());
        if 2.0 * (a - b).abs() < (a + b) * tolerance {
            break;
        }
        let mut o = [0.0f32; 3];
        for (i, p) in s.iter().enumerate() {
            if i != hi {
                for k in 0..3 {
                    o[k] += p[k] / 3.0;
                }
            }
        }
        let towards = |k: f32| -> [f32; 3] { std::array::from_fn(|d| o[d] + k * (o[d] - s[hi][d])) };
        let r = towards(1.0);
        let fr = f(r);
        if fr < fs[nh] {
            if fr < fs[lo] {
                let e = towards(2.0);
                let fe = f(e);
                if fe < fr {
                    (s[hi], fs[hi]) = (e, fe);
                    continue;
                }
            }
            (s[hi], fs[hi]) = (r, fr);
            continue;
        }
        let c = towards(-0.5);
        let fc = f(c);
        if fc < fs[hi] {
            (s[hi], fs[hi]) = (c, fc);
            continue;
        }
        for k in 0..4 {
            if k != lo {
                s[k] = std::array::from_fn(|d| s[lo][d] + 0.5 * (s[k][d] - s[lo][d]));
                fs[k] = f(s[k]);
            }
        }
    }
    s[lo]
}

/// Fits the whole table, as the paper does: from the roughest down, each
/// view angle starting from the last one's fit.
pub fn fit() -> Vec<[f32; 4]> {
    let n = SIZE;
    let mut matrices = vec![Mat3::IDENTITY; n * n];
    let mut lobe = Lobe::new();
    for a in (0..n).rev() {
        for t in 0..n {
            // Down the table, sqrt(1 - cos) of the view angle.
            let x = t as f32 / (n - 1) as f32;
            let theta = (1.0 - x * x).acos().min(1.57);
            let v = Vec3::new(theta.sin(), 0.0, theta.cos());
            let roughness = a as f32 / (n - 1) as f32;
            let alpha = (roughness * roughness).max(MIN_ALPHA);
            let (norm, towards) = average(v, alpha);
            lobe.magnitude = norm;
            let isotropic = t == 0;
            if isotropic {
                // Head on the lobe is round and points along the normal.
                (lobe.x, lobe.y, lobe.z) = (Vec3::X, Vec3::Y, Vec3::Z);
                (lobe.m11, lobe.m22) = if a == n - 1 {
                    (1.0, 1.0)
                } else {
                    let rougher = matrices[a + 1 + t * n];
                    (rougher.x_axis.x, rougher.y_axis.y)
                };
                lobe.m13 = 0.0;
            } else {
                (lobe.x, lobe.y, lobe.z) = (Vec3::new(towards.z, 0.0, -towards.x), Vec3::Y, towards);
            }
            lobe.update();
            let start = [lobe.m11, lobe.m22, lobe.m13];
            let best = nelder_mead(start, 0.05, 1e-5, 100, |p| {
                let mut tried = lobe;
                tried.set(p, isotropic);
                error(&tried, v, alpha)
            });
            lobe.set(best, isotropic);
            let mut m = lobe.m;
            // An isotropic GGX has none of these.
            m.x_axis.y = 0.0;
            m.y_axis.x = 0.0;
            m.z_axis.y = 0.0;
            m.y_axis.z = 0.0;
            matrices[a + t * n] = m;
        }
    }
    matrices
        .iter()
        .map(|m| {
            let inv = m.inverse();
            let inv = inv * (1.0 / inv.y_axis.y);
            [inv.x_axis.x, inv.x_axis.z, inv.z_axis.x, inv.z_axis.z]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `assets/ltc.bin`. Run once, by hand: it takes a few minutes.
    /// `cargo test --release ltc::tests::write_the_table -- --ignored`
    #[test]
    #[ignore]
    fn write_the_table() {
        let table = fit();
        let bytes: Vec<u8> = table.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ltc.bin"), bytes).unwrap();
    }

    /// The table entry for `roughness` and the cosine of the view angle,
    /// read between entries as the shader's filtering does.
    fn lookup(table: &[[f32; 4]], roughness: f32, nv: f32) -> [f32; 4] {
        let at = |u: f32| (u * (SIZE - 1) as f32).clamp(0.0, (SIZE - 1) as f32);
        let (x, y) = (at(roughness), at((1.0 - nv).max(0.0).sqrt()));
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(SIZE - 1), (y0 + 1).min(SIZE - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        std::array::from_fn(|k| {
            let e = |x: usize, y: usize| table[x + y * SIZE][k];
            (e(x0, y0) * (1.0 - fx) + e(x1, y0) * fx) * (1.0 - fy) + (e(x0, y1) * (1.0 - fx) + e(x1, y1) * fx) * fy
        })
    }

    /// GGX times the cosine integrated over a rectangle light by brute
    /// force: a fine grid over its face, each cell weighed by its solid angle.
    fn brute_force(n: Vec3, v: Vec3, p: Vec3, alpha: f32, corners: [Vec3; 4]) -> f32 {
        let t1 = (v - n * v.dot(n)).normalize_or(n.any_orthonormal_vector());
        let frame = Mat3::from_cols(t1, n.cross(t1), n).transpose();
        let v_local = frame * v;
        let (e1, e2) = (corners[1] - corners[0], corners[3] - corners[0]);
        let normal = e1.cross(e2);
        let area = normal.length();
        let steps = 400;
        let mut sum = 0.0f64;
        for j in 0..steps {
            for i in 0..steps {
                let q = corners[0] + e1 * ((i as f32 + 0.5) / steps as f32) + e2 * ((j as f32 + 0.5) / steps as f32);
                let d = q - p;
                let dist2 = d.length_squared();
                let l = d / dist2.sqrt();
                // Solid angle of the cell, seen from p. A light shines on
                // the side its corners' winding normal points away from.
                let facing = l.dot(normal / area);
                if facing <= 0.0 {
                    continue;
                }
                let solid = area / (steps * steps) as f32 * facing / dist2;
                let (brdf_cos, _) = ggx(v_local, frame * l, alpha);
                sum += (brdf_cos * solid) as f64;
            }
        }
        sum as f32
    }

    /// A square light `size` across, `distance` away from the origin toward
    /// `toward`, facing back at it.
    fn square(toward: Vec3, distance: f32, size: f32) -> [Vec3; 4] {
        let d = toward.normalize();
        let centre = d * distance;
        let right = Vec3::Y.cross(d).normalize_or(Vec3::X);
        let up = d.cross(right);
        let (r, u) = (right * size * 0.5, up * size * 0.5);
        // The order the shader's lights have: anticlockwise round the
        // direction toward them, which is what makes the integral positive
        // from in front and nothing from behind.
        [centre - r - u, centre + r - u, centre + r + u, centre - r + u]
    }

    #[test]
    fn diffuse_light_from_a_rectangle_is_its_form_factor() {
        // A small light straight above: about its solid angle times pi... in
        // this form, 2 pi times the form factor, which for a small square
        // straight above is its solid angle over pi.
        let corners = square(Vec3::Z, 10.0, 0.5);
        let got = integral(Vec3::Z, Vec3::Z, Vec3::ZERO, Mat3::IDENTITY, corners);
        let solid = 0.25 / 100.0;
        assert!((got - 2.0 * solid).abs() < 0.02 * 2.0 * solid, "{got}");
        // Behind the surface: nothing.
        let below = square(-Vec3::Z, 10.0, 0.5);
        assert_eq!(integral(Vec3::Z, Vec3::Z, Vec3::ZERO, Mat3::IDENTITY, below), 0.0);
        // Half below the horizon: about half of it.
        let half = square(Vec3::X, 10.0, 2.0);
        let full = square(Vec3::new(1.0, 0.0, 0.2), 10.0, 2.0);
        let (h, f) = (
            integral(Vec3::Z, Vec3::Z, Vec3::ZERO, Mat3::IDENTITY, half),
            integral(Vec3::Z, Vec3::Z, Vec3::ZERO, Mat3::IDENTITY, full),
        );
        assert!(h > 0.0 && h < f, "{h} {f}");
    }

    #[test]
    fn the_fitted_highlight_matches_ggx_over_a_softbox() {
        let table = table();
        assert_eq!(table.len(), SIZE * SIZE);
        assert!(table.iter().flatten().all(|v| v.is_finite()));
        let n = Vec3::Z;
        let cases = [
            // roughness, view angle (degrees), light toward, light size
            (0.5, 0.0, Vec3::new(0.0, 0.0, 1.0), 3.0),
            (0.5, 45.0, Vec3::new(-1.0, 0.0, 1.0), 3.0),
            (0.8, 30.0, Vec3::new(-0.5, 0.3, 1.0), 4.0),
            (0.35, 60.0, Vec3::new(-1.7, 0.0, 1.0), 4.0),
            (0.6, 75.0, Vec3::new(-2.0, 0.5, 1.0), 6.0),
        ];
        for (roughness, angle, toward, size) in cases {
            let (roughness, size): (f32, f32) = (roughness, size);
            let (s, c) = (angle as f32).to_radians().sin_cos();
            let v = Vec3::new(s, 0.0, c);
            let corners = square(toward, 10.0, size);
            let alpha = (roughness * roughness).max(MIN_ALPHA);
            let exact = brute_force(n, v, Vec3::ZERO, alpha, corners);
            // The lobe's share of the light times GGX's whole reflected amount.
            let minv = inverse_of(lookup(&table, roughness, c));
            let (norm, _) = average(v, alpha);
            let fitted = integral(n, v, Vec3::ZERO, minv, corners) / std::f32::consts::TAU * norm;
            // Within a percent or two seen square on; LTC's usual tenth at a
            // grazing view.
            let off = (fitted - exact).abs() / exact.max(1e-6);
            assert!(off < 0.12, "roughness {roughness}, {angle} degrees: fitted {fitted}, exact {exact}");
        }
    }
}
