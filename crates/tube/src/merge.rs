//! Fuses tubes into a model: one watertight surface where the tube grows out
//! of the body, with a smooth fillet at the join.
//!
//! Three steps, each on machinery the app already trusts:
//!
//!   1. The tubes' own field is meshed into a clean closed surface
//!      ([`TubeField::voxelize`]).
//!   2. That mesh and the model are put in one triangle list and remeshed
//!      together (`sculpt_volume::remesh_with`). Overlapping closed shells
//!      come out of a remesh as their union, the property Join + Remesh
//!      relies on, and its cost follows the surface rather than the volume
//!      even for a dense model.
//!   3. The seam is blended ([`blend`]): vertices near where the two surfaces
//!      meet are pushed out onto the smooth union of the two distance fields.
//!      A smooth union cannot be done inside the remesh, which works on the
//!      joined triangles, but it can be done after: on the merged surface
//!      both distances are known and non-negative, and the fillet is exactly
//!      where they are both small.
//!
//! The whole model is resampled at the remesh cell size (the price of a voxel
//! merge, the same one Remesh and Boolean pay). [`plan`] picks a resolution
//! that matches the model's current density so nothing is visibly lost.

use std::collections::HashMap;

use glam::{Mat3, Mat4, Vec3};
use rayon::prelude::*;
use sculpt_geom::{Aabb, TriMesh};
use sculpt_volume::{RemeshSettings, TriangleBvh, remesh, remesh_with};

use crate::field::{FAR, Lookup, MAX_VOXEL_SAMPLES, TubeField};

/// Lattices that mesh a tube on its own (the preview, Keep Separate, and the
/// tube mesh a merge feeds the remesh) never sample coarser than this many
/// cells across the thinnest tube radius worth keeping, and never keep less
/// of a tube than [`RESOLVED_TIP_FRACTION`] of its radius. The merge's final
/// remesh ignores both: it keeps the model's density (see [`plan`]).
pub const CELLS_ACROSS_RADIUS: f32 = 2.5;
pub const RESOLVED_TIP_FRACTION: f32 = 0.3;

/// Fewest cells along the longest side a merge remeshes at.
const MIN_RESOLUTION: u32 = 16;

/// No mirror partner.
const NONE: u32 = u32::MAX;

/// The part of a merge that is under way, for a progress line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Meshing the tubes on their own.
    Tubes,
    /// Remeshing the tubes and the model into one surface.
    Union,
    /// Filleting the join.
    Blend,
}

/// How a merge will sample the model and the tubes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MergePlan {
    /// Cells along the longest side of the merged model.
    pub resolution: u32,
    /// Cell size the tubes are meshed at before the remesh, in field units.
    pub tube_cell: f32,
    /// The model's own density as a cell size in field units (see
    /// [`density_cell`]): what the merge aims to keep.
    pub cell: f32,
    /// The cell the remesh will really use, in field units: the smallest
    /// feature that survives it. Parts of a tube thinner than this are
    /// merged this thick.
    pub remesh_cell: f32,
    /// A tube is thinner than the model's detail, so it cannot come out as
    /// thin as it was drawn.
    pub too_thin: bool,
}

/// World units per local unit of a matrix, averaged over its axes.
fn mean_scale(m: &Mat4) -> f32 {
    (m.x_axis.truncate().length() + m.y_axis.truncate().length() + m.z_axis.truncate().length()) / 3.0
}

/// Triangles a voxel remesh makes per cell-sized square of surface: the
/// figure `sculpt_volume` budgets its own remeshes with.
const TRIANGLES_PER_CELL_AREA: f64 = 3.0;

/// The cell size at which a voxel remesh gives a model back with about the
/// triangles it has now: the density a merge should keep.
///
/// From the model's area and triangle count rather than its mean edge
/// length, which the Unity app used. Edge length reads a mesh of even
/// triangles as coarser than it is and a mesh that came out of a remesh
/// (most sculpts) as finer, so merging at it added or dropped a third of
/// the model's triangles every time. At this cell a model that is merged
/// into again and again keeps the detail it has.
pub fn density_cell(positions: &[Vec3], indices: &[u32]) -> f32 {
    let triangles = indices.len() / 3;
    if triangles == 0 {
        return 0.0;
    }
    let area: f64 = indices
        .par_chunks_exact(3)
        .map(|t| {
            let corner = |c: usize| positions.get(t[c] as usize).map_or(glam::DVec3::ZERO, |p| p.as_dvec3());
            let (a, b, c) = (corner(0), corner(1), corner(2));
            0.5 * (b - a).cross(c - a).length()
        })
        .sum();
    (TRIANGLES_PER_CELL_AREA * area / triangles as f64).sqrt() as f32
}

/// Chooses the resolution of a merge.
///
/// The model keeps its density: the remesh runs at the cell size that gives
/// it back about the triangles it has, whatever the tubes are. A tapered tip finer than that rounds off;
/// refining the whole model to resolve it tripled a test sphere's triangle
/// count. A tube thinner than a cell or so cannot be held by the lattice at
/// all; it is merged one cell thick (see [`TubeField::thickened`]) and that
/// is reported (`too_thin`).
///
/// `target_to_field` takes the model's own space to the space the tubes were
/// built in. `max_radius` and `thinnest_radius` are the largest and smallest
/// Radius setting among the tubes.
pub fn plan(
    positions: &[Vec3],
    indices: &[u32],
    target_to_field: &Mat4,
    tubes: &TubeField,
    max_radius: f32,
    thinnest_radius: f32,
) -> MergePlan {
    let scale = mean_scale(target_to_field);
    let cell = density_cell(positions, indices) * scale;
    let tube_floor = tubes.min_radius().max(max_radius * RESOLVED_TIP_FRACTION);

    // The model's box with the tubes' added, in the model's own space.
    let mut local = Aabb::of(positions);
    let field_to_target = target_to_field.inverse();
    let tube_bounds = tubes.bounds();
    if !tube_bounds.is_empty() {
        for corner in 0..8 {
            let pick = |axis: usize| if corner >> axis & 1 == 0 { tube_bounds.min[axis] } else { tube_bounds.max[axis] };
            local.grow(field_to_target.transform_point3(Vec3::new(pick(0), pick(1), pick(2))));
        }
    }
    let extent = local.size().max_element() * scale;
    let wanted = (extent / cell.max(1e-6)).ceil().clamp(MIN_RESOLUTION as f32, remesh::MAX_RESOLUTION as f32);
    let resolution = (wanted as u32).clamp(MIN_RESOLUTION, remesh::MAX_RESOLUTION);
    // The tubes are meshed a little finer than the remesh that follows (and
    // fine enough for the tubes themselves), so the remesh loses nothing to
    // them. Only the tubes' own small lattice pays for that.
    let tube_cell = (extent / resolution as f32 * 0.7).min(tube_floor / CELLS_ACROSS_RADIUS);
    let remesh_cell = extent / resolution as f32;
    MergePlan { resolution, tube_cell, cell, remesh_cell, too_thin: thinnest_radius < cell.max(remesh_cell) }
}

/// Voxel size for a tube kept as its own object: the anchor's own density
/// where there is one (`anchor_cell`, its [`density_cell`] in field units),
/// so the tube sculpts like the body it will likely be joined to, bounded to
/// a sensible number of cells across the tube.
pub fn separate_cell(radius: f32, anchor_cell: Option<f32>, tubes: &TubeField) -> f32 {
    let cell = match anchor_cell {
        Some(cell) => cell.clamp(radius / 12.0, radius / 3.0),
        None => radius / 6.0,
    };
    let tube_floor = tubes.min_radius().max(radius * RESOLVED_TIP_FRACTION);
    cell.min(tube_floor / CELLS_ACROSS_RADIUS)
}

/// Merges `tubes` into `target`.
///
/// The tubes are in whatever space the caller built them in (the tool uses
/// the model's position and rotation without its scale), the target in its
/// own space, with `target_to_field` taking one to the other. The result is
/// in the target's own space. `blend` is the fillet size in field units and
/// `mirror` the coordinate planes the tubes were mirrored across: where the
/// model is symmetric across them too, so is the result, exactly.
///
/// `stage` is told what is starting; returning false from it cancels.
pub fn merge(
    target: &TriMesh,
    target_to_field: &Mat4,
    tubes: &TubeField,
    plan: &MergePlan,
    blend_size: f32,
    mirror: [bool; 3],
    stage: &dyn Fn(Stage) -> bool,
) -> Result<TriMesh, String> {
    let cancelled = || Err::<TriMesh, String>("Cancelled.".into());
    if target.positions.is_empty() || target.indices.len() < 3 {
        return Err("The object has no geometry.".into());
    }
    if tubes.is_empty() {
        return Err("There is no tube to merge.".into());
    }
    if !stage(Stage::Tubes) {
        return cancelled();
    }
    // What the remesh cannot hold it would break into droplets: a tapered
    // tip rounds off at the model's own detail instead.
    let thick = tubes.thickened(plan.remesh_cell);
    let tubes = &thick;
    let mut tube = tubes.voxelize(plan.tube_cell, mirror, MAX_VOXEL_SAMPLES)?.surface.into_mesh();

    // Into the target's own space, where the remesh has to run: its lattice
    // is laid out about that space's origin, which is where the model's
    // mirror planes are, so a symmetric model stays symmetric.
    tube.transform(&target_to_field.inverse());
    let mut joined = target.clone();
    joined.append(&tube);
    drop(tube);

    if !stage(Stage::Union) {
        return cancelled();
    }
    // The field's space differs from the target's by scale at most, so the
    // planes the tubes were mirrored across are the target's too.
    let settings = RemeshSettings { mirror, ..RemeshSettings::new(plan.resolution) };
    let mut merged = remesh_with(&joined.positions, &joined.indices, &settings)?.mesh;
    drop(joined);

    if blend_size > 0.0 {
        if !stage(Stage::Blend) {
            return cancelled();
        }
        blend(&mut merged, target, target_to_field, tubes, blend_size, mirror);
    }
    Ok(merged)
}

/// Moves the merged surface onto the smooth union of the model and the tubes
/// near where they meet. Returns how many vertices were moved.
///
/// On the union's surface both distances are at least zero and one of them
/// is zero. The quadratic smooth minimum equals the plain minimum wherever
/// the two are more than `k` apart, so everything away from the seam stays
/// exactly where the remesh put it, and bulges out up to `k / 4` at the seam.
/// Each vertex takes a few Newton steps toward the smooth surface, then the
/// moved band is relaxed along the surface so the stretch is spread out.
pub fn blend(
    mesh: &mut TriMesh,
    target: &TriMesh,
    target_to_field: &Mat4,
    tubes: &TubeField,
    k: f32,
    mirror: [bool; 3],
) -> usize {
    // Field units per local unit: exact for an evenly scaled model (the
    // usual case); for a squashed one the fillet is just a little uneven.
    let scale = mean_scale(target_to_field);
    let frames = Frames {
        to_field: *target_to_field,
        to_target: target_to_field.inverse(),
        normal_to_field: Mat3::from_mat4(target_to_field.inverse()).transpose(),
        normal_to_target: Mat3::from_mat4(*target_to_field).transpose(),
        scale,
    };
    let lookup = tubes.lookup(k * 1.5);
    let bvh = TriangleBvh::build(&target.positions, &target.indices);

    // The band: vertices near a tube (the cheap test first) and near the
    // original model. A little wider than the fillet itself, so the relax
    // has room: a vertex just outside it projects to where it already is,
    // but can still be slid along.
    let near: Vec<u32> = (0..mesh.positions.len() as u32)
        .into_par_iter()
        .filter(|&i| lookup.evaluate(frames.to_field.transform_point3(mesh.positions[i as usize])) < k * 1.5)
        .collect();
    let mut wanted: Vec<bool> = near
        .par_iter()
        .map(|&i| {
            let p = mesh.positions[i as usize];
            let b = lookup.evaluate(frames.to_field.transform_point3(p)).abs();
            // |a - b| < 1.3 k needs a < b + 1.3 k: no need to look farther.
            let limit = (b + k * 1.3) / scale.max(1e-12);
            bvh.closest_within(p, limit).is_some_and(|c| (c.distance() * scale - b).abs() < k * 1.3)
        })
        .collect();
    // A vertex and its mirror image are either both in the band or neither,
    // whatever rounding said about each.
    for axis in (0..3).filter(|&a| mirror[a]) {
        let partners = mirror_partners(&mesh.positions, &near, axis);
        for slot in 0..near.len() {
            let twin = partners[slot];
            if twin != NONE && (wanted[slot] || wanted[twin as usize]) {
                wanted[slot] = true;
                wanted[twin as usize] = true;
            }
        }
    }
    let band: Vec<u32> = near.iter().zip(&wanted).filter(|(_, wanted)| **wanted).map(|(&i, _)| i).collect();
    if band.is_empty() {
        return 0;
    }
    // Twins are found by exact position, so before anything moves.
    let partners: Vec<Vec<u32>> =
        (0..3).map(|axis| if mirror[axis] { mirror_partners(&mesh.positions, &band, axis) } else { Vec::new() }).collect();

    let mut slot_of = vec![NONE; mesh.positions.len()];
    for (slot, &i) in band.iter().enumerate() {
        slot_of[i as usize] = slot as u32;
    }
    // Surface normal per band vertex, in the target's space. They get the
    // smooth union's own gradient as they are projected, which is what the
    // relax slides them along.
    let mut normals = band_normals(mesh, &slot_of, band.len());
    let band_triangles: Vec<[u32; 3]> = mesh
        .indices
        .par_chunks_exact(3)
        .filter(|t| t.iter().any(|&v| slot_of[v as usize] != NONE))
        .map(|t| [t[0], t[1], t[2]])
        .collect();

    let probe = k * 0.05;
    let project = |positions: &mut [Vec3], normals: &mut [Vec3]| {
        let moved: Vec<Option<(Vec3, Vec3)>> = band
            .par_iter()
            .map(|&i| project_vertex(positions[i as usize], &frames, &lookup, &bvh, k, probe))
            .collect();
        for (slot, moved) in moved.into_iter().enumerate() {
            if let Some((position, normal)) = moved {
                positions[band[slot] as usize] = position;
                if normal != Vec3::ZERO {
                    normals[slot] = normal;
                }
            }
        }
    };

    // Projection alone crowds the band: nearest points on a concave fillet
    // are closer together than the vertices they came from. Alternating
    // short relax passes with re-projection spreads them back out over the
    // fillet while keeping them on it.
    for _ in 0..4 {
        project(&mut mesh.positions, &mut normals);
    }
    for _ in 0..8 {
        relax(&mut mesh.positions, &band_triangles, &normals, &slot_of, &band);
        project(&mut mesh.positions, &mut normals);
    }

    for axis in (0..3).filter(|&a| mirror[a]) {
        make_symmetric(&mut mesh.positions, &band, &partners[axis], axis);
    }
    band.len()
}

/// The matrices a blend moves points and normals between spaces with.
struct Frames {
    to_field: Mat4,
    to_target: Mat4,
    normal_to_field: Mat3,
    normal_to_target: Mat3,
    scale: f32,
}

/// One Newton step of a band vertex toward the smooth union's surface: its
/// new position and the surface normal there, both in the target's space.
fn project_vertex(p: Vec3, frames: &Frames, tubes: &Lookup, bvh: &TriangleBvh, k: f32, probe: f32) -> Option<(Vec3, Vec3)> {
    let w = frames.to_field.transform_point3(p);
    let b = tubes.evaluate(w);
    if b >= FAR {
        return None;
    }
    // Signed distance to the model and its gradient. The sign matters: with
    // an unsigned distance the smooth union's zero set has a second sheet
    // just inside the model around the tube's root, and the crease vertices
    // sink onto it, leaving a crater round the join instead of a fillet. The
    // sign is from the nearest face's normal, which is exact wherever the
    // nearest point is inside a face and only guesses at a sharp edge, never
    // the case on a sculpt this dense.
    let nearest = bvh.closest(p)?;
    let face = bvh.triangle_normal(nearest.triangle);
    let away = p - nearest.point;
    let side = if away.dot(face) < 0.0 { -1.0 } else { 1.0 };
    let toward = if away.length_squared() > 1e-20 { away * side } else { face };
    let ga = (frames.normal_to_field * toward).try_normalize()?;
    let a = nearest.distance() * frames.scale * side;

    // The tubes' gradient by central differences: the field is exact and
    // cheap.
    let gb = tubes.gradient(w, probe)?.normalize();

    // The quadratic smooth minimum in its lerp form, whose gradient is
    // simply the same blend of the two gradients.
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    let f = b + (a - b) * h - k * h * (1.0 - h);
    let g = ga * h + gb * (1.0 - h);
    let g2 = g.length_squared();
    if g2 < 1e-6 {
        return None;
    }
    // Newton toward f = 0 along the gradient: the nearest point on the
    // smooth surface. From inside a concave fillet (where the crease
    // vertices are) those nearest points never cross, so the band cannot
    // fold.
    let step = (-f * g / g2).clamp_length_max(k * 0.5);
    let position = frames.to_target.transform_point3(w + step);
    Some((position, (frames.normal_to_target * g).normalize_or_zero()))
}

/// Area-weighted normals of the band's vertices, one per band slot.
fn band_normals(mesh: &TriMesh, slot_of: &[u32], slots: usize) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; slots];
    for t in mesh.indices.chunks_exact(3) {
        if t.iter().all(|&v| slot_of[v as usize] == NONE) {
            continue;
        }
        let [a, b, c] = [t[0], t[1], t[2]].map(|v| mesh.positions[v as usize]);
        let face = (b - a).cross(c - a);
        for &v in t {
            let slot = slot_of[v as usize];
            if slot != NONE {
                normals[slot as usize] += face;
            }
        }
    }
    normals.iter_mut().for_each(|n| *n = n.normalize_or(Vec3::Y));
    normals
}

/// One pass of tangential Laplacian relax over the band: each vertex moves
/// halfway to its neighbours' average, minus the part of that move along its
/// normal, so it spreads out without shrinking the fillet.
fn relax(positions: &mut [Vec3], triangles: &[[u32; 3]], normals: &[Vec3], slot_of: &[u32], band: &[u32]) {
    let mut sums = vec![Vec3::ZERO; band.len()];
    let mut counts = vec![0u32; band.len()];
    for t in triangles {
        for corner in 0..3 {
            let slot = slot_of[t[corner] as usize];
            if slot != NONE {
                sums[slot as usize] += positions[t[(corner + 1) % 3] as usize] + positions[t[(corner + 2) % 3] as usize];
                counts[slot as usize] += 2;
            }
        }
    }
    // Every new position is worked out before any vertex moves.
    let next: Vec<Vec3> = (0..band.len())
        .map(|slot| {
            let p = positions[band[slot] as usize];
            if counts[slot] == 0 {
                return p;
            }
            let mut step = (sums[slot] / counts[slot] as f32 - p) * 0.5;
            step -= normals[slot] * step.dot(normals[slot]);
            p + step
        })
        .collect();
    for (slot, p) in next.into_iter().enumerate() {
        positions[band[slot] as usize] = p;
    }
}

/// For each vertex of `set`, the place in `set` of the vertex that is its
/// exact mirror image across the plane of `axis` ([`NONE`] if there is none;
/// itself for a vertex on the plane).
fn mirror_partners(positions: &[Vec3], set: &[u32], axis: usize) -> Vec<u32> {
    // Adding zero turns -0.0 into +0.0, so a vertex on the plane finds itself.
    let bits = |p: Vec3| p.to_array().map(|v| (v + 0.0).to_bits());
    let at: HashMap<[u32; 3], u32> =
        set.iter().enumerate().map(|(slot, &i)| (bits(positions[i as usize]), slot as u32)).collect();
    set.iter()
        .map(|&i| {
            let mut image = positions[i as usize];
            image[axis] = -image[axis];
            at.get(&bits(image)).copied().unwrap_or(NONE)
        })
        .collect()
}

/// Puts each mirrored pair of band vertices back exactly opposite one
/// another. They were moved by the same arithmetic on mirrored inputs, which
/// agrees to rounding but not to the bit, and mirrored sculpting drifts on
/// anything less than exact.
fn make_symmetric(positions: &mut [Vec3], band: &[u32], partners: &[u32], axis: usize) {
    for slot in 0..band.len() {
        let twin = partners[slot];
        if twin == NONE || partners[twin as usize] != slot as u32 {
            continue;
        }
        let (i, j) = (band[slot] as usize, band[twin as usize] as usize);
        if i == j {
            positions[i][axis] = 0.0;
        } else if i < j {
            let mut image = positions[j];
            image[axis] = -image[axis];
            let middle = (positions[i] + image) * 0.5;
            positions[i] = middle;
            positions[j] = middle;
            positions[j][axis] = -middle[axis];
        }
    }
}
