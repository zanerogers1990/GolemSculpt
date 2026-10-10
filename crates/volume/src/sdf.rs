//! Signed distance fields sampled on a lattice: making one from a mesh, and
//! making a mesh from one the caller filled in itself.
//!
//! A sampled field is a plain `Vec<f32>` with one value per lattice sample,
//! laid out x fastest and z slowest ([`Grid::index`]), negative inside. This
//! is the form for tools whose solid is a formula rather than a mesh (a tube
//! swept along a curve, a skin over spheres), and for anything that wants to
//! blend fields before meshing them. A mesh that only needs remeshing or
//! combining with another does not come through here: scan conversion
//! ([`mod@crate::remesh`], [`mod@crate::boolean`]) is exact where sampling rounds, and
//! never fills a whole lattice.
//!
//! A field need only be accurate near its surface. Extraction reads a value
//! for two things: its sign, and, at the corners of a cell the surface passes
//! through, how far along each edge the crossing is. So a sample no such cell
//! touches may hold [`FAR`] with the right sign and nothing more. A sample
//! exactly on the surface is the one trap: an inside distance of zero is
//! `-0.0`, which does not read as negative. Floor inside magnitudes at
//! [`MIN_MAGNITUDE`].

use glam::Vec3;
use rayon::prelude::*;

use crate::bvh::TriangleBvh;
use crate::extract::{self, CellSource, ExtractOptions, Hermite, Surface};
use crate::grid::Grid;
use crate::parallel;
use crate::scan::Scan;
use crate::topology::{self, EDGE_A, EDGE_B};

/// Stands in for the distance at a sample far from the surface. Only ever
/// read for its sign, and safe to negate.
pub const FAR: f32 = 1e6;

/// Smallest distance magnitude a field should carry, so the sign of a stored
/// value is always readable. Far below any cell size.
pub const MIN_MAGNITUDE: f32 = 1e-20;

/// A caller's sampled field, read as if it sat inside a lattice one sample
/// larger on every side whose extra samples are all outside. A field that
/// reaches the edge of its own lattice is thereby closed off flat there
/// instead of being left open.
struct SampledField<'a> {
    values: &'a [f32],
    grid: Grid,
    /// Inside bits of the larger lattice, one row of whole words per (j, k).
    bits: Vec<u64>,
    words: usize,
}

impl<'a> SampledField<'a> {
    fn new(values: &'a [f32], grid: &Grid) -> SampledField<'a> {
        let [nx, ny, nz] = grid.n;
        let words = (nx + 2).div_ceil(64);
        let mut bits = vec![0u64; words * (ny + 2) * (nz + 2)];
        bits.par_chunks_mut(words).enumerate().for_each(|(row, out)| {
            let (j, k) = (row % (ny + 2), row / (ny + 2));
            if j == 0 || k == 0 || j > ny || k > nz {
                return;
            }
            let from = grid.index(0, j - 1, k - 1);
            for (i, &v) in values[from..from + nx].iter().enumerate() {
                out[(i + 1) >> 6] |= ((v < 0.0) as u64) << ((i + 1) & 63);
            }
        });
        SampledField { values, grid: *grid, bits, words }
    }

    /// The value at a sample of the larger lattice.
    #[inline]
    fn value(&self, at: [usize; 3]) -> f32 {
        let n = self.grid.n;
        if (0..3).any(|a| at[a] == 0 || at[a] > n[a]) {
            return FAR;
        }
        self.values[self.grid.index(at[0] - 1, at[1] - 1, at[2] - 1)]
    }

    fn corners(&self, cell: [usize; 3]) -> [f32; 8] {
        std::array::from_fn(|c| self.value([cell[0] + (c & 1), cell[1] + (c >> 1 & 1), cell[2] + (c >> 2 & 1)]))
    }
}

/// Gradient of the trilinear interpolant of a cell's eight corner values at a
/// point in the cell's own coordinates, normalised. For a distance field that
/// is the surface normal, and negative-inside makes it point outward.
///
/// From this cell's corners alone, not a central difference across cells: a
/// narrow-band field only promises real distances at the corners of cells the
/// surface passes through, and the neighbours may hold nothing but [`FAR`].
fn trilinear_gradient(corner: &[f32; 8], p: Vec3) -> Vec3 {
    let mut g = Vec3::ZERO;
    for (c, &value) in corner.iter().enumerate() {
        let high = [c & 1 != 0, c >> 1 & 1 != 0, c >> 2 & 1 != 0];
        let weight = |a: usize| if high[a] { p[a] } else { 1.0 - p[a] };
        let sign = |a: usize| if high[a] { 1.0 } else { -1.0 };
        let slope = [sign(0) * weight(1) * weight(2), weight(0) * sign(1) * weight(2), weight(0) * weight(1) * sign(2)];
        g += value * Vec3::from(slope);
    }
    // Scaled first: with a far value at one corner the squared length would
    // otherwise overflow.
    (g / g.abs().max_element().max(f32::MIN_POSITIVE)).normalize_or_zero()
}

impl CellSource for SampledField<'_> {
    fn row(&self, j: usize, k: usize, out: &mut [u64], _scratch: &mut Vec<u64>) -> bool {
        let from = (j + k * (self.grid.n[1] + 2)) * self.words;
        out.copy_from_slice(&self.bits[from..from + self.words]);
        out.iter().any(|&w| w != 0)
    }

    fn hermite(&self, cell: [usize; 3], mask: u8, out: &mut Hermite) {
        let corner = self.corners(cell);
        for e in (0..12).filter(|&e| topology::edge_crosses(mask, e)) {
            let (va, vb) = (corner[EDGE_A[e]], corner[EDGE_B[e]]);
            let t = va / (va - vb);
            let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.5 };
            let mut p = Vec3::new((EDGE_A[e] & 1) as f32, (EDGE_A[e] >> 1 & 1) as f32, (EDGE_A[e] >> 2 & 1) as f32);
            p[e / 4] = t;
            out.t[e] = t;
            out.normal[e] = trilinear_gradient(&corner, p);
        }
    }

    fn corner_values(&self, cell: [usize; 3], _mask: u8) -> [f32; 8] {
        self.corners(cell)
    }
}

/// Extracts the surface of a sampled field: `values` on `grid`, negative
/// inside. With `mirror`, the result is exactly symmetric across those
/// coordinate planes whatever the field: its half on the positive side of
/// each plane is kept and reflected onto the other. ([`symmetrize`] is for
/// taking both halves into account instead: do that first.)
pub fn surface_from_sdf(values: &[f32], grid: &Grid, mirror: [bool; 3]) -> Result<Surface, String> {
    if values.len() != grid.sample_count() || values.is_empty() {
        return Err("The field does not match its grid.".into());
    }
    let source = SampledField::new(values, grid);
    extract::extract(&source, &grid.grown(), &ExtractOptions { mirror, ..ExtractOptions::default() })
}

/// Makes a sampled field exactly symmetric across the coordinate plane of
/// `axis`: each mirrored pair of samples takes the smaller of the two (their
/// union). `grid` must be its own mirror image there ([`Grid::mirrored`]).
///
/// Evaluating a formula at two mirrored points is only symmetric to rounding,
/// and a sample that lands on the surface can then read +0 on one side and a
/// hair below zero on the other: one cell meshed two different ways.
pub fn symmetrize(values: &mut [f32], grid: &Grid, axis: usize) -> Result<(), String> {
    let n = grid.n;
    if values.len() != grid.sample_count() {
        return Err("The field does not match its grid.".into());
    }
    if n[axis] % 2 != 0 || grid.lo[axis] as i64 * 2 != -(n[axis] as i64) {
        return Err("The grid is not symmetric across that plane.".into());
    }
    let pair = |row: &mut [f32], a: usize, b: usize| {
        let low = row[a].min(row[b]);
        row[a] = low;
        row[b] = low;
    };
    match axis {
        0 => values.par_chunks_mut(n[0]).for_each(|row| (0..n[0] / 2).for_each(|i| pair(row, i, n[0] - 1 - i))),
        1 => values.par_chunks_mut(n[0] * n[1]).for_each(|slab| {
            for j in 0..n[1] / 2 {
                (0..n[0]).for_each(|i| pair(slab, i + n[0] * j, i + n[0] * (n[1] - 1 - j)));
            }
        }),
        _ => {
            let slab = n[0] * n[1];
            let (lower, upper) = values.split_at_mut(slab * (n[2] / 2));
            lower.par_chunks_mut(slab).zip(upper.par_chunks_mut(slab).rev()).for_each(|(a, b)| {
                for (x, y) in a.iter_mut().zip(b) {
                    let low = x.min(*y);
                    *x = low;
                    *y = low;
                }
            });
        }
    }
    Ok(())
}

/// Samples the signed distance to a mesh onto `grid`, as a narrow band: real
/// distances at every corner of every cell the surface passes through, and
/// [`FAR`] with the right sign everywhere else.
///
/// The band is what extraction needs and all it needs, and it turns a
/// nearest-triangle query per sample into one per sample near the surface.
/// Combining two such fields with min and max stays exact for extraction: a
/// sample holds the stand-in only where its own mesh's sign is the same
/// across every cell it belongs to, so the stand-in never ends up on the
/// interpolated side of a crossing.
pub fn sample_mesh(positions: &[Vec3], indices: &[u32], grid: &Grid) -> Result<Vec<f32>, String> {
    crate::check_mesh(positions, indices)?;
    if grid.sample_count() > u32::MAX as usize {
        return Err("That grid has too many samples. Try a larger cell size.".into());
    }
    let scan = Scan::new(positions, indices, grid)?;
    let [nx, ny, nz] = grid.n;
    // Signs first, a slab of the lattice to a task.
    let slab_ends: Vec<usize> = (1..=nz).map(|k| k * nx * ny).collect();
    let mut values = parallel::build(&slab_ends, |k, out| {
        let mut bits = vec![0u64; nx.div_ceil(64)];
        for j in 0..ny {
            let any = scan.row(j, k, &mut bits);
            for i in 0..nx {
                out.push(if any && bits[i >> 6] >> (i & 63) & 1 != 0 { -FAR } else { FAR });
            }
        }
    });

    let cells = extract::find_cells(&scan, grid);
    let mut near: Vec<u32> = cells
        .cells
        .par_iter()
        .flat_map_iter(|cell| {
            let [i, j, k] = cell.at();
            (0..8).map(move |c| grid.index(i + (c & 1), j + (c >> 1 & 1), k + (c >> 2 & 1)) as u32)
        })
        .collect();
    near.par_sort_unstable();
    near.dedup();

    let bvh = TriangleBvh::build(positions, indices);
    let distances: Vec<f32> = near
        .par_iter()
        .with_min_len(256)
        .map(|&s| {
            let s = s as usize;
            let p = grid.position(s % nx, s / nx % ny, s / (nx * ny));
            bvh.distance(p).max(MIN_MAGNITUDE)
        })
        .collect();
    for (&s, d) in near.iter().zip(distances) {
        let v = &mut values[s as usize];
        *v = if *v < 0.0 { -d } else { d };
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_closed, icosphere, shells, sphere_volume};
    use sculpt_geom::Aabb;

    fn sphere_field(grid: &Grid, centre: Vec3, radius: f32) -> Vec<f32> {
        let mut values = vec![0.0; grid.sample_count()];
        for k in 0..grid.n[2] {
            for j in 0..grid.n[1] {
                for i in 0..grid.n[0] {
                    values[grid.index(i, j, k)] = grid.position(i, j, k).distance(centre) - radius;
                }
            }
        }
        values
    }

    fn cube_grid(half: f32, cell: f32) -> Grid {
        Grid::covering(&Aabb::new(Vec3::splat(-half), Vec3::splat(half)), cell).unwrap()
    }

    #[test]
    fn a_sphere_field_becomes_a_sphere() {
        let grid = cube_grid(1.0, 0.05);
        let centre = Vec3::new(0.013, -0.02, 0.031);
        let surface = surface_from_sdf(&sphere_field(&grid, centre, 0.8), &grid, [false; 3]).unwrap();
        assert_eq!(surface.stats.skipped_quads, 0);
        assert_eq!(surface.normals.len(), surface.positions.len());
        for (p, n) in surface.positions.iter().zip(&surface.normals) {
            assert!((p.distance(centre) - 0.8).abs() < 0.004, "vertex off the sphere: {p}");
            assert!(n.dot((*p - centre).normalize()) > 0.99);
        }
        let mesh = surface.into_mesh();
        assert_closed(&mesh, "sphere field");
        assert_eq!(shells(&mesh), 1);
        assert!((mesh.signed_volume() / sphere_volume(0.8) - 1.0).abs() < 0.005);
    }

    #[test]
    fn a_field_running_off_its_grid_is_closed_off() {
        // A sphere far larger than the lattice: every sample is inside.
        let grid = cube_grid(0.5, 0.1);
        let surface = surface_from_sdf(&sphere_field(&grid, Vec3::ZERO, 5.0), &grid, [false; 3]).unwrap();
        let mesh = surface.into_mesh();
        assert_closed(&mesh, "clipped field");
        let side = (grid.n[0] - 1) as f64 * 0.1;
        assert!((mesh.signed_volume() / side.powi(3) - 1.0).abs() < 0.02, "{}", mesh.signed_volume());
    }

    #[test]
    fn a_field_with_nothing_inside_is_an_error_not_a_panic() {
        let grid = cube_grid(0.5, 0.1);
        assert!(surface_from_sdf(&vec![FAR; grid.sample_count()], &grid, [false; 3]).is_err());
        assert!(surface_from_sdf(&[1.0, 2.0], &grid, [false; 3]).is_err());
        // One sample, the smallest grid there is: a speck, but a closed one.
        let one = Grid { cell: 1.0, lo: [0; 3], n: [1; 3] };
        let mesh = surface_from_sdf(&[-0.25], &one, [false; 3]).unwrap().into_mesh();
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(crate::testing::defects(&mesh), crate::testing::Defects::default());
    }

    #[test]
    fn a_symmetrized_field_meshes_to_exact_mirror_images() {
        let grid = cube_grid(1.0, 0.07).mirrored([true, true, true]);
        // Two blobs that are mirror images only to rounding.
        let mut values = sphere_field(&grid, Vec3::new(0.3, 0.1, 0.0), 0.5);
        let other = sphere_field(&grid, Vec3::new(-0.3000001, 0.1, 0.0), 0.5);
        values.iter_mut().zip(other).for_each(|(a, b)| *a = a.min(b));
        symmetrize(&mut values, &grid, 0).unwrap();
        let surface = surface_from_sdf(&values, &grid, [true, false, false]).unwrap();
        assert_eq!(surface.stats.mirror_skipped, 0);
        let bits = |p: Vec3| p.to_array().map(|v| (v + 0.0).to_bits());
        let all: std::collections::HashSet<_> = surface.positions.iter().map(|&p| bits(p)).collect();
        assert!(surface.positions.iter().all(|&p| all.contains(&bits(Vec3::new(-p.x, p.y, p.z)))));
        assert!(surface.positions.iter().any(|p| p.x == 0.0));
        assert_closed(&surface.into_mesh(), "mirrored blobs");
    }

    /// A field that is not symmetric, on a grid that is not either, with the
    /// mirror on: the positive half and its reflection, exactly.
    #[test]
    fn a_lopsided_field_is_made_symmetric_from_its_positive_half() {
        let bounds = Aabb::new(Vec3::new(-0.45, -0.6, -0.6), Vec3::new(0.85, 0.6, 0.6));
        let grid = Grid::covering(&bounds, 0.04).unwrap();
        assert_ne!(grid.lo[0] as i64 * 2, -(grid.n[0] as i64), "the grid was meant to be lopsided");
        // A ball off to the positive side, and a small one on the negative.
        let mut values = sphere_field(&grid, Vec3::new(0.25, 0.0, 0.05), 0.5);
        let small = sphere_field(&grid, Vec3::new(-0.3, 0.3, 0.0), 0.12);
        values.iter_mut().zip(small).for_each(|(a, b)| *a = a.min(b));

        let surface = surface_from_sdf(&values, &grid, [true, false, false]).unwrap();
        assert_eq!(surface.stats.mirror_skipped, 0);
        let bits = |p: Vec3| p.to_array().map(|v| (v + 0.0).to_bits());
        let all: std::collections::HashSet<_> = surface.positions.iter().map(|&p| bits(p)).collect();
        assert!(surface.positions.iter().all(|&p| all.contains(&bits(Vec3::new(-p.x, p.y, p.z)))));
        assert!(surface.positions.iter().any(|p| p.x == 0.0));
        // Two balls overlapping across the plane; the small one, being on the
        // negative side, is nowhere.
        let reach = surface.positions.iter().map(|p| p.x.abs()).fold(0.0, f32::max);
        assert!((reach - 0.75).abs() < 0.01, "reaches {reach}");
        for p in &surface.positions {
            let folded = Vec3::new(p.x.abs(), p.y, p.z);
            assert!((folded.distance(Vec3::new(0.25, 0.0, 0.05)) - 0.5).abs() < 0.01, "a vertex off both balls: {p}");
        }
        let mesh = surface.into_mesh();
        assert_closed(&mesh, "mirrored field");
        assert_eq!(shells(&mesh), 1);
    }

    #[test]
    fn symmetrize_works_along_every_axis_and_refuses_a_lopsided_grid() {
        let grid = cube_grid(0.5, 0.1).mirrored([true; 3]);
        for axis in 0..3 {
            let mut values = sphere_field(&grid, Vec3::new(0.11, 0.07, -0.13), 0.3);
            symmetrize(&mut values, &grid, axis).unwrap();
            let n = grid.n;
            for k in 0..n[2] {
                for j in 0..n[1] {
                    for i in 0..n[0] {
                        let mut twin = [i, j, k];
                        twin[axis] = n[axis] - 1 - twin[axis];
                        assert_eq!(values[grid.index(i, j, k)], values[grid.index(twin[0], twin[1], twin[2])]);
                    }
                }
            }
        }
        let lopsided = Grid { cell: 0.1, lo: [-3, -4, -4], n: [8, 8, 8] };
        assert!(symmetrize(&mut vec![0.0; 512], &lopsided, 0).is_err());
        assert!(symmetrize(&mut vec![0.0; 512], &lopsided, 1).is_ok());
    }

    #[test]
    fn a_mesh_samples_to_its_own_distance_field_near_the_surface() {
        let mesh = icosphere(5, 0.8, Vec3::ZERO);
        let grid = cube_grid(0.8, 0.06);
        let values = sample_mesh(&mesh.positions, &mesh.indices, &grid).unwrap();
        let mut banded = 0;
        for k in 0..grid.n[2] {
            for j in 0..grid.n[1] {
                for i in 0..grid.n[0] {
                    let (v, exact) = (values[grid.index(i, j, k)], grid.position(i, j, k).length() - 0.8);
                    if v.abs() < FAR {
                        banded += 1;
                        assert!((v - exact).abs() < 0.004, "{v} vs {exact}");
                    } else {
                        // Outside the band only the sign is promised, and no
                        // far sample may be within a cell of the surface.
                        assert!(exact.abs() > 0.03 && (v < 0.0) == (exact < 0.0));
                    }
                }
            }
        }
        assert!(banded > 2000 && banded < grid.sample_count() / 2);
        let mesh = surface_from_sdf(&values, &grid, [false; 3]).unwrap().into_mesh();
        assert_closed(&mesh, "resampled sphere");
        assert!((mesh.signed_volume() / sphere_volume(0.8) - 1.0).abs() < 0.01);
    }
}
