//! Builds a solid, closed shell from the masked region of a sculpt: the
//! "Extract" of other sculpting apps, used to pull armour plates, clothing and
//! similar surface-derived pieces off a body as their own object.
//!
//! Pure geometry: takes the source's positions, normals, triangles and mask and
//! returns a new mesh in the source's own local space, touching nothing. That
//! keeps it trivially re-runnable, which is what makes a live preview
//! possible: every settings change throws the previous result away and calls
//! this again.
//!
//! The result is watertight by construction: an outer surface (the region
//! pushed out along its normals), an inner surface (the region itself, wound
//! inside out), and a rim wall stitching the two along the region's boundary
//! edges. Each takes its winding from the source's own, so a source wound
//! counter-clockwise from outside gives a shell wound the same.

use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::TriMesh;

use crate::polygon::edge_key;

/// Tunables for `extract`. Lengths are fractions of the source object's own
/// size rather than local units: an extract that behaves identically on a
/// half-unit sphere and a 100-unit imported model is the difference between a
/// slider that can be learned and one that has to be found again every time.
/// They are multiplied by half the source's largest side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExtractSettings {
    /// Shell thickness.
    pub thickness_fraction: f32,
    /// Pushes the shell's inner surface off the source along the surface
    /// normal before extruding. Negative sinks it in, so a plate can be made
    /// to bite into the body rather than float on it.
    pub offset_fraction: f32,
    /// Mask value a triangle's three vertices must average to be included.
    /// A half by default, so a soft-edged mask extracts roughly where it looks
    /// half painted.
    pub mask_threshold: f32,
    /// Relaxation passes applied to the region's boundary vertices only, along
    /// the boundary itself. Straightens the stair-stepped edge a mask
    /// threshold inevitably leaves (the region can only ever be a whole number
    /// of triangles).
    pub border_smoothing: u32,
    /// Smoothing passes over the finished shell: rounds the hard rim and
    /// settles the surface.
    pub surface_smoothing: u32,
    /// 0 is uniform thickness everywhere; 1 tapers the thickness to nearly
    /// nothing where the mask fades out, a feathered edge instead of a slab
    /// with a square rim.
    pub falloff_amount: f32,
    /// How strongly the shell's inner surface is pulled back onto the source
    /// after smoothing. 1 keeps it exactly flush (the piece sits on the body
    /// like armour or cloth), 0 lets smoothing round it away. The outer
    /// surface stays fully smoothed either way.
    pub shrinkwrap: f32,
    /// Extracts the unmasked region instead. The mask marks what is protected
    /// from brushes, so "extract the masked part" is the natural default, but
    /// masking the part to keep and extracting everything else is common
    /// enough to be worth a switch.
    pub invert_region: bool,
}

impl Default for ExtractSettings {
    fn default() -> ExtractSettings {
        ExtractSettings {
            thickness_fraction: 0.06,
            offset_fraction: 0.0,
            mask_threshold: 0.5,
            border_smoothing: 4,
            surface_smoothing: 3,
            falloff_amount: 0.0,
            shrinkwrap: 1.0,
            invert_region: false,
        }
    }
}

/// Floor on the per-vertex thickness multiplier. With full falloff the taper
/// would otherwise reach exactly zero at the mask's edge, collapsing the rim
/// quads there into zero-area triangles, which shade as artefacts and break
/// the normals of everything touching them. A thin but real edge looks the
/// same and stays valid geometry.
const MIN_THICKNESS_FACTOR: f32 = 0.04;

/// The standard Taubin pair: a pass that shrinks, then a slightly larger
/// negative pass that pushes back out (see `smooth`).
const TAUBIN_LAMBDA: f32 = 0.5;
const TAUBIN_MU: f32 = -0.53;

/// How far a boundary vertex moves toward the middle of its two boundary
/// neighbours on each border-smoothing pass.
const BORDER_RELAXATION: f32 = 0.6;

/// Below this many items a parallel split costs more than it saves.
const MIN_PARALLEL: usize = 4096;

/// Extracts the masked region of a mesh as a closed shell.
///
/// `normals` and `mask` have one entry per vertex; the normals point out of
/// the surface and need not be unit length. Fails, with what to tell the user,
/// when there is nothing sensible to build.
pub fn extract(
    positions: &[Vec3],
    normals: &[Vec3],
    indices: &[u32],
    mask: &[f32],
    settings: &ExtractSettings,
) -> Result<TriMesh, String> {
    if positions.is_empty() || indices.len() < 3 || normals.len() != positions.len() || mask.len() != positions.len() {
        return Err("Mesh data not ready.".to_string());
    }
    let invert = settings.invert_region;
    let threshold = settings.mask_threshold.clamp(0.0, 1.0);
    let mask_at = |i: u32| if invert { 1.0 - mask[i as usize] } else { mask[i as usize] };

    // 1. Pick the region. Averaged over the triangle's three vertices rather
    // than requiring all three past the threshold: an all-three test erodes
    // the region by a full ring of triangles, so a mask painted right up to
    // where the edge is wanted would extract visibly short of it.
    let selected: Vec<u32> = indices
        .par_chunks_exact(3)
        .enumerate()
        .with_min_len(MIN_PARALLEL)
        .filter(|(_, t)| (mask_at(t[0]) + mask_at(t[1]) + mask_at(t[2])) / 3.0 >= threshold)
        .map(|(t, _)| t as u32)
        .collect();
    if selected.is_empty() {
        let why = if invert {
            "Everything is masked - nothing left to extract."
        } else {
            "Nothing masked - paint a mask first."
        };
        return Err(why.to_string());
    }

    // 2. Number the region's own vertices.
    let (region_to_source, region_tris) = compact_region(positions.len(), indices, &selected);
    let n = region_to_source.len();

    // 3. Base surface, extrusion directions and thickness.
    let extent = largest_extent(positions);
    let thickness = settings.thickness_fraction * extent;
    let offset = settings.offset_fraction * extent;
    let falloff = settings.falloff_amount.clamp(0.0, 1.0);
    let per_vertex: Vec<(Vec3, Vec3, f32)> = region_to_source
        .par_iter()
        .with_min_len(MIN_PARALLEL)
        .map(|&source| {
            // A zero normal would send the whole extrusion to a point; "no
            // direction" leaves that vertex flat against the base instead,
            // which the surface smoothing then pulls back into line.
            let direction = normals[source as usize].normalize_or_zero();
            let ramp =
                if threshold < 1.0 { ((mask_at(source) - threshold) / (1.0 - threshold)).clamp(0.0, 1.0) } else { 0.0 };
            let smooth = ramp * ramp * (3.0 - 2.0 * ramp);
            let factor = (1.0 + (smooth - 1.0) * falloff).max(MIN_THICKNESS_FACTOR);
            (positions[source as usize] + direction * offset, direction, factor)
        })
        .collect();
    let mut base: Vec<Vec3> = per_vertex.iter().map(|v| v.0).collect();

    // 4. The region's boundary, and its clean-up.
    let boundary = boundary_edges(&region_tris);
    smooth_boundary(&mut base, &boundary, settings.border_smoothing);

    // 5. Build the shell. The inner surface is vertices 0..n and the outer
    // n..2n, so a vertex and its extruded twin are always n apart.
    let mut verts = vec![Vec3::ZERO; n * 2];
    let (inner, outer) = verts.split_at_mut(n);
    inner.copy_from_slice(&base);
    outer
        .par_iter_mut()
        .zip(base.par_iter())
        .zip(per_vertex.par_iter())
        .with_min_len(MIN_PARALLEL)
        .for_each(|((out, &base), &(_, direction, factor))| *out = base + direction * (thickness * factor));
    let tris = shell_triangles(&region_tris, &boundary, n as u32);

    // 6. Polish.
    smooth(&mut verts, &tris, settings.surface_smoothing);
    // Pull the inner surface back onto the source. Smoothing is what rounds
    // the rim and settles the form, but it also drifts the inner face off the
    // body; pinning it back to where it was before smoothing (which already
    // includes the boundary relaxation, so that is not undone) keeps the
    // piece sitting flush while the outer face stays fully smoothed.
    let shrinkwrap = settings.shrinkwrap.clamp(0.0, 1.0);
    if shrinkwrap > 0.0 && settings.surface_smoothing > 0 {
        // Exactly back when fully pinned, not merely to within rounding.
        let pin = |v: Vec3, b: Vec3| if shrinkwrap >= 1.0 { b } else { v.lerp(b, shrinkwrap) };
        verts[..n].par_iter_mut().zip(base.par_iter()).with_min_len(MIN_PARALLEL).for_each(|(v, &b)| *v = pin(*v, b));
    }

    let mut shell = TriMesh::new(verts, tris);
    if thickness < 0.0 {
        // Extruded into the body: what was built as the outer surface is now
        // underneath, so the whole shell is inside out.
        shell.flip();
    }
    Ok(shell)
}

/// The vertices the selected triangles use, numbered in the order they appear
/// in the source, and the selected triangles in that numbering.
fn compact_region(vertex_count: usize, indices: &[u32], selected: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let corners = |t: u32| &indices[t as usize * 3..t as usize * 3 + 3];
    // Threads only ever store `true`, so the order they run in cannot matter.
    let used: Vec<AtomicBool> = (0..vertex_count).into_par_iter().map(|_| AtomicBool::new(false)).collect();
    selected.par_iter().with_min_len(MIN_PARALLEL).for_each(|&t| {
        for &i in corners(t) {
            used[i as usize].store(true, Ordering::Relaxed);
        }
    });

    const UNUSED: u32 = u32::MAX;
    let mut remap = vec![UNUSED; vertex_count];
    let mut region_to_source = Vec::new();
    for (i, flag) in used.iter().enumerate() {
        if flag.load(Ordering::Relaxed) {
            remap[i] = region_to_source.len() as u32;
            region_to_source.push(i as u32);
        }
    }
    let mut region_tris = vec![0u32; selected.len() * 3];
    region_tris.par_chunks_exact_mut(3).zip(selected.par_iter()).with_min_len(MIN_PARALLEL).for_each(|(out, &t)| {
        for (slot, &i) in out.iter_mut().zip(corners(t)) {
            *slot = remap[i as usize];
        }
    });
    (region_to_source, region_tris)
}

/// Half the source's largest side in its own local space: the "how big is this
/// object" every fraction-based setting is measured against.
fn largest_extent(positions: &[Vec3]) -> f32 {
    let (lo, hi) = positions
        .par_iter()
        .with_min_len(MIN_PARALLEL)
        .fold(|| (Vec3::MAX, Vec3::MIN), |(lo, hi), &p| (lo.min(p), hi.max(p)))
        .reduce(|| (Vec3::MAX, Vec3::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
    let extent = ((hi - lo) * 0.5).max_element();
    if extent > 1e-6 { extent } else { 1.0 }
}

/// Edges used by exactly one of the triangles: where the region stops. Each is
/// returned in the direction its one owning triangle walks it, which is what
/// the rim wall needs to wind itself correctly, and in triangle order.
fn boundary_edges(tris: &[u32]) -> Vec<(u32, u32)> {
    // Every edge with the triangle corner that walks it, sorted so the two
    // walkers of a shared edge sit side by side. A sort rather than a hash
    // map: at a million triangles the map is the slowest thing in an extract.
    let mut walkers: Vec<(u64, u32)> = tris
        .par_chunks_exact(3)
        .enumerate()
        .with_min_len(MIN_PARALLEL)
        .flat_map_iter(|(t, tri)| (0..3).map(move |k| (edge_key(tri[k], tri[(k + 1) % 3]), (t * 3 + k) as u32)))
        .collect();
    walkers.par_sort_unstable();
    let count = walkers.len();
    let mut alone: Vec<u32> = (0..count)
        .into_par_iter()
        .with_min_len(MIN_PARALLEL)
        .filter(|&i| {
            (i == 0 || walkers[i - 1].0 != walkers[i].0) && (i + 1 == count || walkers[i + 1].0 != walkers[i].0)
        })
        .map(|i| walkers[i].1)
        .collect();
    alone.par_sort_unstable();
    alone
        .iter()
        .map(|&corner| (tris[corner as usize], tris[(corner as usize / 3) * 3 + (corner as usize + 1) % 3]))
        .collect()
}

/// Relaxes boundary vertices toward the middle of their boundary neighbours,
/// along the boundary only. This is what turns a stair-stepped mask edge into
/// a clean one: the region can only ever be a whole number of triangles, so
/// its raw edge zigzags at the scale of the mesh however carefully the mask
/// was painted, and no amount of whole-surface smoothing fixes that without
/// also destroying the shape.
///
/// Every new position is computed from the old ones and then swapped in,
/// rather than updated in place: updating in place makes the result depend on
/// vertex order, which would make the same mask smooth differently on a
/// remeshed copy of the same sculpt.
fn smooth_boundary(positions: &mut [Vec3], boundary: &[(u32, u32)], iterations: u32) {
    if boundary.is_empty() || iterations == 0 {
        return;
    }
    // Each boundary vertex and the vertices it shares a boundary edge with.
    let mut links: Vec<(u32, u32)> = boundary.iter().flat_map(|&(u, v)| [(u, v), (v, u)]).collect();
    links.sort_unstable();
    let mut next = Vec::with_capacity(links.len());
    for _ in 0..iterations {
        next.clear();
        for run in links.chunk_by(|a, b| a.0 == b.0) {
            let here = positions[run[0].0 as usize];
            let mean = run.iter().map(|&(_, other)| positions[other as usize]).sum::<Vec3>() / run.len() as f32;
            next.push((run[0].0, here.lerp(mean, BORDER_RELAXATION)));
        }
        for &(vertex, position) in &next {
            positions[vertex as usize] = position;
        }
    }
}

/// The shell's triangles: the outer surface, the inner surface and the wall
/// round the rim.
fn shell_triangles(region_tris: &[u32], boundary: &[(u32, u32)], n: u32) -> Vec<u32> {
    let region = region_tris.len();
    let mut tris = vec![0u32; region * 2 + boundary.len() * 6];
    let (outer, rest) = tris.split_at_mut(region);
    let (inner, wall) = rest.split_at_mut(region);
    // Outer surface: the source's winding, so it faces away from the body
    // exactly as the source surface did.
    outer.par_iter_mut().zip(region_tris.par_iter()).with_min_len(MIN_PARALLEL).for_each(|(out, &i)| *out = n + i);
    // Inner surface: the source's winding reversed, so it faces back toward
    // the body.
    inner.par_chunks_exact_mut(3).zip(region_tris.par_chunks_exact(3)).with_min_len(MIN_PARALLEL).for_each(
        |(out, t)| {
            out.copy_from_slice(&[t[0], t[2], t[1]]);
        },
    );
    // Rim wall. Each boundary edge runs the way its one owning triangle walks
    // it, so the outer surface walks it the same way and the inner surface the
    // other: the wall must do the opposite of each, which also faces it away
    // from the region rather than tucked inside the shell.
    for (out, &(u, v)) in wall.chunks_exact_mut(6).zip(boundary) {
        out.copy_from_slice(&[n + u, u, v, n + u, v, n + v]);
    }
    tris
}

/// Taubin smoothing (a shrinking pass followed by a larger negative pass), not
/// plain Laplacian. Plain Laplacian shrinks whatever it smooths, which on a
/// thin shell is actively destructive: the inner and outer surfaces are only a
/// few percent of the object apart, so a handful of shrinking passes visibly
/// eats the thickness just dialled in. The negative pass pushes back out at a
/// slightly larger rate, leaving the shape (and therefore the thickness) where
/// it was while still removing the rim's hardness and the faceting.
fn smooth(verts: &mut Vec<Vec3>, tris: &[u32], iterations: u32) {
    if iterations == 0 {
        return;
    }
    let (offsets, neighbours) = vertex_neighbours(verts.len(), tris);
    let mut scratch = vec![Vec3::ZERO; verts.len()];
    for _ in 0..iterations {
        for factor in [TAUBIN_LAMBDA, TAUBIN_MU] {
            scratch.par_iter_mut().enumerate().with_min_len(MIN_PARALLEL).for_each(|(i, out)| {
                let around = &neighbours[offsets[i] as usize..offsets[i + 1] as usize];
                *out = if around.is_empty() {
                    verts[i]
                } else {
                    let mean = around.iter().map(|&j| verts[j as usize]).sum::<Vec3>() / around.len() as f32;
                    verts[i] + (mean - verts[i]) * factor
                };
            });
            std::mem::swap(verts, &mut scratch);
        }
    }
}

/// Each vertex's neighbours along mesh edges, as one flat list:
/// `neighbours[offsets[i]..offsets[i + 1]]` are vertex `i`'s. Built from the
/// sorted, de-duplicated list of directed edges, so there is one allocation
/// per list rather than one per vertex; a preview rebuild runs several times a
/// second while a slider is dragged.
fn vertex_neighbours(vertex_count: usize, tris: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut directed: Vec<u64> = tris
        .par_chunks_exact(3)
        .with_min_len(MIN_PARALLEL)
        .flat_map_iter(|t| {
            (0..3).flat_map(move |k| {
                let (a, b) = (t[k] as u64, t[(k + 1) % 3] as u64);
                [(a << 32) | b, (b << 32) | a]
            })
        })
        .collect();
    directed.par_sort_unstable();
    directed.dedup();

    let mut offsets = vec![0u32; vertex_count + 1];
    for &edge in &directed {
        offsets[(edge >> 32) as usize + 1] += 1;
    }
    for i in 0..vertex_count {
        offsets[i + 1] += offsets[i];
    }
    let neighbours = directed.par_iter().with_min_len(MIN_PARALLEL).map(|&edge| edge as u32).collect();
    (offsets, neighbours)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_closed, assert_near, closed_error, cube_sphere, icosphere};
    use std::time::Instant;

    /// No smoothing of any kind: the shell is exactly the region and its copy.
    fn plain(thickness: f32) -> ExtractSettings {
        ExtractSettings {
            thickness_fraction: thickness,
            border_smoothing: 0,
            surface_smoothing: 0,
            ..ExtractSettings::default()
        }
    }

    /// The triangles `extract` should pick, straight from the definition.
    fn picked(mesh: &TriMesh, mask: &[f32], threshold: f32) -> Vec<usize> {
        (0..mesh.triangle_count())
            .filter(|&t| {
                mesh.indices[t * 3..t * 3 + 3].iter().map(|&i| mask[i as usize]).sum::<f32>() / 3.0 >= threshold
            })
            .collect()
    }

    /// Volume of the cone from the origin to each of the given triangles.
    fn cone_volume(mesh: &TriMesh, triangles: &[usize]) -> f64 {
        triangles
            .iter()
            .map(|&t| {
                let [a, b, c] = mesh.triangle(t).map(|p| p.as_dvec3());
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    fn cap_mask(mesh: &TriMesh) -> Vec<f32> {
        mesh.positions.iter().map(|p| ((p.y - 0.2) * 4.0).clamp(0.0, 1.0)).collect()
    }

    #[test]
    fn a_masked_cap_becomes_a_closed_shell_of_the_right_volume() {
        // A unit sphere, so normals are positions and the largest extent is 1:
        // a shell of thickness t over a region is exactly the region's cone
        // from the centre scaled up by (1 + t), less the cone itself.
        let sphere = icosphere(4);
        let mask = cap_mask(&sphere);
        let region = picked(&sphere, &mask, 0.5);
        assert!(region.len() > 200 && region.len() < sphere.triangle_count() / 2);
        let cone = cone_volume(&sphere, &region);

        let shell = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &plain(0.1)).unwrap();
        assert_closed(&shell, "shell");
        assert_near(shell.signed_volume(), cone * (1.1f64.powi(3) - 1.0), 1e-4, "shell volume");
        let region_vertices = shell.vertex_count() / 2;
        assert!(shell.triangle_count() > region.len() * 2, "two surfaces and a rim wall");
        // The inner surface is the source's own vertices; the outer is the
        // same vertices pushed straight out.
        for i in 0..region_vertices {
            assert!((shell.positions[i].length() - 1.0).abs() < 1e-6);
            assert!(shell.positions[region_vertices + i].distance(shell.positions[i] * 1.1) < 1e-5);
        }

        // Lifted off the surface first, the same cone between two other radii.
        let lifted = ExtractSettings { offset_fraction: 0.05, ..plain(0.1) };
        let shell = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &lifted).unwrap();
        assert_closed(&shell, "lifted shell");
        assert_near(shell.signed_volume(), cone * (1.15f64.powi(3) - 1.05f64.powi(3)), 1e-4, "lifted shell volume");

        // Extruded inward it is still wound outward.
        let sunk = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &plain(-0.1)).unwrap();
        assert_closed(&sunk, "sunk shell");
        assert_near(sunk.signed_volume(), cone * (1.0 - 0.9f64.powi(3)), 1e-4, "sunk shell volume");
    }

    #[test]
    fn inverting_takes_the_rest_and_the_two_shells_make_the_whole() {
        let sphere = icosphere(4);
        // Hard-edged, so every triangle is wholly one side or straddles it.
        let mask: Vec<f32> = sphere.positions.iter().map(|p| if p.y > 0.3 { 1.0 } else { 0.0 }).collect();
        let settings = plain(0.08);
        let masked = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &settings).unwrap();
        let rest = ExtractSettings { invert_region: true, mask_threshold: 0.51, ..settings };
        let rest = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &rest).unwrap();
        assert_closed(&masked, "masked shell");
        assert_closed(&rest, "the rest");
        // A triangle averages at least a half one way or more than a half the
        // other, never both, so between them the two regions are the sphere.
        let all: Vec<usize> = (0..sphere.triangle_count()).collect();
        let whole = cone_volume(&sphere, &all) * (1.08f64.powi(3) - 1.0);
        assert_near(masked.signed_volume() + rest.signed_volume(), whole, 1e-4, "both shells together");
        // And they stop at the same edges, so their rim walls are as long.
        let inside = picked(&sphere, &mask, 0.5).len();
        let wall = masked.triangle_count() - 2 * inside;
        assert!(wall > 0);
        assert_eq!(rest.triangle_count() - 2 * (sphere.triangle_count() - inside), wall);

        // Everything masked: two whole spheres, one inside the other.
        let ones = vec![1.0; sphere.vertex_count()];
        let hollow = extract(&sphere.positions, &sphere.positions, &sphere.indices, &ones, &settings).unwrap();
        assert_closed(&hollow, "hollow sphere");
        assert_eq!(hollow.triangle_count(), 2 * sphere.triangle_count());
        assert_near(hollow.signed_volume(), whole, 1e-4, "hollow sphere volume");
    }

    #[test]
    fn nothing_to_extract_says_so() {
        let sphere = icosphere(2);
        let (p, i) = (&sphere.positions, &sphere.indices);
        let zeros = vec![0.0; sphere.vertex_count()];
        let ones = vec![1.0; sphere.vertex_count()];
        let settings = ExtractSettings::default();
        assert_eq!(extract(p, p, i, &zeros, &settings).unwrap_err(), "Nothing masked - paint a mask first.");
        let inverted = ExtractSettings { invert_region: true, ..settings };
        assert_eq!(extract(p, p, i, &ones, &inverted).unwrap_err(), "Everything is masked - nothing left to extract.");
        assert_eq!(extract(p, &p[1..], i, &ones, &settings).unwrap_err(), "Mesh data not ready.");
        assert_eq!(extract(p, p, i, &ones[1..], &settings).unwrap_err(), "Mesh data not ready.");
        assert_eq!(extract(&[], &[], &[], &[], &settings).unwrap_err(), "Mesh data not ready.");
        assert!(extract(p, p, i, &ones, &settings).is_ok());
    }

    #[test]
    fn falloff_feathers_the_edge_without_collapsing_it() {
        let sphere = icosphere(4);
        let mask = cap_mask(&sphere);
        let slab = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &plain(0.1)).unwrap();
        let feathered = ExtractSettings { falloff_amount: 1.0, ..plain(0.1) };
        let feathered = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &feathered).unwrap();
        assert_closed(&feathered, "feathered shell");
        assert!(feathered.signed_volume() < slab.signed_volume() * 0.95);
        assert!(feathered.signed_volume() > slab.signed_volume() * 0.3);
        // Full thickness where the mask is full, a sliver of it (but never
        // nothing) where the mask only just passes the threshold.
        let n = feathered.vertex_count() / 2;
        let thickness: Vec<f32> =
            (0..n).map(|i| feathered.positions[n + i].length() - feathered.positions[i].length()).collect();
        let (thinnest, thickest) = thickness.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &t| (lo.min(t), hi.max(t)));
        assert!((thickest - 0.1).abs() < 1e-5, "thickest {thickest}");
        assert!((0.1 * MIN_THICKNESS_FACTOR - 1e-6..0.02).contains(&thinnest), "thinnest {thinnest}");
    }

    /// Total length of the edges only one triangle of the first `count` uses.
    fn rim_length(shell: &TriMesh, region_triangles: usize) -> f32 {
        let outer = &shell.indices[..region_triangles * 3];
        boundary_edges(outer)
            .iter()
            .map(|&(u, v)| shell.positions[u as usize].distance(shell.positions[v as usize]))
            .sum()
    }

    #[test]
    fn smoothing_straightens_the_rim_and_keeps_the_shell_sound() {
        let sphere = icosphere(4);
        let mask = cap_mask(&sphere);
        let region = picked(&sphere, &mask, 0.5).len();
        let rough = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &plain(0.1)).unwrap();

        // Border smoothing alone moves only the rim, and shortens it: the
        // region's raw edge zigzags along triangle edges.
        let bordered = ExtractSettings { border_smoothing: 4, ..plain(0.1) };
        let bordered = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &bordered).unwrap();
        assert_closed(&bordered, "bordered shell");
        assert!(rim_length(&bordered, region) < rim_length(&rough, region) * 0.97);
        let n = rough.vertex_count() / 2;
        let moved = (0..n).filter(|&i| rough.positions[i] != bordered.positions[i]).count();
        assert!(moved > 10 && moved < n / 3, "{moved} of {n} inner vertices moved");

        // The full default polish: still closed, about the same volume (Taubin
        // smoothing does not eat the thickness), and with full shrinkwrap the
        // inner surface exactly where border smoothing left it.
        let polished = ExtractSettings { thickness_fraction: 0.1, ..ExtractSettings::default() };
        let polished = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &polished).unwrap();
        assert_closed(&polished, "polished shell");
        let share = polished.signed_volume() / rough.signed_volume();
        assert!(share > 0.8 && share < 1.1, "polished volume is {share} of the rough one");
        assert!((0..n).all(|i| polished.positions[i] == bordered.positions[i]));
        assert!(
            (0..n).any(|i| polished.positions[n + i].distance(bordered.positions[n + i]) > 1e-4),
            "the outer surface was smoothed"
        );

        // Let go of, the inner surface drifts with the smoothing.
        let loose = ExtractSettings { thickness_fraction: 0.1, shrinkwrap: 0.0, ..ExtractSettings::default() };
        let loose = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &loose).unwrap();
        assert!(closed_error(&loose).is_none());
        assert!((0..n).any(|i| loose.positions[i].distance(bordered.positions[i]) > 1e-4));
    }

    #[test]
    fn neighbour_lists_hold_each_edge_once_each_way() {
        // Two triangles sharing an edge, and a vertex nothing uses.
        let (offsets, neighbours) = vertex_neighbours(5, &[0, 1, 2, 2, 1, 3]);
        let of = |i: usize| neighbours[offsets[i] as usize..offsets[i + 1] as usize].to_vec();
        assert_eq!((of(0), of(1), of(2), of(3), of(4)), (vec![1, 2], vec![0, 2, 3], vec![0, 1, 3], vec![1, 2], vec![]));
        assert_eq!(boundary_edges(&[0, 1, 2, 2, 1, 3]), vec![(0, 1), (2, 0), (1, 3), (3, 2)]);
    }

    /// Timing: `cargo test -p sculpt-cut --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_extract_from_a_million_triangle_sphere() {
        let sphere = cube_sphere(290);
        let mask: Vec<f32> = sphere.positions.iter().map(|p| ((p.y + 0.1) * 6.0).clamp(0.0, 1.0)).collect();
        println!("{} triangles, {} threads", sphere.triangle_count(), rayon::current_num_threads());
        for (name, settings) in [("no smoothing", plain(0.06)), ("default settings", ExtractSettings::default())] {
            extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &settings).unwrap();
            let start = Instant::now();
            let shell = extract(&sphere.positions, &sphere.positions, &sphere.indices, &mask, &settings).unwrap();
            let elapsed = start.elapsed();
            assert_closed(&shell, name);
            println!(
                "extract, {name}: {:.1} ms for a shell of {} triangles",
                elapsed.as_secs_f64() * 1e3,
                shell.triangle_count()
            );
        }
    }
}
