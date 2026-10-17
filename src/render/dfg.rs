//! The split-sum table for reflections: how much light a glossy surface
//! sends back toward the eye, all of its highlight's lobe added up, for each
//! angle it is seen at and each roughness. Worked out once on the processor
//! when the renderer starts (a few milliseconds) and read by the surface
//! shader (`dfg_lookup` in sculpt_pbr.wgsl).
//!
//! Each texel holds two numbers, the way Filament keeps them:
//!
//! - **y**, the whole of what a perfectly white mirror of that roughness
//!   reflects (fresnel 1 everywhere). It is below 1 for a rough surface:
//!   light that bounces twice between its microscopic facets is lost to the
//!   single-bounce model, which is why rough metal used to go dark.
//! - **x**, the part of that which grazing-angle fresnel adds.
//!
//! A surface of reflectance `f0` then reflects `x + (y - x) f0` of its
//! surroundings, and the lost second bounce comes back by multiplying its
//! highlights by `1 + f0 (1 / y - 1)` (Fdez-Agüera's energy compensation),
//! so a white metal reflects everything at every roughness.

use super::Gpu;

/// Texels along each side: view angle across, roughness down.
pub const SIZE: usize = 32;
const SAMPLES: u32 = 1024;

/// Height-correlated Smith visibility for GGX, the shader's `v_smith`; `a` is
/// the linear roughness (perceptual roughness squared).
fn visibility(nv: f64, nl: f64, a: f64) -> f64 {
    let a2 = a * a;
    let gv = nl * (nv * nv * (1.0 - a2) + a2).sqrt();
    let gl = nv * (nl * nl * (1.0 - a2) + a2).sqrt();
    0.5 / (gv + gl).max(1e-12)
}

/// The `i`th of `n` points of the Hammersley set.
fn hammersley(i: u32, n: u32) -> (f64, f64) {
    (i as f64 / n as f64, i.reverse_bits() as f64 / 4_294_967_296.0)
}

/// (x, y) for a surface seen at `nv` (the cosine of the view angle) with
/// perceptual roughness `roughness`.
pub fn entry(nv: f64, roughness: f64) -> [f64; 2] {
    let a = roughness * roughness;
    let nv = nv.clamp(1e-4, 1.0);
    let v = [(1.0 - nv * nv).sqrt(), 0.0, nv];
    let (mut x, mut y) = (0.0, 0.0);
    for i in 0..SAMPLES {
        let (u1, u2) = hammersley(i, SAMPLES);
        // A microfacet normal, drawn as often as GGX has them.
        let phi = std::f64::consts::TAU * u1;
        let cos_t = ((1.0 - u2) / (1.0 + (a * a - 1.0) * u2)).sqrt();
        let sin_t = (1.0 - cos_t * cos_t).sqrt();
        let h = [sin_t * phi.cos(), sin_t * phi.sin(), cos_t];
        let vh = v[0] * h[0] + v[1] * h[1] + v[2] * h[2];
        let l = [2.0 * vh * h[0] - v[0], 2.0 * vh * h[1] - v[1], 2.0 * vh * h[2] - v[2]];
        let (nl, nh) = (l[2], h[2]);
        if nl <= 0.0 || vh <= 0.0 {
            continue;
        }
        // The highlight times the cosine over how often this direction was
        // drawn: the distribution itself cancels out.
        let weight = visibility(nv, nl, a) * nl * vh / nh;
        let fc = (1.0 - vh).powi(5);
        x += weight * fc;
        y += weight;
    }
    let scale = 4.0 / SAMPLES as f64;
    [x * scale, y * scale]
}

/// The whole table, row by row: roughness down the rows, view angle across,
/// each taken at its texel's centre.
pub fn table() -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(SIZE * SIZE);
    for row in 0..SIZE {
        let roughness = (row as f64 + 0.5) / SIZE as f64;
        for column in 0..SIZE {
            let nv = (column as f64 + 0.5) / SIZE as f64;
            let [x, y] = entry(nv, roughness);
            out.push([x as f32, y as f32]);
        }
    }
    out
}

/// The table as a texture for the surface shader.
pub fn texture(gpu: &Gpu) -> wgpu::TextureView {
    let texels: Vec<u16> =
        table().iter().flat_map(|&[x, y]| [x, y, 0.0, 1.0]).map(super::env::f32_to_f16).collect();
    let size = wgpu::Extent3d { width: SIZE as u32, height: SIZE as u32, depth_or_array_layers: 1 };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("reflection table"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What the shader makes of an entry for reflectance `f0`: the share of
    /// the surroundings reflected, energy compensation included.
    fn reflected(e: [f64; 2], f0: f64) -> f64 {
        let single = e[0] + (e[1] - e[0]) * f0;
        single * (1.0 + f0 * (1.0 / e[1] - 1.0))
    }

    #[test]
    fn a_white_metal_reflects_everything_at_every_roughness() {
        // The furnace test: in an even white world a mirror with fresnel 1
        // is as bright as the world, however rough it is and however it is
        // seen. Without the compensation it loses up to half of it.
        for roughness in [0.05, 0.25, 0.5, 0.75, 1.0] {
            for nv in [0.1, 0.5, 0.9] {
                let e = entry(nv, roughness);
                assert!((reflected(e, 1.0) - 1.0).abs() < 1e-6, "{roughness} {nv}: {e:?}");
            }
        }
        let rough = entry(0.5, 1.0);
        assert!(rough[1] < 0.7, "single scattering alone loses a good share at full roughness: {rough:?}");
    }

    #[test]
    fn a_smooth_surface_is_a_plain_mirror() {
        // Nearly smooth: the whole lobe is one direction, so y is about 1 and
        // x is the grazing fresnel of that one direction.
        let e = entry(1.0, 0.02);
        assert!((e[1] - 1.0).abs() < 0.02, "{e:?}");
        assert!(e[0] < 1e-3, "{e:?}");
        let grazing = entry(0.2, 0.02);
        assert!((grazing[0] - 0.8f64.powi(5)).abs() < 0.02, "{grazing:?}");
    }

    #[test]
    fn the_table_holds_sensible_numbers_everywhere() {
        let t = table();
        assert_eq!(t.len(), SIZE * SIZE);
        for &[x, y] in &t {
            assert!(x.is_finite() && y.is_finite());
            assert!((0.0..=1.001).contains(&y), "{y}");
            assert!(x >= 0.0 && x <= y + 1e-6, "{x} {y}");
        }
        // Rougher reflects less of a white mirror, before compensation.
        let at = |row: usize, column: usize| t[row * SIZE + column][1];
        assert!(at(SIZE - 1, SIZE / 2) < at(2, SIZE / 2));
    }

    #[test]
    fn a_dielectric_reflects_about_its_reflectance_head_on() {
        // Plastic (f0 = 0.04) seen square on reflects about 4%; at a
        // grazing angle far more.
        let head_on = reflected(entry(0.98, 0.3), 0.04);
        assert!((head_on - 0.04).abs() < 0.01, "{head_on}");
        let grazing = reflected(entry(0.1, 0.3), 0.04);
        assert!(grazing > 0.15, "{grazing}");
    }
}
