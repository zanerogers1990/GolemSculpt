//! Decimate with a mirror on: the model cut at its mirror planes, one part of
//! it decimated, and that part mirrored back, so that the sides of the result
//! are mirror images of each other point for point and triangle for
//! triangle, and brushes, Make Symmetric and the rest find them paired.
//!
//! - **The part decimated** is the positive side of every plane ticked (a
//!   half, a quarter or an eighth), cut by Cut & Mirror's own routine
//!   (`sculpt_symmetry::weld::mirror_and_weld`), with the Symmetry card's
//!   Match Tolerance for the band of points taken as on a plane. The engine
//!   holds the cut's edge to the planes (`sculpt_geom::Keep::seams`), so the
//!   part and its mirror images join up again into one closed model.
//! - **Where the sides are not the same.** A model sculpted with a mirror is
//!   seldom exactly symmetric, and it may have been made different on purpose
//!   (a scar on one cheek). So the other sides are not thrown away. Where one
//!   is further from the part's mirror image than the Match Tolerance, its
//!   planes are added to the part's (`Keep::extra`), so that its detail is
//!   paid for in triangles too, and afterwards each of its points is moved by
//!   how far its own side is from the mirror image there. Nearer than half
//!   the tolerance the sides count as the same and the points mirror
//!   exactly; between half and the whole of it the move is eased in. A move
//!   that would turn a triangle over is not made.
//! - **Where the sides are two different shapes** (anywhere further apart
//!   than [`NOT_ALIKE`] tolerances: a door on one side only), mirrored
//!   triangles could not follow the difference, and the model is decimated
//!   whole, as it is, with nothing mirrored ([`Sides::Unlike`]; the line
//!   says so). That is looked at first, before anything is cut. The brushes' mirror never decides that one side is made over into
//!   the other: that lost Zane a door to Remesh once.
//! - **The mask protects on every side**: a point of the part is as masked
//!   as the most masked of it and its mirror images, so surface masked on
//!   either side keeps its detail on both. The mask the result carries is
//!   each side's own.
//! - **Hidden parts and face groups.** A triangle of the part keeps its own
//!   (it is a piece of a triangle of the model); a triangle of the other
//!   sides takes those of the model's triangle nearest its middle, so a side
//!   hidden or grouped on its own stays so.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::{Decimated, Extra};
use sculpt_symmetry::{Axis, weld};

use crate::mesh::bvh::Bvh;
use crate::mesh::bvh::nearest::closest_on_triangle;

/// A move of a point of another side that would turn one of its triangles
/// this far (the cosine of the angle) is not made: the engine's own limit.
const MOST_TURN: f32 = 0.2;

/// How far apart, in Match Tolerances (half a triangle's width, about), the
/// sides may be anywhere and still be decimated as mirror images, each
/// keeping its own shape where it is a little off. Further apart than that
/// they are two different shapes (a door on one side only), whose
/// difference the mirrored triangles could not follow, and the model is
/// decimated whole as it is: a mirror on for the brushes is never what
/// decides that one side is made over into the other (docs/remesh-symmetry-fix.md).
///
/// Measured on Zane's 13M-triangle figure sculpted with Mirror X (22
/// October 2026): 99.8% of one side's surface within one tolerance of the
/// other's mirror image, 99.96% within two, 99.999% within four and all of
/// it within eight (0.65 mm at the most). At four that figure was taken
/// for two shapes over a few specks; at eight it is mirrored, and a door
/// or a cut a few millimetres deep still is not.
const NOT_ALIKE: f32 = 8.0;

/// `p` reflected across each plane in `bits` (bit 0 X, 1 Y, 2 Z).
fn reflected(mut p: Vec3, bits: u8) -> Vec3 {
    for axis in 0..3 {
        if bits & 1 << axis != 0 {
            p[axis] = -p[axis];
        }
    }
    p
}

/// The planes of `bits` that `p` is on the negative side of: which mirror
/// image of the part it is in.
fn side_of(p: Vec3, bits: u8) -> u8 {
    (0..3).filter(|&axis| bits & 1 << axis != 0 && p[axis] < 0.0).fold(0, |side, axis| side | 1 << axis)
}

/// A surface in a tree, to ask which of its points is nearest a point.
struct Surface {
    bvh: Bvh,
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    /// For each triangle in the tree's order, the number it had.
    order: Vec<u32>,
    /// For each vertex in the tree's order, the number it had.
    numbers: Vec<u32>,
    /// The mask at each vertex, in the tree's order; empty without one.
    mask: Vec<f32>,
}

/// The point of a surface nearest some point.
#[derive(Clone, Copy)]
struct Near {
    /// The triangle it is on, in the tree's order.
    tri: u32,
    at: Vec3,
}

impl Surface {
    fn of(positions: &[Vec3], indices: &[u32], mask: &[f32]) -> Surface {
        let built = Bvh::build(positions, indices);
        let mut numbers = vec![u32::MAX; built.positions.len()];
        for (old, &new) in built.vert_remap.iter().enumerate() {
            if new != u32::MAX {
                numbers[new as usize] = old as u32;
            }
        }
        let mask = if mask.iter().any(|&m| m > 0.0) {
            numbers.iter().map(|&old| mask.get(old as usize).copied().unwrap_or(0.0)).collect()
        } else {
            Vec::new()
        };
        Surface { bvh: built.bvh, positions: built.positions, indices: built.indices, order: built.tri_order, numbers, mask }
    }

    fn corners(&self, tri: u32) -> [u32; 3] {
        let i = tri as usize * 3;
        [self.indices[i], self.indices[i + 1], self.indices[i + 2]]
    }

    /// The point of the surface nearest `p`, if there is one no further than
    /// `within` (squared).
    fn nearest(&self, p: Vec3, within: f32) -> Option<Near> {
        let (tri, _) = self.bvh.nearest_triangle(&self.positions, &self.indices, p, within, |_| false)?;
        let [a, b, c] = self.corners(tri).map(|v| self.positions[v as usize]);
        Some(Near { tri, at: closest_on_triangle(p, a, b, c) })
    }

    /// The vertex of the surface nearest `p`, by the triangle nearest it, by
    /// the number it had.
    fn nearest_vertex(&self, p: Vec3) -> Option<u32> {
        let near = self.nearest(p, f32::INFINITY)?;
        let corner = self.corners(near.tri).into_iter().min_by(|&a, &b| {
            let (da, db) = (self.positions[a as usize].distance_squared(p), self.positions[b as usize].distance_squared(p));
            da.total_cmp(&db)
        })?;
        Some(self.numbers[corner as usize])
    }

    /// The mask where `near` is, between its triangle's corners.
    fn mask_at(&self, near: &Near) -> f32 {
        if self.mask.is_empty() {
            return 0.0;
        }
        let corners = self.corners(near.tri);
        let [a, b, c] = corners.map(|v| self.positions[v as usize]);
        let (v0, v1, v2) = (b - a, c - a, near.at - a);
        let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
        let den = d00 * d11 - d01 * d01;
        let [ma, mb, mc] = corners.map(|v| self.mask[v as usize]);
        if !(den.abs() > 1e-30) {
            return ma.max(mb).max(mc);
        }
        let v = ((d11 * d20 - d01 * d21) / den).clamp(0.0, 1.0);
        let w = ((d00 * d21 - d01 * d20) / den).clamp(0.0, 1.0 - v);
        ma * (1.0 - v - w) + mb * v + mc * w
    }
}

/// The model cut down to the part that is decimated, with what the
/// decimation and the mirroring back need.
pub(super) struct Cut {
    /// The planes, a bit for each of X, Y and Z.
    pub axes: u8,
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
    /// The part's own mask, which the result's points on this side carry.
    pub mask: Vec<f32>,
    /// The mask that protects: at each point the most of the part's and its
    /// mirror images'.
    pub protect: Vec<f32>,
    /// For each triangle of the part, the model's triangle it is a piece of.
    pub triangle_source: Vec<u32>,
    /// The planes of the other sides where they are not the part's mirror image.
    pub extra: Vec<Extra>,
    /// Points of the other sides further than this from the part's mirror
    /// image keep their own side's shape.
    pub tolerance: f32,
    /// The model as it was.
    model: Surface,
}

/// What the sides of a model are to each other.
pub(super) enum Sides {
    /// Alike, or a little off: the model cut, for the part to be decimated.
    Cut(Cut),
    /// Somewhere further apart than [`NOT_ALIKE`] tolerances: two different
    /// shapes, which are not mirrored. How far apart they are, at the most
    /// that was measured.
    Unlike(f32),
}

/// Of the triangles of another side that are too far off, how many at most
/// are measured for how far: the line's figure, not a decision.
const MEASURED: usize = 4096;

impl Cut {
    /// How many times over the part makes the whole: 2, 4 or 8.
    pub fn copies(&self) -> usize {
        1 << self.axes.count_ones()
    }

    /// Whether some of another side was not the part's mirror image.
    pub fn differs(&self) -> bool {
        !self.extra.is_empty()
    }
}

/// Cuts a model (its points, triangles and mask) at the planes `axes` (X, Y,
/// Z), keeping the positive side of each, with points within `tolerance`
/// of a plane taken as on it; unless its sides are two different shapes,
/// which is looked at first. `Err` when nothing is on that side. `progress`
/// as for a decimation; `None` if it said to stop.
pub(super) fn cut(
    positions: &[Vec3],
    indices: &[u32],
    mask: &[f32],
    axes: [bool; 3],
    tolerance: f32,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Option<Sides>, String> {
    let bits = (0..3).filter(|&axis| axes[axis]).fold(0u8, |bits, axis| bits | 1 << axis);
    let model = Surface::of(positions, indices, mask);
    if !progress(0.25) {
        return Ok(None);
    }

    // The model's triangles on the other sides, against the part's mirror
    // image: within the tolerance they are alike; within `NOT_ALIKE` times
    // it, a little off, and their planes are brought onto the part; further,
    // and the sides are two different shapes, which are not mirrored.
    let (alike, little) = (tolerance * tolerance, (NOT_ALIKE * tolerance).powi(2));
    let off: Vec<Off> = indices
        .par_chunks_exact(3)
        .filter_map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|v| positions[v as usize]);
            let middle = (a + b + c) / 3.0;
            let side = side_of(middle, bits);
            let normal = (b - a).cross(c - a);
            let area = normal.length() * 0.5;
            if side == 0 || !(area > 0.0) {
                return None;
            }
            let mirrored = reflected(middle, side);
            if model.nearest(mirrored, alike).is_some() {
                None
            } else if model.nearest(mirrored, little).is_some() {
                Some(Off::Little(mirrored, reflected(normal / (2.0 * area), side), area))
            } else {
                Some(Off::Far(mirrored))
            }
        })
        .collect();
    let far: Vec<Vec3> = off.iter().filter_map(|off| if let Off::Far(at) = off { Some(*at) } else { None }).collect();
    if !far.is_empty() {
        let measured: Vec<Vec3> = far.iter().step_by(far.len().div_ceil(MEASURED)).copied().collect();
        let apart = measured
            .par_iter()
            .map(|&at| model.nearest(at, f32::INFINITY).map_or(0.0, |near| near.at.distance(at)))
            .reduce(|| 0.0, f32::max);
        return Ok(Some(Sides::Unlike(apart)));
    }
    let lone: Vec<(Vec3, Vec3, f32)> =
        off.into_iter().filter_map(|off| if let Off::Little(point, normal, area) = off { Some((point, normal, area)) } else { None }).collect();
    if !progress(0.5) {
        return Ok(None);
    }

    let mut part = (positions.to_vec(), indices.to_vec(), mask.to_vec());
    part.2.resize(positions.len(), 0.0);
    let mut sources: Vec<u32> = (0..indices.len() as u32 / 3).collect();
    for axis in (0..3).filter(|&axis| axes[axis]) {
        let plane = Axis::from_index(axis).expect("an axis is 0, 1 or 2");
        let welded = weld::mirror_and_weld(&part.0, &part.1, plane, true, tolerance)
            .ok_or_else(|| format!("Nothing is on the +{} side of the mirror to decimate.", plane.name()))?;
        let carried = welded.carry_blended(&part.2);
        // The side kept: the first of each pair of triangles, the second being its mirror image.
        let kept: Vec<u32> = welded.mesh.indices.chunks_exact(6).flat_map(|pair| [pair[0], pair[1], pair[2]]).collect();
        sources = welded.triangle_source.iter().step_by(2).map(|&t| sources[t as usize]).collect();
        // Without the mirror image's own vertices, numbered as the kept triangles first name them.
        let mut number = vec![u32::MAX; welded.mesh.positions.len()];
        let (mut kept_positions, mut kept_mask) = (Vec::new(), Vec::new());
        let indices = kept
            .iter()
            .map(|&v| {
                if number[v as usize] == u32::MAX {
                    number[v as usize] = kept_positions.len() as u32;
                    kept_positions.push(welded.mesh.positions[v as usize]);
                    kept_mask.push(carried[v as usize]);
                }
                number[v as usize]
            })
            .collect();
        part = (kept_positions, indices, kept_mask);
    }
    let (positions_kept, indices_kept, own_mask) = part;
    if !progress(0.75) {
        return Ok(None);
    }
    let others: Vec<u8> = (1..8u8).filter(|&side| side & !bits == 0).collect();

    // As masked as the most masked of a point and its mirror images. The
    // sides are alike to within `little`, so that is far enough to look.
    let protect: Vec<f32> = if model.mask.is_empty() {
        own_mask.clone()
    } else {
        positions_kept
            .par_iter()
            .zip(&own_mask)
            .map(|(&p, &own)| {
                others.iter().fold(own, |most, &side| {
                    let mirrored = reflected(p, side);
                    let near = model.nearest(mirrored, little).or_else(|| model.nearest(mirrored, f32::INFINITY));
                    near.map_or(most, |near| most.max(model.mask_at(&near)))
                })
            })
            .collect()
    };

    let extra = if lone.is_empty() {
        Vec::new()
    } else {
        let part_surface = Surface::of(&positions_kept, &indices_kept, &[]);
        lone.par_iter()
            .filter_map(|&(point, normal, area)| Some(Extra { vertex: part_surface.nearest_vertex(point)?, point, normal, area }))
            .collect()
    };
    progress(1.0);
    Ok(Some(Sides::Cut(Cut {
        axes: bits,
        positions: positions_kept,
        indices: indices_kept,
        mask: own_mask,
        protect,
        triangle_source: sources,
        extra,
        tolerance,
        model,
    })))
}

/// A triangle of another side that is not where the part's mirror image is.
enum Off {
    /// A little off: where it would be on the part (its middle and normal
    /// mirrored), and its area.
    Little(Vec3, Vec3, f32),
    /// Further than that: where it would be on the part.
    Far(Vec3),
}

/// The decimated part with its mirror images, as one model.
pub(super) struct Whole {
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
    pub mask: Vec<f32>,
    /// For each triangle, the model's triangle whose hidden flag and group
    /// it takes; empty unless asked for.
    pub triangle_source: Vec<u32>,
    /// Points of the other sides that keep their own side's shape, and how
    /// far apart the sides were where they were furthest (world units).
    pub own_shape: usize,
    pub apart: f32,
    /// Points of the other sides more than the tolerance off their own
    /// side's shape that were held to the mirror image, since moving them
    /// would have turned a triangle over; and how far off they are at most.
    pub held_back: usize,
    pub held_apart: f32,
}

/// `done`, the decimated part of `cut`, mirrored back across every plane it
/// was cut at. `sources`: whether to work out which of the model's
/// triangles each triangle takes its hidden flag and group from.
pub(super) fn mirrored_back(cut: &Cut, done: &Decimated, sources: bool) -> Whole {
    let mut positions = done.positions.clone();
    // Which vertex of the part each vertex is an image of, and which image.
    let mut from: Vec<u32> = (0..positions.len() as u32).collect();
    let mut sides = vec![0u8; positions.len()];
    let mut indices = done.indices.clone();
    let mut tri_from: Vec<u32> = (0..done.triangle_count() as u32).collect();
    let mut tri_sides = vec![0u8; done.triangle_count()];
    for axis in (0..3).filter(|&axis| cut.axes & 1 << axis != 0) {
        let bit = 1u8 << axis;
        let count = positions.len();
        // A point on the plane is its own mirror image.
        let mut twin = vec![0u32; count];
        for v in 0..count {
            if positions[v][axis] == 0.0 {
                twin[v] = v as u32;
            } else {
                twin[v] = positions.len() as u32;
                positions.push(reflected(positions[v], bit));
                from.push(from[v]);
                sides.push(sides[v] | bit);
            }
        }
        // The mirror image wound the other way round, or it would face in.
        let triangles = indices.len() / 3;
        for t in 0..triangles {
            let [a, b, c] = [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]];
            indices.extend_from_slice(&[twin[a as usize], twin[c as usize], twin[b as usize]]);
            tri_from.push(tri_from[t]);
            tri_sides.push(tri_sides[t] | bit);
        }
    }

    // Where the sides are not the same, each point of another side moved by
    // how far its own side is from the mirror image there.
    let (mut own_shape, mut apart, mut held_back, mut held_apart) = (0, 0.0f32, 0, 0.0f32);
    if cut.differs() {
        let tolerance = cut.tolerance;
        let nearest = |p: Vec3| cut.model.nearest(p, f32::INFINITY).map_or(p, |near| near.at);
        let on_part: Vec<Vec3> = done.positions.par_iter().map(|&p| nearest(p)).collect();
        let moves: Vec<(Vec3, f32)> = (0..positions.len())
            .into_par_iter()
            .map(|v| {
                if sides[v] == 0 {
                    return (Vec3::ZERO, 0.0);
                }
                let mirrored = reflected(on_part[from[v] as usize], sides[v]);
                let shift = nearest(positions[v]) - mirrored;
                let length = shift.length();
                let ease = ((length - 0.5 * tolerance) / (0.5 * tolerance)).clamp(0.0, 1.0);
                (shift * ease, length)
            })
            .collect();
        let exact = positions.clone();
        let mut held = vec![false; positions.len()];
        // A move that would turn a triangle over is held back, and so are the
        // others of that triangle, until none turns; if that takes too long,
        // every point is held to the mirror image.
        let mut settled = false;
        for _ in 0..8 {
            for v in 0..positions.len() {
                positions[v] = if held[v] { exact[v] } else { exact[v] + moves[v].0 };
            }
            let turned: Vec<usize> = (0..indices.len() / 3)
                .into_par_iter()
                .filter(|&t| {
                    tri_sides[t] != 0 && {
                        let c = [indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]].map(|v| v as usize);
                        let normal = |p: &[Vec3]| (p[c[1]] - p[c[0]]).cross(p[c[2]] - p[c[0]]);
                        let (before, after) = (normal(&exact), normal(&positions));
                        before.length_squared() > 0.0 && !(before.dot(after) > MOST_TURN * before.length() * after.length())
                    }
                })
                .collect();
            if turned.is_empty() {
                settled = true;
                break;
            }
            for t in turned {
                for k in 0..3 {
                    held[indices[t * 3 + k] as usize] = true;
                }
            }
        }
        if !settled {
            held.fill(true);
            positions.clone_from(&exact);
        }
        for v in 0..positions.len() {
            if !held[v] && moves[v].0 != Vec3::ZERO {
                own_shape += 1;
                apart = apart.max(moves[v].1);
            } else if held[v] && moves[v].1 > tolerance {
                held_back += 1;
                held_apart = held_apart.max(moves[v].1);
            }
        }
    }

    // The mask: the part's own on the part, each other side's own on that side.
    let carried = done.values.as_deref().unwrap_or(&[]);
    let mask: Vec<f32> = (0..positions.len())
        .into_par_iter()
        .map(|v| {
            if sides[v] == 0 {
                carried.get(from[v] as usize).copied().unwrap_or(0.0)
            } else {
                cut.model.nearest(positions[v], f32::INFINITY).map_or(0.0, |near| cut.model.mask_at(&near))
            }
        })
        .collect();

    let triangle_source = if sources {
        (0..indices.len() / 3)
            .into_par_iter()
            .map(|t| {
                if tri_sides[t] == 0 {
                    cut.triangle_source[done.triangle_source[tri_from[t] as usize] as usize]
                } else {
                    let middle = (0..3).map(|k| positions[indices[t * 3 + k] as usize]).sum::<Vec3>() / 3.0;
                    cut.model.nearest(middle, f32::INFINITY).map_or(0, |near| cut.model.order[near.tri as usize])
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    Whole { positions, indices, mask, triangle_source, own_shape, apart, held_back, held_apart }
}
