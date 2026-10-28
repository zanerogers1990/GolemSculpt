//! Voxel remesh: rebuilds a model's surface as an even mesh at a chosen
//! resolution, fusing overlapping and self-intersecting parts on the way.
//!
//! The mesh is scan-converted onto a lattice ([`crate::scan`]) and the surface
//! re-extracted from that ([`mod@crate::extract`]). No distance field is sampled
//! at any point: scan conversion gives the exact place the old surface
//! crosses every lattice edge and the true face normal there, which is both
//! faster than a nearest-triangle query per sample and better input for
//! placing vertices on creases. Time and memory follow the surface area in
//! cells; nothing is ever allocated per lattice sample.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::TriMesh;

use crate::budget::{self, Job};
// The limits are kept in one place; these names are where callers have always found them.
pub use crate::budget::{MAX_RESOLUTION, MAX_TRIANGLES, MIN_RESOLUTION};
use crate::extract::{self, ExtractOptions, Stats, Surface};
use crate::grid::Grid;
use crate::qef;
use crate::scan::Scan;

/// Output triangles per unit of source area, in cells. Surface Nets theory
/// gives twice the mean of |nx| + |ny| + |nz| (1.5 over a sphere); 2.8 was
/// measured on a sculpted model. The rounder figure errs toward too many.
const TRIANGLES_PER_CELL_AREA: f64 = 3.0;

#[derive(Clone, Copy, Debug)]
pub struct RemeshSettings {
    /// Cells along the model's longest side.
    pub resolution: u32,
    /// The coordinate planes (x = 0, y = 0, z = 0) the result must be exactly
    /// symmetric across: the model's mirror axes. It always is, whatever the
    /// model: the half on the positive side of each plane is kept and
    /// reflected onto the other, so where the halves differ the positive one
    /// wins.
    pub mirror: [bool; 3],
    /// The resolution is lowered as far as needed to stay under this.
    pub max_triangles: usize,
    /// How aligned a cell's normals must be for its vertex to be placed as
    /// a smooth surface rather than solved for a crease: the sharpest,
    /// [`qef::PLANAR_COSINE`], unless asked otherwise, which copies every
    /// edge the old surface has. See [`RemeshSettings::rounding_edges_under`].
    pub planar_cosine: f32,
}

impl RemeshSettings {
    pub fn new(resolution: u32) -> RemeshSettings {
        RemeshSettings { resolution, mirror: [false; 3], max_triangles: MAX_TRIANGLES, planar_cosine: qef::PLANAR_COSINE }
    }

    /// The same settings, placing every cell whose normals turn by less than
    /// `degrees` as the smooth surface it nearly is (see [`qef::solve_with`]):
    /// for a sculpted model whose detail is finer than the cells, which
    /// solved for its creases comes out scaly and pitted. Edges that turn by
    /// more stay sharp. Nought, or anything under the sharpest the solve
    /// gets, is the settings as they were.
    pub fn rounding_edges_under(self, degrees: f32) -> RemeshSettings {
        let cosine = (degrees.clamp(0.0, 180.0).to_radians() * 0.5).cos();
        RemeshSettings { planar_cosine: cosine.min(qef::PLANAR_COSINE), ..self }
    }
}

/// A rebuilt surface and what it took to make it.
#[derive(Clone, Debug)]
pub struct Remeshed {
    /// Closed, wound counter-clockwise seen from outside.
    pub mesh: TriMesh,
    /// A normal per vertex, taken from the faces of the source surface rather
    /// than re-averaged from the new triangles.
    pub normals: Vec<Vec3>,
    /// The resolution actually used: lower than asked for when the triangle
    /// budget stepped in.
    pub resolution: u32,
    /// Size of one lattice cell, the smallest feature that survives.
    pub cell: f32,
    pub stats: Stats,
    /// What was done about the meshes that went in not being closed.
    pub sources: Sources,
}

/// How many of the meshes handed to a remesh (one) or a boolean (several)
/// were not closed surfaces, and what was read in their place. The result is
/// closed either way; this is so the caller can say that the model's holes
/// were closed for it rather than leave that to be discovered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    /// Meshes with holes that had a rim, each capped flat across it before
    /// the mesh was read.
    pub capped: usize,
    /// Meshes still open after any capping (a tear with no rim, faces wound
    /// the wrong way), where inside and outside were settled by a vote of the
    /// three axes: right away from the damage, a best guess next to it.
    pub voted: usize,
}

impl Sources {
    pub(crate) fn of<'a>(scans: impl IntoIterator<Item = &'a Scan>) -> Sources {
        let mut sources = Sources::default();
        for scan in scans {
            sources.capped += scan.was_capped() as usize;
            sources.voted += scan.was_voted() as usize;
        }
        sources
    }
}

impl Remeshed {
    pub(crate) fn new(surface: Surface, resolution: u32, cell: f32, sources: Sources) -> Remeshed {
        let Surface { positions, normals, indices, stats } = surface;
        Remeshed { mesh: TriMesh::new(positions, indices), normals, resolution, cell, stats, sources }
    }
}

/// Total area of a triangle list.
pub fn surface_area(positions: &[Vec3], indices: &[u32]) -> f64 {
    indices
        .par_chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize].as_dvec3());
            0.5 * (b - a).cross(c - a).length()
        })
        .sum()
}

/// `requested`, brought into range and then lowered as far as needed for a
/// surface of `area` on a model `extent` long to come out under
/// `max_triangles`. Also returns the triangles to expect at that resolution.
/// Triangles grow with resolution squared, so the fit is a square root.
pub fn budgeted_resolution(area: f64, extent: f64, requested: u32, max_triangles: usize) -> (u32, usize) {
    let mut resolution = requested.clamp(MIN_RESOLUTION, MAX_RESOLUTION);
    if !(extent > 0.0 && area > 0.0) {
        return (resolution, 0);
    }
    let per_resolution_squared = TRIANGLES_PER_CELL_AREA * area / (extent * extent);
    let fits = (max_triangles as f64 / per_resolution_squared).sqrt().floor();
    if (resolution as f64) > fits {
        resolution = (fits as u32).clamp(MIN_RESOLUTION, MAX_RESOLUTION);
    }
    (resolution, (per_resolution_squared * (resolution as f64).powi(2)) as usize)
}

/// Rebuilds `mesh` at `resolution` cells along its longest side. With
/// `mirror`, the result is exactly symmetric across those coordinate planes
/// whatever the model: its positive half and that half's reflection (see
/// [`RemeshSettings::mirror`]).
pub fn remesh(mesh: &TriMesh, resolution: u32, mirror: [bool; 3]) -> Result<TriMesh, String> {
    let settings = RemeshSettings { mirror, ..RemeshSettings::new(resolution) };
    remesh_with(&mesh.positions, &mesh.indices, &settings).map(|r| r.mesh)
}

/// What a model is told when it lies wholly behind one of its mirror planes.
/// A mirrored rebuild keeps the half on the positive side of each plane and
/// reflects it (see [`RemeshSettings::mirror`]): of this model, nothing.
const FAR_SIDE: &str = "The model lies wholly on the far side of its mirror plane, so its mirror image is empty. \
                        Turn the mirror off for this, move the model across the plane, or move its centre onto it \
                        with Set Origin to Mesh.";

/// Whether everything up to `highest` (the top corner of some bounds) lies
/// behind one of the `mirror` planes, where a mirrored rebuild keeps nothing.
/// On the plane counts as behind it: there is no thickness there to keep.
pub(crate) fn behind_mirror(highest: Vec3, mirror: [bool; 3]) -> bool {
    (0..3).any(|axis| mirror[axis] && highest[axis] <= 0.0)
}

/// Refuses a model whose bounds reach no higher than `highest` if a rebuild
/// mirrored across `mirror` would leave nothing of it, in the words
/// [`remesh_with`] uses.
///
/// This is asked before anything about size is. Too many triangles and too
/// much memory are both answered with a lower resolution to try, and for
/// this model there is none: it was sent from 1024 to 740, and at 740 told
/// that it lay on the far side of its mirror plane. A caller with a size
/// limit of its own to apply first asks this before that.
pub fn check_mirror_side(highest: Vec3, mirror: [bool; 3]) -> Result<(), String> {
    if behind_mirror(highest, mirror) { Err(FAR_SIDE.into()) } else { Ok(()) }
}

/// What a remesh works on, once everything that can be asked beforehand has
/// been: its lattice, and the cell and resolution that lattice is of.
#[derive(Debug)]
struct Admitted {
    grid: Grid,
    cell: f32,
    resolution: u32,
}

/// Everything asked of a remesh before the model is read: whether there is
/// a model, and whether what rebuilding it would take can be afforded.
fn admit(positions: &[Vec3], indices: &[u32], settings: &RemeshSettings) -> Result<Admitted, String> {
    let bounds = crate::check_mesh(positions, indices)?;
    let extent = bounds.size().max_element();
    if !(extent > 0.0) {
        return Err("The model has no size.".into());
    }
    check_mirror_side(bounds.max, settings.mirror)?;
    let area = surface_area(positions, indices);
    let (resolution, _) = budgeted_resolution(area, extent as f64, settings.resolution, settings.max_triangles);
    // Before anything is allocated: what the lattice and the scan will take.
    let job_at = |resolution: u32| {
        let cell = extent / resolution as f32;
        let crossings = budget::crossings(positions, indices, &bounds, settings.mirror, cell);
        Job { bounds, cell, mirror: settings.mirror, solids: 1, crossings }
    };
    let grid = budget::plan(resolution, job_at).map_err(|refused| refused.message(&budget::MODEL))?;
    Ok(Admitted { grid, cell: extent / resolution as f32, resolution })
}

/// Whether [`remesh_with`] would set to work on this, or refuse it for its
/// place or its size, and in what words. Nothing is read onto a lattice and
/// nothing allocated: this is for a caller with a refusal of its own to
/// word, which wants to offer a resolution that will not be refused in turn.
pub fn admits(positions: &[Vec3], indices: &[u32], settings: &RemeshSettings) -> Result<(), String> {
    admit(positions, indices, settings).map(|_| ())
}

/// [`remesh()`] from plain arrays, with every setting and the full result.
///
/// The triangles need not be one clean shell. Closed shells that overlap
/// (parts joined without welding) come out as their union, a surface that
/// passes through itself is fused, and holes are capped, each flat across its
/// own rim, whether three missing triangles or the open base of a bust.
pub fn remesh_with(positions: &[Vec3], indices: &[u32], settings: &RemeshSettings) -> Result<Remeshed, String> {
    let Admitted { grid, cell, resolution } = admit(positions, indices, settings)?;
    let scan = Scan::new(positions, indices, &grid)?;
    // The estimate can be low for a crumpled surface: allow half as much again
    // before refusing outright.
    let options = ExtractOptions {
        mirror: settings.mirror,
        max_vertices: settings.max_triangles / 4 * 3,
        planar_cosine: settings.planar_cosine,
    };
    let surface = extract::extract(&scan, &grid, &options).map_err(|error| {
        if scan.open_fraction() > 0.5 {
            "The model is not a closed surface, so it has no inside to rebuild. Close its holes first.".to_string()
        } else {
            error
        }
    })?;
    Ok(Remeshed::new(surface, resolution, cell, Sources::of([&scan])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_closed, box_mesh, icosphere, lens_volume, rotated, shells, sphere_volume, uv_sphere};
    use crate::testing::{Rng, bowl, open_tube, turn, without_near, worst_radius_error};
    use std::collections::{HashMap, HashSet};

    fn settings(resolution: u32, mirror: [bool; 3]) -> RemeshSettings {
        RemeshSettings { mirror, ..RemeshSettings::new(resolution) }
    }

    fn rebuilt(mesh: &TriMesh, resolution: u32, mirror: [bool; 3]) -> Remeshed {
        let out = remesh_with(&mesh.positions, &mesh.indices, &settings(resolution, mirror)).unwrap();
        assert_eq!(out.stats.skipped_quads, 0);
        assert_eq!(out.stats.residual_collapses, 0);
        assert_eq!(out.normals.len(), out.mesh.vertex_count());
        out
    }

    #[test]
    fn a_sphere_comes_back_a_sphere() {
        let source = icosphere(5, 0.5, Vec3::new(0.013, -0.007, 0.021));
        for resolution in [32, 64, 100] {
            let out = rebuilt(&source, resolution, [false; 3]);
            assert_closed(&out.mesh, "sphere");
            assert_eq!(shells(&out.mesh), 1);
            let volume = out.mesh.signed_volume() / source.signed_volume();
            assert!((volume - 1.0).abs() < 0.004, "volume ratio {volume} at {resolution}");
            let off = worst_radius_error(&out.mesh, 0.5, Vec3::new(0.013, -0.007, 0.021));
            assert!(off < 0.12 * out.cell + 4e-4, "vertex {} cells off the surface at {resolution}", off / out.cell);
            assert_eq!(out.resolution, resolution);
        }
    }

    /// The check for a leaked inside/outside decision. A lattice line that
    /// slips between two triangles turns a run of samples inside out, which
    /// shows up as a spike or tunnel changing the volume, or as a stray shell
    /// inside or outside the surface. Dense sources at lattices that do and
    /// do not line up with them, a pole-fan sphere among them.
    #[test]
    fn a_dense_sphere_has_no_stray_shells_or_spikes() {
        let centre = Vec3::new(0.02, 0.01, -0.03);
        let sources = [(icosphere(7, 0.5, centre), "icosphere"), (uv_sphere(300, 600, 0.5, centre), "uv sphere")];
        for (source, name) in sources {
            for resolution in [60, 97] {
                let out = rebuilt(&source, resolution, [false; 3]);
                assert_closed(&out.mesh, name);
                assert_eq!(shells(&out.mesh), 1, "{name} at {resolution}");
                let volume = out.mesh.signed_volume() / sphere_volume(0.5);
                assert!((volume - 1.0).abs() < 0.003, "{name} at {resolution}: volume ratio {volume}");
                let off = worst_radius_error(&out.mesh, 0.5, centre);
                assert!(off < 0.1 * out.cell, "{name} at {resolution}: a vertex {} cells off", off / out.cell);
            }
        }
    }

    /// A ball ridged finer than the cells (a muscled body remeshed low): the
    /// shading noise each vertex normal makes against its neighbours', in
    /// degrees, root mean square.
    fn shading_noise(mesh: &TriMesh) -> f32 {
        let mut normals = vec![Vec3::ZERO; mesh.vertex_count()];
        for t in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize]);
            let n = (b - a).cross(c - a);
            t.iter().for_each(|&v| normals[v as usize] += n);
        }
        let normals: Vec<Vec3> = normals.iter().map(|n| n.normalize_or_zero()).collect();
        let mut sums = vec![Vec3::ZERO; mesh.vertex_count()];
        for t in mesh.indices.chunks_exact(3) {
            for e in 0..3 {
                sums[t[e] as usize] += normals[t[(e + 1) % 3] as usize];
            }
        }
        let total: f32 = sums
            .iter()
            .zip(&normals)
            .map(|(sum, n)| n.angle_between(sum.normalize_or(*n)).to_degrees().powi(2))
            .sum();
        (total / mesh.vertex_count() as f32).sqrt()
    }

    /// Rounding edges under 60 degrees: a ball ridged finer than the cells
    /// comes out much less rough than solved for every crease, the same size,
    /// and a box still a box. The sharpest setting is the plain remesh to
    /// the bit.
    #[test]
    fn rounding_slight_edges_smooths_fine_ridges_and_keeps_a_box_sharp() {
        let mut ridged = icosphere(7, 0.5, Vec3::ZERO);
        for p in &mut ridged.positions {
            let d = p.normalize();
            // Ridges round the ball, about 1.7 cells apart at 64, and two
            // fifths of a cell deep.
            let around = d.z.atan2(d.x);
            let r = 0.5 + 0.003 * (120.0 * around).sin() * (1.0 - d.y * d.y).sqrt();
            *p = d * r;
        }
        let sharp = rebuilt(&ridged, 64, [false; 3]);
        let rounded = remesh_with(&ridged.positions, &ridged.indices, &settings(64, [false; 3]).rounding_edges_under(60.0)).unwrap();
        assert_closed(&rounded.mesh, "rounded ridged ball");
        let (noise_sharp, noise_rounded) = (shading_noise(&sharp.mesh), shading_noise(&rounded.mesh));
        assert!(noise_rounded < 0.6 * noise_sharp, "shading noise {noise_rounded} rounded, {noise_sharp} sharp");
        let volume = rounded.mesh.signed_volume() / sharp.mesh.signed_volume();
        assert!((volume - 1.0).abs() < 0.005, "volume ratio {volume}");
        let off = worst_radius_error(&rounded.mesh, 0.5, Vec3::ZERO);
        assert!(off < 0.5 * rounded.cell, "a vertex {} cells off the ball", off / rounded.cell);

        let half = Vec3::new(0.5, 0.35, 0.25);
        let cube = rotated(&box_mesh(-half, half), Vec3::new(31.0, 17.0, 9.0));
        let plain = rebuilt(&cube, 48, [false; 3]);
        let still_sharp = remesh_with(&cube.positions, &cube.indices, &settings(48, [false; 3]).rounding_edges_under(60.0)).unwrap();
        assert_eq!(still_sharp.mesh.positions, plain.mesh.positions, "a box's right angles are still solved for");
        let as_before = remesh_with(&ridged.positions, &ridged.indices, &settings(64, [false; 3]).rounding_edges_under(0.0)).unwrap();
        assert_eq!(as_before.mesh.positions, sharp.mesh.positions);
    }

    #[test]
    fn a_box_keeps_its_volume_edges_and_corners() {
        let half = Vec3::new(0.5, 0.35, 0.25);
        let cube = box_mesh(-half, half);
        for (source, name) in [(cube.clone(), "box"), (rotated(&cube, Vec3::new(31.0, 17.0, 9.0)), "turned box")] {
            let out = rebuilt(&source, 48, [false; 3]);
            assert_closed(&out.mesh, name);
            assert_eq!(shells(&out.mesh), 1);
            let volume = out.mesh.signed_volume() / source.signed_volume();
            assert!((volume - 1.0).abs() < 0.004, "{name}: volume ratio {volume}");
            // Plain Surface Nets leaves every corner most of a cell short.
            for corner in &source.positions {
                let nearest = out.mesh.positions.iter().map(|p| p.distance(*corner)).fold(f32::INFINITY, f32::min);
                assert!(nearest < 0.2 * out.cell, "{name}: corner {} cells from any vertex", nearest / out.cell);
            }
        }
    }

    /// Faces, edges and corners exactly on lattice samples, where every
    /// inside/outside decision is a tie.
    #[test]
    fn a_box_snapped_to_the_lattice_is_rebuilt_exactly() {
        let source = box_mesh(Vec3::splat(-0.75), Vec3::splat(0.75));
        let out = rebuilt(&source, 15, [true; 3]);
        assert_eq!(out.cell, 0.1);
        assert_closed(&out.mesh, "snapped box");
        assert!((out.mesh.signed_volume() / 3.375 - 1.0).abs() < 1e-3, "{}", out.mesh.signed_volume());
        assert_eq!(out.stats.mirror_skipped, 0);
        for p in &out.mesh.positions {
            assert!((p.abs().max_element() - 0.75).abs() < 2e-3, "vertex off the box: {p}");
        }
    }

    /// Twelve triangles at a fine resolution: each spans tens of thousands of
    /// lattice lines, and its rows are shared out between threads.
    #[test]
    fn a_coarse_model_at_a_fine_resolution() {
        let source = rotated(&box_mesh(Vec3::splat(-0.5), Vec3::splat(0.5)), Vec3::new(3.0, 0.0, 7.0));
        let out = rebuilt(&source, 300, [false; 3]);
        assert_closed(&out.mesh, "fine box");
        assert_eq!(shells(&out.mesh), 1);
        assert!((out.mesh.signed_volume() - 1.0).abs() < 2e-4, "{}", out.mesh.signed_volume());
        assert!(out.mesh.triangle_count() > 700_000);
    }

    /// The same sphere in each octant, well clear of the coordinate planes,
    /// and astride them: wherever the lattice's zero falls (before its first
    /// sample, past its last, or in the middle) the result is the same shape.
    #[test]
    fn a_model_is_rebuilt_alike_wherever_it_sits() {
        let mut volumes = Vec::new();
        for octant in 0..9 {
            let sign = |bit: usize| if octant >> bit & 1 != 0 { -1.0 } else { 1.0 };
            let astride = Vec3::new(0.1, -0.2, 0.05);
            let centre = if octant == 8 { astride } else { Vec3::new(sign(0), sign(1), sign(2)) * 3.0 };
            let out = rebuilt(&icosphere(4, 0.5, centre), 40, [false; 3]);
            assert_closed(&out.mesh, "sphere in an octant");
            assert!(worst_radius_error(&out.mesh, 0.5, centre) < 0.05 * out.cell);
            volumes.push(out.mesh.signed_volume());
        }
        let (low, high) = volumes.iter().fold((f64::MAX, f64::MIN), |(l, h), &v| (l.min(v), h.max(v)));
        assert!(high / low - 1.0 < 2e-3, "volumes from {low} to {high}");
    }

    #[test]
    fn overlapping_shells_are_fused_into_their_union() {
        let mut source = icosphere(5, 1.0, Vec3::new(-0.5, 0.0, 0.0));
        source.append(&icosphere(5, 1.0, Vec3::new(0.5, 0.0, 0.0)));
        let out = rebuilt(&source, 80, [false; 3]);
        assert_closed(&out.mesh, "two spheres");
        assert_eq!(shells(&out.mesh), 1, "the wall inside the overlap should be gone");
        let want = 2.0 * sphere_volume(1.0) - lens_volume(1.0, 1.0);
        assert!((out.mesh.signed_volume() / want - 1.0).abs() < 0.01, "{} vs {want}", out.mesh.signed_volume());
    }

    /// Every vertex's reflection is a vertex, bit for bit, with the mirrored
    /// neighbours. Mirrored strokes stay exact on such a mesh and drift on
    /// anything less.
    #[test]
    fn a_symmetric_model_comes_back_exactly_symmetric() {
        let source = icosphere(5, 0.5, Vec3::ZERO);
        for (resolution, mirror) in [(64, [true, false, false]), (97, [true, false, false]), (50, [true, true, true])] {
            let out = rebuilt(&source, resolution, mirror);
            assert_closed(&out.mesh, "mirrored sphere");
            assert_eq!(out.stats.mirror_skipped, 0);
            for axis in (0..3).filter(|&a| mirror[a]) {
                let (unmatched, on_plane, mismatched) = symmetry_report(&out.mesh, axis);
                let report = format!(
                    "res {resolution} axis {axis}: {} vertices, {unmatched} without an exact twin, \
                     {on_plane} on the plane, {mismatched} whose neighbours are not their twin's mirrored",
                    out.mesh.vertex_count()
                );
                assert_eq!((unmatched, mismatched), (0, 0), "{report}");
                assert!(on_plane > 0, "{report}");
            }
        }
    }

    /// Symmetry where the surface is at its most awkward: a tangle and its
    /// mirror image, coarse enough that many cells hold several sheets. Those
    /// cells have to be divided alike on both sides before they can be paired.
    #[test]
    fn a_symmetric_tangle_comes_back_exactly_symmetric() {
        let (mut creases, mut vertices) = (0, 0);
        for seed in 0..5 {
            let mut rng = Rng::new(seed + 40);
            let mut half = TriMesh::default();
            for _ in 0..25 {
                let centre = (rng.vec3() - 0.5) * Vec3::new(1.2, 1.6, 1.6);
                half.append(&icosphere(2, 0.08 + rng.next() * 0.22, centre));
            }
            let mut source = half.clone();
            half.transform(&glam::Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
            source.append(&half);

            let out = rebuilt(&source, 24 + seed as u32 * 5, [true, false, false]);
            assert_closed(&out.mesh, "symmetric tangle");
            assert_eq!(out.stats.mirror_skipped, 0, "seed {seed}");
            let (unmatched, on_plane, mismatched) = symmetry_report(&out.mesh, 0);
            assert_eq!((unmatched, mismatched), (0, 0), "seed {seed}: {on_plane} on the plane");
            creases += out.stats.crease_cells;
            vertices += out.mesh.vertex_count();
        }
        assert!(creases > 100, "{creases} crease cells among {vertices} vertices");
    }

    /// Asserts a mesh is exactly its own mirror image across the given planes:
    /// every vertex's reflection a vertex, bit for bit, with the mirrored
    /// neighbours, and some of them on the plane.
    fn assert_mirrored(out: &Remeshed, axes: [bool; 3], what: &str) {
        assert_closed(&out.mesh, what);
        assert_eq!(out.stats.mirror_skipped, 0, "{what}");
        for axis in (0..3).filter(|&a| axes[a]) {
            let (unmatched, on_plane, mismatched) = symmetry_report(&out.mesh, axis);
            assert_eq!((unmatched, mismatched), (0, 0), "{what}, axis {axis}: {on_plane} on the plane");
        }
    }

    /// The model the app actually hands over: sculpted with the mirror on, so
    /// symmetric to a few parts in ten million and no better. Here one side
    /// of a sphere is jittered by a millionth. The result is exactly
    /// symmetric, and more than that it is the unjittered sphere's own remesh
    /// to the last bit, because the jittered side was never looked at: the
    /// positive half is the source.
    #[test]
    fn a_nearly_symmetric_model_comes_back_exactly_symmetric() {
        let sound = icosphere(5, 0.5, Vec3::ZERO);
        let mut rng = Rng::new(31);
        let mut source = sound.clone();
        let mut jittered = 0;
        // Not the outermost vertices: the lattice is sized from the bounds.
        // Nor those of the triangles that reach across the plane.
        for p in source.positions.iter_mut().filter(|p| p.x < -0.03 && p.abs().max_element() < 0.49) {
            *p += (rng.vec3() - 0.5) * 2e-6;
            jittered += 1;
        }
        assert!(jittered > 3000 && source.bounds() == sound.bounds());
        for resolution in [64, 97] {
            let out = rebuilt(&source, resolution, [true, false, false]);
            assert_mirrored(&out, [true, false, false], "jittered sphere");
            assert_eq!(shells(&out.mesh), 1);
            let reference = rebuilt(&sound, resolution, [true, false, false]);
            assert!(out.mesh == reference.mesh, "the jittered half showed through at {resolution}");
            // Jitter on the positive side does show, and still comes out symmetric.
            let flipped: Vec<Vec3> = source.positions.iter().map(|p| Vec3::new(-p.x, p.y, p.z)).collect();
            let mut other = TriMesh::new(flipped, source.indices.clone());
            other.flip();
            let out = rebuilt(&other, resolution, [true, false, false]);
            assert_mirrored(&out, [true, false, false], "sphere jittered on the positive side");
            assert!(out.mesh != reference.mesh);
        }
    }

    /// A model that really is lopsided, with the mirror on. A large bump on
    /// the positive side appears on both; the same bump on the negative side
    /// is gone; either way the result is exactly symmetric and in one piece.
    #[test]
    fn a_bump_on_one_side_is_mirrored_or_dropped_by_which_side_it_is_on() {
        let ball = icosphere(5, 0.5, Vec3::ZERO);
        let bump_at = Vec3::new(0.45, 0.2, 0.1);
        let alone = rebuilt(&icosphere(4, 0.2, bump_at), 40, [false; 3]).mesh.signed_volume();
        for side in [1.0, -1.0] {
            let mut source = ball.clone();
            source.append(&icosphere(4, 0.2, bump_at * Vec3::new(side, 1.0, 1.0)));
            let plain = rebuilt(&source, 80, [false; 3]);
            let out = rebuilt(&source, 80, [true, false, false]);
            let what = if side > 0.0 { "bump on the positive side" } else { "bump on the negative side" };
            assert_mirrored(&out, [true, false, false], what);
            assert_eq!(shells(&out.mesh), 1, "{what}");

            let sphere = sphere_volume(0.5);
            let bump = plain.mesh.signed_volume() - sphere;
            assert!(bump > 0.3 * alone && bump < 0.9 * alone, "the bump sticks out by {bump}");
            let want = if side > 0.0 { sphere + 2.0 * bump } else { sphere };
            let volume = out.mesh.signed_volume();
            assert!((volume / want - 1.0).abs() < 0.006, "{what}: volume {volume}, expected {want}");
            let reach = out.mesh.positions.iter().map(|p| p.x.abs()).fold(0.0, f32::max);
            let expected = if side > 0.0 { 0.65 } else { 0.5 };
            assert!((reach - expected).abs() < out.cell, "{what}: reaches {reach}");
        }
    }

    /// The ends of the same rule: a model wholly on the positive side gains a
    /// twin, one wholly on the negative side leaves nothing and says so, and
    /// several planes at once keep one corner of the model.
    #[test]
    fn a_model_to_one_side_of_its_mirror_plane_is_doubled_or_refused() {
        let aside = icosphere(4, 0.3, Vec3::new(1.0, 0.1, 0.0));
        let out = rebuilt(&aside, 48, [true, false, false]);
        assert_mirrored(&out, [true, false, false], "sphere to one side");
        assert_eq!(shells(&out.mesh), 2);
        assert!((out.mesh.signed_volume() / (2.0 * sphere_volume(0.3)) - 1.0).abs() < 0.01);

        let behind = icosphere(4, 0.3, Vec3::new(-1.0, 0.1, 0.0));
        let settings = settings(48, [true, false, false]);
        let error = remesh_with(&behind.positions, &behind.indices, &settings).unwrap_err();
        assert_eq!(error, FAR_SIDE);

        let mut lopsided = icosphere(4, 0.5, Vec3::new(0.11, 0.03, 0.0));
        lopsided.append(&icosphere(3, 0.2, Vec3::new(-0.45, 0.2, 0.1)));
        for axes in [[true, true, false], [true, true, true], [false, false, true]] {
            let out = rebuilt(&lopsided, 64, axes);
            assert_mirrored(&out, axes, "lopsided model");
            assert_eq!(shells(&out.mesh), 1);
            assert!(out.stats.mirrored_cells >= out.stats.cells);
        }
    }

    /// Normals on meshes whose triangles are tiny in absolute terms: dense, or
    /// small, or both. A normal worked out carelessly from such a triangle
    /// vanishes, and the vertex then falls back to straight up.
    #[test]
    fn normals_follow_the_surface_on_dense_and_on_tiny_models() {
        let first = rebuilt(&icosphere(5, 0.5, Vec3::ZERO), 128, [false; 3]);
        assert!(first.mesh.triangle_count() > 100_000, "the first remesh should be dense");
        let second = rebuilt(&first.mesh, 128, [false; 3]);
        assert_normals_follow_surface(&second, "sphere, 128 then 128");
        // A remesh of a remesh is no further from the surface than the first.
        let off = worst_radius_error(&second.mesh, 0.5, Vec3::ZERO);
        assert!(off < 0.05 * second.cell, "a vertex {} cells off after two remeshes", off / second.cell);

        let tiny = rebuilt(&icosphere(4, 0.005, Vec3::ZERO), 64, [false; 3]);
        assert_normals_follow_surface(&tiny, "1 cm sphere, 64");
        assert!((tiny.mesh.signed_volume() / sphere_volume(0.005) - 1.0).abs() < 0.01);
    }

    #[test]
    fn spheres_that_nearly_touch_stay_two_clean_surfaces() {
        // A gap of a third of a cell: the cells between hold two sheets each.
        let mut source = icosphere(4, 0.5, Vec3::new(-0.505, 0.0, 0.0));
        source.append(&icosphere(4, 0.5, Vec3::new(0.505, 0.0, 0.0)));
        let out = rebuilt(&source, 67, [false; 3]);
        assert_closed(&out.mesh, "kissing spheres");
        let volume = out.mesh.signed_volume() / (2.0 * sphere_volume(0.5));
        assert!((volume - 1.0).abs() < 0.01, "volume ratio {volume}");
    }

    /// Tangles of small overlapping shapes at a lattice too coarse for them:
    /// thin wedges, pinches and near-misses in every cell. Whatever shape
    /// comes out, it must be a closed 2-manifold.
    #[test]
    fn a_coarse_tangle_still_gives_a_closed_manifold() {
        let mut creases = 0;
        for seed in 0..6 {
            let mut rng = Rng::new(seed + 1);
            let mut source = TriMesh::default();
            for _ in 0..40 {
                let centre = (rng.vec3() - 0.5) * 1.6;
                source.append(&icosphere(2, 0.08 + rng.next() * 0.22, centre));
            }
            let mirror = [seed % 2 == 0, false, seed % 3 == 0];
            let out = rebuilt(&source, 22 + seed as u32 * 3, mirror);
            assert_closed(&out.mesh, "tangle");
            creases += out.stats.crease_cells;
        }
        assert!(creases > 50, "the tangles were meant to be awkward: {creases} crease cells");
    }

    /// The caller is told when the model was not closed and what was read
    /// in its place, so the app can say so instead of quietly lidding a bust.
    #[test]
    fn the_result_says_when_holes_were_capped() {
        let whole = icosphere(5, 0.5, Vec3::new(0.01, -0.02, 0.015));
        assert_eq!(rebuilt(&whole, 48, [false; 3]).sources, Sources::default());

        let pierced = without_near(&whole, Vec3::new(0.51, -0.02, 0.015), 0.05);
        assert!(pierced.triangle_count() < whole.triangle_count());
        assert_eq!(rebuilt(&pierced, 48, [false; 3]).sources, Sources { capped: 1, voted: 0 });
        assert_eq!(rebuilt(&bowl(48, 96, 0.5), 48, [false; 3]).sources, Sources { capped: 1, voted: 0 });

        // Half the sphere wound inside out: there is no hole to cap, and no
        // line through both halves that enters and then leaves.
        let mut torn = whole.clone();
        for t in 0..torn.triangle_count() {
            if whole.triangle(t).iter().all(|p| p.x > 0.01) {
                torn.indices.swap(t * 3 + 1, t * 3 + 2);
            }
        }
        assert_eq!(rebuilt(&torn, 48, [false; 3]).sources, Sources { capped: 0, voted: 1 });
    }

    /// A dense sphere with a few triangles missing, here and there and all
    /// at once. Each hole is capped where it is and nothing else changes: the
    /// same triangle count as the whole sphere's remesh, the same volume, and
    /// not one vertex off the sphere. In particular no streak, which is what
    /// a hole paints when inside is read along one axis alone: a bar from the
    /// hole to the edge of the lattice.
    #[test]
    fn a_few_missing_triangles_are_patched_and_nothing_else_changes() {
        let centre = Vec3::new(0.01, -0.02, 0.015);
        let whole = icosphere(6, 0.5, centre);
        let ways = [Vec3::X, -Vec3::Y, Vec3::new(0.3, 0.5, 0.8).normalize(), Vec3::new(-0.7, 0.1, -0.7).normalize()];
        let pierce = |mesh: &TriMesh, way: Vec3| without_near(mesh, centre + way * 0.5, 0.02);
        let mut sources: Vec<TriMesh> = ways.iter().map(|&way| pierce(&whole, way)).collect();
        sources.push(ways.iter().fold(whole.clone(), |mesh, &way| pierce(&mesh, way)));
        for resolution in [64, 150] {
            let sound = rebuilt(&whole, resolution, [false; 3]);
            for (case, source) in sources.iter().enumerate() {
                let missing = whole.triangle_count() - source.triangle_count();
                assert!((20..200).contains(&missing), "{missing} triangles removed");
                let out = rebuilt(source, resolution, [false; 3]);
                let what = format!("case {case} at {resolution}");
                assert_closed(&out.mesh, &what);
                assert_eq!(shells(&out.mesh), 1, "{what}");
                assert_eq!(out.mesh.triangle_count(), sound.mesh.triangle_count(), "{what}");
                let volume = out.mesh.signed_volume() / sound.mesh.signed_volume();
                assert!((volume - 1.0).abs() < 1e-4, "{what}: volume ratio {volume}");
                let off = worst_radius_error(&out.mesh, 0.5, centre);
                assert!(off < 0.1 * out.cell, "{what}: a vertex {} cells off the sphere", off / out.cell);
            }
        }
    }

    /// A sphere with a large opening: the lower half of one, a bowl with no
    /// thickness.
    ///
    /// Strictly there is no right answer, since an open sheet encloses
    /// nothing. The useful one is the solid a person sees in it: the bowl
    /// filled to the brim and closed flat across the rim, the same whichever
    /// way the bowl is turned.
    ///
    /// The Unity SparseRemesher read inside along x alone, as a running sum of
    /// signed crossings, and only in the bricks of lattice near the surface;
    /// quads it could not finish were closed by its hole patcher. Going by
    /// its code, a bowl opening along y or z came out right, lidded by the
    /// patcher. Opening along +x, every ray went in through the wall and
    /// never came back to zero, so the solid ran on past the rim as far as
    /// the bricks did before it was patched shut. Opening along -x, every ray
    /// came in through the opening and left through the wall, so the solid
    /// was read as lying behind the bowl instead of in it: a crust of bricks.
    /// A vote between the three axes, which is what the app's own remesher
    /// did, mends all of those and fills a bowl turned 45 degrees only a
    /// third of the way, with scraps left over.
    ///
    /// Here the rim is capped before anything is scanned, so the answer is
    /// the half ball in every position: volume to a third of a percent, in
    /// one piece, flat across the top.
    #[test]
    fn a_bowl_is_filled_to_the_brim_whichever_way_it_faces() {
        let half_ball = sphere_volume(0.5) / 2.0;
        let turns = [
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -90.0),
            Vec3::new(90.0, 0.0, 0.0),
            Vec3::new(180.0, 0.0, 0.0),
            Vec3::new(45.0, 0.0, 0.0),
            Vec3::new(35.0, 20.0, 50.0),
            Vec3::new(54.7, 0.0, 45.0),
        ];
        for degrees in turns {
            let source = rotated(&bowl(96, 192, 0.5), degrees);
            let up = turn(degrees) * Vec3::Y;
            for resolution in [48, 96] {
                let out = rebuilt(&source, resolution, [false; 3]);
                let what = format!("bowl opening towards {up:.2} at {resolution}");
                assert_closed(&out.mesh, &what);
                assert_eq!(shells(&out.mesh), 1, "{what}");
                let volume = out.mesh.signed_volume() / half_ball;
                assert!((volume - 1.0).abs() < 0.0035, "{what}: {volume} of a half ball");
                for p in &out.mesh.positions {
                    let (height, radius) = (p.dot(up), p.length());
                    // On the wall below the rim, on the lid inside it, or on
                    // the crease where they meet, which is held less tightly.
                    let (slack, crease) = (0.1 * out.cell, 0.3 * out.cell);
                    let on_wall = (radius - 0.5).abs() < slack && height < slack;
                    let on_lid = height.abs() < slack && radius < 0.5 + slack;
                    let on_rim = height.abs() < crease && (radius - 0.5).abs() < crease;
                    assert!(on_wall || on_lid || on_rim, "{what}: a vertex at height {height}, radius {radius}");
                }
            }
        }
    }

    /// Shapes open at both ends: a pipe with no thickness, and a sphere with
    /// two opposite patches cut away. Lying along an axis these are the cases
    /// with no tell-tale at all: a lattice line down the bore meets nothing
    /// and every line across it closes, so each line on its own looks
    /// healthy, and a remesh that trusted the lines along x would return
    /// nothing for the pipe along x and a solid rod for the same pipe along
    /// y. Capped at both ends they are a rod and a ball with two flats,
    /// whichever way they lie.
    #[test]
    fn a_shape_open_at_both_ends_is_capped_at_both() {
        let rod = std::f64::consts::PI * 0.09 * 1.2;
        let turns = [
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 90.0),
            Vec3::new(0.0, 90.0, 0.0),
            Vec3::new(0.0, 0.0, 45.0),
            Vec3::new(10.0, 40.0, 25.0),
        ];
        for degrees in turns {
            let along = turn(degrees) * Vec3::X;
            let pipe = rotated(&open_tube(96, 60, 0.3, 1.2), degrees);
            for resolution in [48, 96] {
                let out = rebuilt(&pipe, resolution, [false; 3]);
                let what = format!("pipe along {along:.2} at {resolution}");
                assert_closed(&out.mesh, &what);
                assert_eq!(shells(&out.mesh), 1, "{what}");
                let volume = out.mesh.signed_volume() / rod;
                assert!((volume - 1.0).abs() < 0.005, "{what}: {volume} of a rod");
                let longest = out.mesh.positions.iter().map(|p| p.dot(along).abs()).fold(0.0, f32::max);
                assert!((longest - 0.6).abs() < 0.1 * out.cell, "{what}: ends at {longest}");
            }

            let ball = icosphere(5, 0.5, Vec3::ZERO);
            let pierced = without_near(&without_near(&ball, along * 0.5, 0.2), along * -0.5, 0.2);
            let out = rebuilt(&pierced, 64, [false; 3]);
            let what = format!("ball pierced along {along:.2}");
            assert_closed(&out.mesh, &what);
            assert_eq!(shells(&out.mesh), 1, "{what}");
            // The ball less two caps 0.04 high, give or take the ragged rims.
            let volume = out.mesh.signed_volume() / sphere_volume(0.5);
            assert!((0.985..0.995).contains(&volume), "{what}: {volume} of the ball");
            let reach = out.mesh.positions.iter().map(|p| p.dot(along).abs()).fold(0.0, f32::max);
            assert!((0.44..0.48).contains(&reach), "{what}: flats at {reach}");
        }
    }

    /// Triangles that share no vertices, as an STL file stores them, with a
    /// hole among them. The seams between them are not holes; the hole is.
    #[test]
    fn a_hole_in_a_mesh_of_unshared_vertices_is_still_just_the_hole() {
        let whole = icosphere(5, 0.5, Vec3::ZERO);
        let holed = without_near(&whole, Vec3::new(0.3, 0.3, 0.26).normalize() * 0.5, 0.05);
        let soup: Vec<Vec3> = holed.indices.iter().map(|&i| holed.positions[i as usize]).collect();
        let loose = TriMesh::new(soup, (0..holed.indices.len() as u32).collect());
        let (sound, out) = (rebuilt(&whole, 80, [false; 3]), rebuilt(&loose, 80, [false; 3]));
        assert_closed(&out.mesh, "holed soup");
        assert_eq!(shells(&out.mesh), 1);
        assert_eq!(out.mesh.triangle_count(), sound.mesh.triangle_count());
        assert!((out.mesh.signed_volume() / sound.mesh.signed_volume() - 1.0).abs() < 2e-3);
    }

    /// A tear with no rim to cap: a patch of the surface wound backwards, so
    /// that lines through it never come back to zero, with every edge still
    /// shared by two triangles. This is left to the vote between the axes,
    /// which keeps the damage to the patch: the sphere comes back whole.
    #[test]
    fn a_patch_wound_backwards_is_outvoted() {
        let centre = Vec3::new(0.02, 0.01, -0.015);
        let mut source = icosphere(5, 0.5, centre);
        let patch = centre + Vec3::new(0.5, 0.3, 0.2).normalize() * 0.5;
        let mut flipped = 0;
        for t in 0..source.triangle_count() {
            let [a, b, c] = source.triangle(t);
            if ((a + b + c) / 3.0).distance(patch) < 0.08 {
                source.indices.swap(t * 3 + 1, t * 3 + 2);
                flipped += 1;
            }
        }
        assert!(flipped > 30, "{flipped} triangles flipped");
        for resolution in [40, 90] {
            let out = rebuilt(&source, resolution, [false; 3]);
            let what = format!("flipped patch at {resolution}");
            assert_closed(&out.mesh, &what);
            assert_eq!(shells(&out.mesh), 1, "{what}");
            let volume = out.mesh.signed_volume() / sphere_volume(0.5);
            assert!((volume - 1.0).abs() < 0.01, "{what}: volume ratio {volume}");
            for p in &out.mesh.positions {
                let off = (p.distance(centre) - 0.5).abs();
                // Off the sphere only at the patch, and by no more than its size.
                assert!(off < 0.1 * out.cell || (p.distance(patch) < 0.2 && off < 0.1), "{what}: vertex {off} off");
            }
        }
    }

    #[test]
    fn small_holes_are_closed_over() {
        let mut source = icosphere(4, 0.5, Vec3::ZERO);
        source.indices.truncate(source.indices.len() - 3 * 20);
        let out = rebuilt(&source, 48, [false; 3]);
        assert_closed(&out.mesh, "holed sphere");
        assert!((out.mesh.signed_volume() / sphere_volume(0.5) - 1.0).abs() < 0.03);
    }

    #[test]
    fn things_that_cannot_be_remeshed_say_why() {
        let sphere = icosphere(2, 1.0, Vec3::ZERO);
        let try_with = |positions: &[Vec3], indices: &[u32]| remesh_with(positions, indices, &RemeshSettings::new(32));
        assert!(try_with(&[], &[]).is_err());
        assert!(try_with(&sphere.positions, &[]).is_err());
        assert!(try_with(&sphere.positions, &[0, 1]).is_err());
        assert!(try_with(&sphere.positions, &[0, 1, 9999]).is_err());
        assert!(try_with(&[Vec3::ONE; 3], &[0, 1, 2]).is_err());
        let mut broken = sphere.positions.clone();
        broken[5].y = f32::NAN;
        assert!(try_with(&broken, &sphere.indices).is_err());
        // A flat sheet has no inside.
        let sheet = [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::X + Vec3::Y];
        let error = try_with(&sheet, &[0, 1, 2, 2, 1, 3]).unwrap_err();
        assert!(error.contains("closed"), "{error}");
    }

    /// A small model a long way off two of its mirror planes, at the highest
    /// resolution the app's slider has. The lattice has to reach from the
    /// model to its mirror images: 53,254 samples a side at 25 model-lengths
    /// off, whose lines along z alone were an 11.3 GB table, asked for
    /// without a word. Now refused from the numbers, saying what to do. (At
    /// 40 lengths off the old code refused too, the lattice being longer than
    /// a lattice can be, but only with "too fine for this model".)
    #[test]
    fn a_model_far_from_two_mirror_planes_is_refused_with_what_to_do_about_it() {
        for (off, said) in [(40.0, "40"), (25.0, "25")] {
            let middle = Vec3::new(off, off, 0.0);
            let rivet = box_mesh(middle - 0.5, middle + 0.5);
            let error = remesh_with(&rivet.positions, &rivet.indices, &settings(1024, [true, true, false])).unwrap_err();
            let start = format!("The model is about {said} times its own size away from its X and Y mirror planes. ");
            assert!(error.starts_with(&start), "{error}");
            assert!(error.contains(" Turn the X and Y mirrors off for this, lower the resolution to "), "{error}");
            assert!(error.ends_with(" or less, or move its centre onto it with Set Origin to Mesh."), "{error}");
        }
    }

    /// A model wholly on the far side of a mirror plane has no resolution at
    /// which it can be rebuilt mirrored, so it is never offered one: a cube
    /// forty lengths off on the negative side at 1024 used to be told to
    /// "lower the resolution to 740 or less", and at 740 that it lay on the
    /// far side of its mirror plane. It is told that at once, at any
    /// resolution, and nothing is read or allocated first.
    #[test]
    fn a_model_on_the_far_side_of_its_mirror_plane_is_never_offered_a_resolution() {
        let far = box_mesh(Vec3::new(-40.5, -0.5, -0.5), Vec3::new(-39.5, 0.5, 0.5));
        // Touching the plane from behind is behind it: nothing of the model is on the side that is kept.
        let touching = box_mesh(Vec3::new(-1.0, -0.5, -0.5), Vec3::new(0.0, 0.5, 0.5));
        for (model, mirror) in [(&far, [true, false, false]), (&far, [true, true, false]), (&touching, [true; 3])] {
            for resolution in [MAX_RESOLUTION, 1024, 740, 64, MIN_RESOLUTION] {
                let error = admit(&model.positions, &model.indices, &settings(resolution, mirror)).unwrap_err();
                assert_eq!(error, FAR_SIDE, "at {resolution}, mirrored {mirror:?}");
            }
        }
        let error = remesh_with(&far.positions, &far.indices, &settings(740, [true, false, false])).unwrap_err();
        assert_eq!(error, FAR_SIDE);
        // On the side that is kept, as far off, it is the distance that is spoken of, with a resolution.
        let near = box_mesh(Vec3::new(39.5, -0.5, -0.5), Vec3::new(40.5, 0.5, 0.5));
        let error = admit(&near.positions, &near.indices, &settings(1024, [true, false, false])).unwrap_err();
        assert!(error.contains(", lower the resolution to "), "{error}");
        // And behind a plane that is not a mirror, there is nothing to say.
        assert!(admit(&far.positions, &far.indices, &settings(64, [false, true, true])).is_ok());
    }

    /// The resolution a refusal offers is one the same asking then lets
    /// through: models off one, two and three mirror planes, near and far,
    /// over the resolutions there are.
    #[test]
    fn the_resolution_a_refusal_offers_is_then_admitted() {
        let mut offers = 0;
        for off in [3.0, 10.0, 25.0, 40.0, 55.0] {
            for mirror in [[true, false, false], [true, true, false], [true; 3]] {
                let middle = Vec3::new(off, off * 0.7, off * 0.4);
                let rivet = box_mesh(middle - Vec3::new(0.5, 0.3, 0.2), middle + Vec3::new(0.5, 0.3, 0.2));
                for resolution in [MAX_RESOLUTION, 3000, 2048, 1024, 740, 512, 100, 17] {
                    let asked = |resolution: u32| admit(&rivet.positions, &rivet.indices, &settings(resolution, mirror));
                    let Err(error) = asked(resolution) else { continue };
                    let Some(offered) = crate::testing::offered_resolution(&error) else { continue };
                    offers += 1;
                    assert!(offered < resolution, "{error}");
                    let again = asked(offered);
                    let again = again.unwrap_or_else(|second| panic!("{off}, {mirror:?}: {error:?}, and then {second:?}"));
                    assert_eq!(again.resolution, offered);
                }
            }
        }
        assert!(offers > 20, "only {offers} refusals offered a resolution");
    }

    /// Off-centre is not refused for its own sake: ten lengths from one
    /// plane is a long thin lattice and nothing more.
    #[test]
    fn a_model_some_way_off_one_mirror_plane_is_still_rebuilt_with_its_twin() {
        let aside = icosphere(4, 0.5, Vec3::new(10.0, 0.2, -0.1));
        let out = rebuilt(&aside, 64, [true, false, false]);
        assert_closed(&out.mesh, "sphere ten lengths off its plane");
        assert_eq!(shells(&out.mesh), 2);
        assert!((out.mesh.signed_volume() / (2.0 * sphere_volume(0.5)) - 1.0).abs() < 0.01);
    }

    /// The app's highest setting on a model that sits on its mirror planes,
    /// start to finish: twelve million triangles. Slow for every run of the
    /// tests, so run by name; `budget`'s own tests check the sums for this
    /// and for the kernel's highest resolution.
    #[test]
    #[ignore = "seconds: cargo test -p sculpt-volume the_apps_highest_resolution -- --ignored"]
    fn the_apps_highest_resolution_is_still_accepted() {
        let cube = box_mesh(Vec3::splat(-0.5), Vec3::splat(0.5));
        let out = remesh_with(&cube.positions, &cube.indices, &settings(1024, [true; 3])).unwrap();
        assert_eq!(out.resolution, 1024);
        assert!(out.mesh.triangle_count() > 12_000_000, "{} triangles", out.mesh.triangle_count());
    }

    #[test]
    fn the_resolution_is_kept_in_range_and_within_budget() {
        // The lowest request there is, and the next few.
        for requested in [0, 1, 2, 3, 4] {
            assert_eq!(budgeted_resolution(1.0, 1.0, requested, MAX_TRIANGLES).0, MIN_RESOLUTION);
        }
        assert_eq!(budgeted_resolution(1.0, 1.0, 5, MAX_TRIANGLES).0, 5);
        assert_eq!(budgeted_resolution(1.0, 1.0, u32::MAX, usize::MAX).0, MAX_RESOLUTION);
        // No budget at all still leaves the smallest lattice.
        assert_eq!(budgeted_resolution(1.0, 1.0, 100, 0).0, MIN_RESOLUTION);
        assert_eq!(budgeted_resolution(1.0, 1.0, 100, 1).0, MIN_RESOLUTION);
        assert_eq!(budgeted_resolution(0.0, 0.0, 0, 0), (MIN_RESOLUTION, 0));

        let source = icosphere(4, 0.5, Vec3::ZERO);
        let lowest = remesh(&source, 0, [false; 3]).unwrap();
        assert_closed(&lowest, "sphere at the lowest resolution");

        let capped = RemeshSettings { max_triangles: 20_000, ..RemeshSettings::new(400) };
        let out = remesh_with(&source.positions, &source.indices, &capped).unwrap();
        assert!(out.resolution < 400 && out.resolution > 20, "resolution {}", out.resolution);
        let triangles = out.mesh.triangle_count();
        assert!(triangles <= 20_000 && triangles > 12_000, "{triangles} triangles");
    }

    /// For the mirror plane of `axis`: vertices with no exact twin, vertices
    /// on the plane, and vertices whose neighbours are not their twin's
    /// neighbours mirrored.
    fn symmetry_report(mesh: &TriMesh, axis: usize) -> (usize, usize, usize) {
        let bits = |p: Vec3| p.to_array().map(|v| (v + 0.0).to_bits());
        let at: HashMap<[u32; 3], u32> = mesh.positions.iter().enumerate().map(|(i, &p)| (bits(p), i as u32)).collect();
        assert_eq!(at.len(), mesh.vertex_count(), "two vertices in one place");
        let partner: Vec<Option<u32>> = mesh
            .positions
            .iter()
            .map(|&p| {
                let mut r = p;
                r[axis] = -r[axis];
                at.get(&bits(r)).copied()
            })
            .collect();
        let mut neighbours = vec![HashSet::new(); mesh.vertex_count()];
        for t in mesh.indices.chunks_exact(3) {
            for c in 0..3 {
                let (a, b) = (t[c], t[(c + 1) % 3]);
                neighbours[a as usize].insert(b);
                neighbours[b as usize].insert(a);
            }
        }
        let unmatched = partner.iter().filter(|p| p.is_none()).count();
        let on_plane = partner.iter().enumerate().filter(|&(i, p)| *p == Some(i as u32)).count();
        let mismatched = (0..mesh.vertex_count())
            .filter(|&i| {
                let Some(j) = partner[i] else { return false };
                neighbours[i].iter().any(|&a| !partner[a as usize].is_some_and(|m| neighbours[j as usize].contains(&m)))
            })
            .count();
        (unmatched, on_plane, mismatched)
    }

    /// Every stored normal must be unit length and within 60 degrees of the
    /// mesh's own area-weighted geometric normal, worked in doubles so this
    /// check has no small-number trap of its own. 60 is far outside anything
    /// a correct normal does on a sphere and far inside what a lost one does.
    fn assert_normals_follow_surface(out: &Remeshed, label: &str) {
        let mesh = &out.mesh;
        let mut sum = vec![glam::DVec3::ZERO; mesh.vertex_count()];
        for t in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize].as_dvec3());
            let n = (b - a).cross(c - a);
            t.iter().for_each(|&v| sum[v as usize] += n);
        }
        let (mut not_unit, mut deviating) = (0, 0);
        for (stored, geometric) in out.normals.iter().zip(&sum) {
            if (stored.as_dvec3().length() - 1.0).abs() > 1e-3 {
                not_unit += 1;
            } else if geometric.length() > 0.0 && stored.as_dvec3().dot(geometric.normalize()) < 0.5 {
                deviating += 1;
            }
        }
        assert_eq!((not_unit, deviating), (0, 0), "{label}: of {} normals", out.normals.len());
    }
}
