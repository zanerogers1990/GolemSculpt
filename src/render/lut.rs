//! Colour grading by a 3D look-up table: the `.cube` files colourists and
//! photo and video tools trade (Adobe's format). Each picture colour, as the
//! screen shows it (sRGB-encoded, 0..1), is looked up in an N x N x N grid
//! of the colours it becomes, between the grid's points. Applied last, after
//! the tone curve (post.wgsl, `fs_composite`), with a strength to fade it.

use super::Gpu;

/// The largest table read: 64 points a side is as fine as any in common use.
const MAX_SIZE: usize = 128;

/// A table as read from a file: `size` points along each side, red running
/// fastest, then green, then blue (the order a .cube file lists them).
#[derive(Clone, Debug, PartialEq)]
pub struct Lut {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

impl Lut {
    /// The table that changes nothing, at its smallest.
    pub fn identity() -> Lut {
        let mut data = Vec::with_capacity(8);
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    data.push([r as f32, g as f32, b as f32]);
                }
            }
        }
        Lut { size: 2, data }
    }

    /// Reads a .cube file's text.
    pub fn parse(text: &str) -> Result<Lut, String> {
        let mut size = None;
        let mut data = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut words = line.split_whitespace();
            let first = words.next().unwrap_or("");
            match first.to_ascii_uppercase().as_str() {
                "TITLE" => {}
                "LUT_1D_SIZE" => return Err("This is a 1D table; only 3D .cube tables are read.".into()),
                "LUT_3D_SIZE" => {
                    let n: usize = words.next().and_then(|w| w.parse().ok()).ok_or("The table's size does not read.")?;
                    if !(2..=MAX_SIZE).contains(&n) {
                        return Err(format!("A table {n} points a side is not one this reads (2 to {MAX_SIZE})."));
                    }
                    size = Some(n);
                }
                "DOMAIN_MIN" | "DOMAIN_MAX" => {
                    let want = if first.eq_ignore_ascii_case("DOMAIN_MIN") { 0.0 } else { 1.0 };
                    let values: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                    if values.len() != 3 || values.iter().any(|v| (v - want).abs() > 1e-4) {
                        return Err("Only tables over the usual 0 to 1 range are read.".into());
                    }
                }
                // A keyword of some other tool's: passed over.
                _ if first.parse::<f32>().is_err() => {}
                _ => {
                    let values: Vec<f32> = line.split_whitespace().filter_map(|w| w.parse().ok()).collect();
                    match values[..] {
                        [r, g, b] if r.is_finite() && g.is_finite() && b.is_finite() => data.push([r, g, b]),
                        _ => return Err(format!("A line of the table does not read: {line}")),
                    }
                }
            }
        }
        let size = size.ok_or("No LUT_3D_SIZE line: this is not a 3D .cube table.")?;
        if data.len() != size * size * size {
            return Err(format!("The table should have {} colours and has {}.", size * size * size, data.len()));
        }
        Ok(Lut { size, data })
    }

    /// Reads a .cube file.
    pub fn load(path: &std::path::Path) -> Result<Lut, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("Could not read file: {e}"))?;
        Lut::parse(&text)
    }

    /// The colour `rgb` (0..1, as shown) becomes, between the grid's points:
    /// the CPU twin of the shader's lookup, for the tests.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let at = |c: f32| c.clamp(0.0, 1.0) * (n - 1) as f32;
        let (x, y, z) = (at(rgb[0]), at(rgb[1]), at(rgb[2]));
        let (x0, y0, z0) = (x.floor() as usize, y.floor() as usize, z.floor() as usize);
        let (x1, y1, z1) = ((x0 + 1).min(n - 1), (y0 + 1).min(n - 1), (z0 + 1).min(n - 1));
        let (fx, fy, fz) = (x - x0 as f32, y - y0 as f32, z - z0 as f32);
        let e = |r: usize, g: usize, b: usize| self.data[r + g * n + b * n * n];
        std::array::from_fn(|k| {
            let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let c00 = lerp(e(x0, y0, z0)[k], e(x1, y0, z0)[k], fx);
            let c10 = lerp(e(x0, y1, z0)[k], e(x1, y1, z0)[k], fx);
            let c01 = lerp(e(x0, y0, z1)[k], e(x1, y0, z1)[k], fx);
            let c11 = lerp(e(x0, y1, z1)[k], e(x1, y1, z1)[k], fx);
            lerp(lerp(c00, c10, fy), lerp(c01, c11, fy), fz)
        })
    }
}

/// The table as a 3D texture for the post shader.
pub fn texture(gpu: &Gpu, lut: &Lut) -> wgpu::TextureView {
    let texels: Vec<u16> =
        lut.data.iter().flat_map(|&[r, g, b]| [r, g, b, 1.0]).map(super::env::f32_to_f16).collect();
    let side = lut.size as u32;
    let size = wgpu::Extent3d { width: side, height: side, depth_or_array_layers: side };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("colour table"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
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
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(8 * side), rows_per_image: Some(side) },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table of `n` a side that does `f` to every colour, as a .cube text.
    fn cube(n: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> String {
        let mut text = format!("# made for a test\nTITLE \"test\"\nLUT_3D_SIZE {n}\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n");
        let step = |i: usize| i as f32 / (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let [x, y, z] = f([step(r), step(g), step(b)]);
                    text += &format!("{x:.6} {y:.6} {z:.6}\n");
                }
            }
        }
        text
    }

    #[test]
    fn a_table_reads_and_looks_up_between_its_points() {
        // Warmer: red up, blue down, by amounts that vary smoothly.
        let warm = |c: [f32; 3]| [(c[0] * 1.1).min(1.0), c[1], c[2] * 0.8];
        let lut = Lut::parse(&cube(17, warm)).unwrap();
        assert_eq!((lut.size, lut.data.len()), (17, 17 * 17 * 17));
        for c in [[0.0, 0.0, 0.0], [0.5, 0.25, 0.75], [0.33, 0.66, 0.2], [1.0, 1.0, 1.0]] {
            let (got, want) = (lut.apply(c), warm(c));
            for k in 0..3 {
                assert!((got[k] - want[k]).abs() < 1e-3, "{c:?}: {got:?} {want:?}");
            }
        }
        let same = Lut::identity();
        assert_eq!(same.apply([0.2, 0.4, 0.6]), [0.2, 0.4, 0.6]);
    }

    #[test]
    fn a_file_that_is_not_a_3d_table_says_why() {
        assert!(Lut::parse("LUT_1D_SIZE 1024\n0 0 0\n").unwrap_err().contains("1D"));
        assert!(Lut::parse("0.1 0.2 0.3\n").unwrap_err().contains("LUT_3D_SIZE"));
        let short = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n";
        assert!(Lut::parse(short).unwrap_err().contains("should have 8"));
        let domain = "LUT_3D_SIZE 2\nDOMAIN_MAX 2 2 2\n";
        assert!(Lut::parse(domain).unwrap_err().contains("0 to 1"));
        assert!(Lut::parse("LUT_3D_SIZE 1\n").is_err());
        assert!(Lut::parse(&format!("LUT_3D_SIZE 2\n{}", "0 0 x\n".repeat(8))).is_err());
    }
}
