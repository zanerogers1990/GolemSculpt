//! Drawing the low model's triangles into the picture, texel by texel.
//!
//! A texel belongs to a triangle when its centre does: the rule graphics
//! cards draw by, and the one that makes a baked texel stand for the very
//! point the renderer will look it up at. The corners are put in texel
//! space from their UVs (U to the right, V up, so V = 1 is the picture's top
//! row) and snapped to 1/256 of a texel; the inside tests are then sums of
//! whole numbers, worked exactly. That matters on the edge two triangles
//! share: each tests it with the same sum, signs turned, so a centre lying
//! exactly on it is settled by the top-left rule (as if nudged a hair right
//! and down) and goes to exactly one of the two, never both and never
//! neither. The same holds where many triangles meet at a corner.
//!
//! A triangle whose UVs are mirrored (wound the other way round in the
//! picture, as a mirrored island is) is drawn the same way, its corners
//! taken in the other order for the tests; the weights it hands out are for
//! its own corners in their own order.
//!
//! The picture is cut into bands of `BAND` rows, each with the list of
//! triangles that reach it, so that bands can be baked in parallel, each
//! thread writing only its own rows.

use glam::Vec2;

/// Rows to a band.
pub(super) const BAND: u32 = 4;

/// Steps to a texel that the corners are snapped to.
const SUB: i64 = 256;

/// A triangle with a corner further than this from the unit square (in
/// whole squares) is not drawn: no unwrap puts one there, and its sums would
/// outgrow the whole numbers they are worked in.
const FAR: f32 = 64.0;

/// A triangle ready to draw.
struct Tri {
    /// Its index among the target's triangles.
    index: u32,
    /// The corners in texel space, in steps of 1/`SUB`, in the order that
    /// makes the area positive.
    at: [[i64; 2]; 3],
    /// Which of the triangle's own corners each of `at` is.
    corner: [usize; 3],
    /// Twice the area, in steps squared: always positive.
    area: i64,
    /// Per edge `at[k] -> at[k + 1]`: 0 if a centre exactly on it is inside
    /// (a top or left edge), -1 if not.
    bias: [i64; 3],
    /// The texels whose centres may be inside: columns and rows, inclusive.
    x: [u32; 2],
    y: [u32; 2],
}

/// The target's triangles drawn into a square picture of `size` texels.
pub(super) struct Raster {
    size: u32,
    tris: Vec<Tri>,
    /// For each band, the triangles (indices into `tris`) that reach it.
    bands: Vec<Vec<u32>>,
}

/// How far `p` is to the left of the edge `a -> b`, times the edge's length.
#[inline]
fn edge(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i64 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Whether a centre exactly on the edge `a -> b` of a triangle of positive
/// area is inside it: whether it would be if nudged a hair right and down
/// (rows count downward), which takes in left and top edges.
#[inline]
fn top_left(a: [i64; 2], b: [i64; 2]) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    dy < 0 || (dy == 0 && dx > 0)
}

impl Raster {
    /// `uvs` has one entry per triangle corner.
    pub(super) fn new(uvs: &[Vec2], size: u32) -> Raster {
        let scale = size as f64 * SUB as f64;
        let snap = |uv: Vec2| [(uv.x as f64 * scale).round() as i64, ((1.0 - uv.y as f64) * scale).round() as i64];
        let last = size as i64 - 1;
        // The first and last texel whose centre is at or past `lo`, at or before `hi`.
        let texels = |lo: i64, hi: i64| -> Option<[u32; 2]> {
            let first = (lo - SUB / 2 + SUB - 1).div_euclid(SUB).max(0);
            let end = (hi - SUB / 2).div_euclid(SUB).min(last);
            (first <= end).then_some([first as u32, end as u32])
        };
        let mut tris = Vec::with_capacity(uvs.len() / 3);
        for (index, uv) in uvs.chunks_exact(3).enumerate() {
            if uv.iter().any(|c| !c.is_finite() || c.abs().max_element() > FAR) {
                continue;
            }
            let mut at = [snap(uv[0]), snap(uv[1]), snap(uv[2])];
            let mut corner = [0, 1, 2];
            let mut area = edge(at[0], at[1], at[2]);
            if area == 0 {
                continue;
            }
            if area < 0 {
                at.swap(1, 2);
                corner.swap(1, 2);
                area = -area;
            }
            let bias = [0, 1, 2].map(|k| if top_left(at[k], at[(k + 1) % 3]) { 0 } else { -1 });
            let lo = |axis: usize| at.iter().map(|p| p[axis]).min().unwrap_or(0);
            let hi = |axis: usize| at.iter().map(|p| p[axis]).max().unwrap_or(0);
            let (Some(x), Some(y)) = (texels(lo(0), hi(0)), texels(lo(1), hi(1))) else { continue };
            tris.push(Tri { index: index as u32, at, corner, area, bias, x, y });
        }
        let mut bands = vec![Vec::new(); size.div_ceil(BAND) as usize];
        for (i, tri) in tris.iter().enumerate() {
            for band in tri.y[0] / BAND..=tri.y[1] / BAND {
                bands[band as usize].push(i as u32);
            }
        }
        Raster { size, tris, bands }
    }

    pub(super) fn band_count(&self) -> usize {
        self.bands.len()
    }

    /// Calls `f(triangle, x, y, weights)` for every texel of band `band`
    /// whose centre a triangle covers, with the weights of the triangle's
    /// three corners there. Asks `stop` before each row of each triangle and
    /// returns at once when it says so. Triangles are taken in order, so
    /// where UVs overlap the later triangle comes last, the same every time.
    pub(super) fn band(&self, band: usize, mut f: impl FnMut(u32, u32, u32, [f32; 3]), stop: impl Fn() -> bool) {
        let rows = [band as u32 * BAND, ((band as u32 + 1) * BAND).min(self.size) - 1];
        for &i in &self.bands[band] {
            let tri = &self.tris[i as usize];
            let area = tri.area as f64;
            let [a, b, c] = tri.at;
            for y in tri.y[0].max(rows[0])..=tri.y[1].min(rows[1]) {
                if stop() {
                    return;
                }
                let py = y as i64 * SUB + SUB / 2;
                for x in tri.x[0]..=tri.x[1] {
                    let p = [x as i64 * SUB + SUB / 2, py];
                    let e = [edge(b, c, p), edge(c, a, p), edge(a, b, p)];
                    // Edge `b -> c` is the one across from `a`, and so on.
                    if e[0] + tri.bias[1] < 0 || e[1] + tri.bias[2] < 0 || e[2] + tri.bias[0] < 0 {
                        continue;
                    }
                    let mut weights = [0.0f32; 3];
                    for k in 0..3 {
                        weights[tri.corner[k]] = (e[k] as f64 / area) as f32;
                    }
                    f(tri.index, x, y, weights);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many times each texel is covered.
    fn coverage(uvs: &[Vec2], size: u32) -> Vec<u32> {
        let raster = Raster::new(uvs, size);
        let mut hits = vec![0u32; (size * size) as usize];
        for band in 0..raster.band_count() {
            raster.band(
                band,
                |_, x, y, w| {
                    assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5, "weights add up to one");
                    assert!(w.iter().all(|&w| w >= -1e-6), "and none is below nothing");
                    hits[(y * size + x) as usize] += 1;
                },
                || false,
            );
        }
        hits
    }

    #[test]
    fn two_triangles_making_the_square_cover_every_texel_once() {
        let (a, b, c, d) = (Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0));
        let hits = coverage(&[a, b, c, a, c, d], 64);
        assert!(hits.iter().all(|&n| n == 1));
    }

    #[test]
    fn a_fan_with_corners_on_texel_centres_covers_each_texel_once() {
        // Every corner and the middle sit exactly on texel centres, so many
        // centres lie exactly on edges and on the shared corner; each still
        // goes to exactly one triangle, whichever way round a triangle is wound.
        let size = 32;
        let centre = |x: u32, y: u32| Vec2::new((x as f32 + 0.5) / size as f32, 1.0 - (y as f32 + 0.5) / size as f32);
        let middle = centre(13, 17);
        let ring =
            [centre(3, 3), centre(20, 2), centre(29, 9), centre(29, 29), centre(13, 28), centre(2, 25), centre(2, 12)];
        let mut uvs = Vec::new();
        for k in 0..ring.len() {
            let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
            // Half the triangles one way round, half the other.
            if k % 2 == 0 { uvs.extend([middle, p, q]) } else { uvs.extend([middle, q, p]) }
        }
        let hits = coverage(&uvs, size);
        assert!(hits.iter().all(|&n| n <= 1), "no texel twice");
        // Inside the ring every texel is covered: the middle row, across.
        let row = 17;
        let covered: Vec<u32> = (0..size).filter(|&x| hits[(row * size + x) as usize] == 1).collect();
        assert_eq!(covered.first(), Some(&2));
        assert!(covered.windows(2).all(|w| w[1] == w[0] + 1), "no gap: {covered:?}");
    }

    #[test]
    fn the_weights_find_the_point_under_the_texel() {
        let size = 64;
        let uvs = [Vec2::new(0.1, 0.1), Vec2::new(0.9, 0.2), Vec2::new(0.3, 0.95)];
        let raster = Raster::new(&uvs, size);
        for band in 0..raster.band_count() {
            raster.band(
                band,
                |_, x, y, w| {
                    let uv = uvs[0] * w[0] + uvs[1] * w[1] + uvs[2] * w[2];
                    let want = Vec2::new((x as f32 + 0.5) / size as f32, 1.0 - (y as f32 + 0.5) / size as f32);
                    assert!(uv.distance(want) < 1e-3, "{uv} against {want}");
                },
                || false,
            );
        }
    }
}
