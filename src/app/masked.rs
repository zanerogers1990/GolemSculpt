//! Deforming around a mask with the gizmo. With part of an object masked, a
//! gizmo drag does not move the object: it moves, turns or stretches the
//! unmasked part of its surface and leaves the masked part exactly where it
//! is. Mask the body, then pull a limb out of it.
//!
//! It runs as a stroke, like a brush: the vertices are written through the
//! mesh's stroke calls, so the renderer follows each frame and the whole drag
//! is one ordinary undo step.
//!
//! **Symmetry.** With mirror axes ticked for the object, each half of the
//! surface moves as the mirror image of the other: mask the body, leave both
//! arms free, pull the X arrow out and both arms go out by the same distance.
//! One half follows the gizmo and the others are its mirror images. That is
//! the half on the plus side of each plane, the side the arrow points to,
//! unless nearly all the free surface is on the minus side (one arm left
//! free): then that half follows, so the arm goes where the arrow is pulled.
//! A vertex on a plane is moved by the average of the two halves' moves,
//! which keeps it on the plane. With the object's own planes, a model that
//! is mirrored to the bit stays mirrored to the bit.
//!
//! **Radial symmetry.** With strokes repeated round an axis as well, the
//! drag is repeated as a stroke is: through the same list of copies the
//! brushes and the Pose tool keep (`Settings::symmetry_group`), every turn
//! round the axis and every mirror image. Mask round six tentacles, pull one
//! out, and all six come out. The free surface is shared out between the
//! copies as pieces: one piece follows the gizmo, and each of the others is
//! that piece as one copy carries it, moved as that copy carries the drag.
//! Which piece a vertex is in is a matter of which copy of one point, the
//! middle of the leading piece, it is nearest; so the borders between the
//! pieces run halfway between one tentacle and the next, through the mask,
//! where they tear nothing. The leading piece is the one furthest out along
//! the first spoke of the radial guide (the plus side of the planes, as
//! above), or the only free piece when just one is left free. A vertex on a
//! border (on the axis, or on a plane among the copies) is shared by the
//! copies that meet there and moved by the average of their moves, which is
//! what the Move grab's sharing rule (`share::Hold`) comes to for copies that
//! hold a vertex alike: it keeps a vertex on the axis on the axis, and no
//! copy counts for more than another, whatever order the copies come in.
//! Turns by a share of a circle are not exact in floats, so the copies agree
//! to rounding rather than to the bit.

use glam::{Affine3A, DMat4, DVec3, Mat3, Mat4, Quat, Vec3};
use rayon::prelude::*;
use sculpt_geom::SymmetryGroup;
use sculpt_symmetry::Placement;

use crate::mesh::{Mesh, StrokeDelta};
use crate::sculpt::{Radial, RadialAxis, Settings};

/// Mask values at or below this count as no mask at all, as in the Unity
/// app: what an erased or blurred-out mask leaves behind must not switch the
/// gizmo from moving the object to reshaping it.
const MASKED: f32 = 0.001;

/// The minus half of a plane follows the gizmo when it holds more than this
/// many times the free surface the plus half does.
const LEADS: f64 = 3.0;

/// The mirror planes a masked drag is repeated across: the object's symmetry.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mirror {
    /// Across X, Y and Z.
    pub axes: [bool; 3],
    /// Where the object stands, when the planes are the world's (its
    /// symmetry is World); `None` for planes of its own.
    pub world: Option<Mat4>,
    /// Radial symmetry on top of the planes, if the object has it on.
    /// `None`, or radial symmetry that is off, leaves the drag exactly as it
    /// was before radial symmetry had a say in it.
    pub radial: Option<Turns>,
}

/// The radial symmetry a masked drag is repeated under, as the object has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turns {
    pub radial: Radial,
    /// Where the object stands, when its symmetry is the world's; `None` for
    /// a symmetry of its own.
    pub world: Option<Placement>,
}

/// How near the border between two pieces a vertex has to be, as a share of
/// the model's size, to be shared by both. Far more than the rounding a
/// vertex kept on the axis is off by, and far less than any vertex a
/// sculptor could have put beside it.
const ON_BORDER: f64 = 1e-5;

/// How many times the middle of the leading piece is looked for again, each
/// time from where the last left it (see `Copies::new`).
const SETTLE: usize = 4;

/// The free surface under radial symmetry, shared out between the copies of
/// the drag (see the top of this file).
struct Copies {
    /// Each copy as a map of the mesh's own space, in the symmetry's own
    /// order (the drag as made first), and the inverse of each.
    ops: Vec<DMat4>,
    inverses: Vec<DMat4>,
    /// The middle of the leading piece, as each copy carries it: a vertex is
    /// in the piece of the copy whose point here it is nearest.
    seeds: Vec<DVec3>,
    /// `ON_BORDER` in the mesh's own units.
    near: f64,
    /// The sets of copies that share a vertex on a border, one bit a copy.
    shared: Vec<[u64; 4]>,
    /// For each vertex of `Deform::verts`, which move it takes: the copy its
    /// piece is, or past the last copy, a set of `shared`.
    region: Vec<u32>,
    /// What the status line says the drag was repeated as.
    words: String,
}

/// The copies a vertex at `p` takes its move from: one, or several sharing it.
enum Region {
    One(usize),
    Shared([u64; 4]),
}

impl Copies {
    /// The copies under `turns` with the mirror axes `axes`, and the free
    /// vertices `verts` of `mesh` shared out between them. `None` when the
    /// symmetry has no turns in it after all: the planes alone then do it,
    /// as they always have.
    fn new(mesh: &Mesh, verts: &[u32], axes: [bool; 3], turns: Turns) -> Option<Copies> {
        if turns.radial.repeats() < Radial::MIN_COUNT || verts.is_empty() {
            return None;
        }
        // The very list a stroke under this symmetry is made as.
        let mut settings = Settings::default();
        settings.set_symmetry(axes, turns.radial, turns.world);
        let group: SymmetryGroup = settings.symmetry_group();
        if group.radial_count() < 2 || group.len() > SymmetryGroup::MAX_ELEMENTS {
            return None;
        }
        let ops: Vec<DMat4> = group.iter().map(|op| op.to_mat4().as_dmat4()).collect();
        let inverses: Vec<DMat4> = ops.iter().map(|op| op.inverse()).collect();

        // Where to start looking for the leading piece: the free vertex
        // furthest out along the radial guide's first spoke, leant a little
        // to the plus side of every plane (a spoke can lie on one). That is
        // in the symmetry's own space, which for a World symmetry is the
        // world's.
        let spoke = Quat::from_rotation_arc(Vec3::Y, turns.radial.axis_vector()) * Vec3::X;
        let lead = (spoke + Vec3::splat(0.05)).as_dvec3();
        let frame = turns.world.map_or(DMat4::IDENTITY, |at| at.matrix().as_dmat4());
        let at = |v: u32| mesh.positions[v as usize].as_dvec3();
        let first = verts
            .par_iter()
            .with_min_len(1 << 14)
            .map(|&v| (frame.transform_point3(at(v)).dot(lead), v))
            // The furthest out, and of those the first: the same vertex
            // however the work is shared between threads.
            .reduce(|| (f64::NEG_INFINITY, u32::MAX), |a, b| if b.0 > a.0 || (b.0 == a.0 && b.1 < a.1) { b } else { a })
            .1;
        let mut copies = Copies {
            seeds: Vec::new(),
            ops,
            inverses,
            near: ON_BORDER * f64::from(mesh.bounds().size().max_element().max(1e-12)),
            shared: Vec::new(),
            region: Vec::new(),
            words: words(axes, turns.radial),
        };

        // The middle of the leading piece: every free vertex is carried back
        // into the leading piece by the copy it is nearest, and the middle is
        // where those land, by how free each is. Looked for again from there
        // a few times, it settles in the middle of one tentacle and its
        // images, with the borders halfway to the next. On a sample of the
        // vertices, added up in fixed runs, so the same mesh always settles
        // in the same place.
        let stride = (verts.len() >> 17).max(1);
        let sample: Vec<u32> = verts.iter().step_by(stride).copied().collect();
        let mut middle = at(first);
        for _ in 0..SETTLE {
            copies.seeds = copies.ops.iter().map(|op| op.transform_point3(middle)).collect();
            let runs: Vec<(DVec3, f64)> = sample
                .par_chunks(4096)
                .map(|run| {
                    let (mut sum, mut weight) = (DVec3::ZERO, 0.0);
                    for &v in run {
                        let p = at(v);
                        let w = f64::from(1.0 - mesh.mask[v as usize]);
                        sum += copies.inverses[copies.nearest(p).0].transform_point3(p) * w;
                        weight += w;
                    }
                    (sum, weight)
                })
                .collect();
            let (sum, weight) = runs.iter().fold((DVec3::ZERO, 0.0), |(s, w), &(a, b)| (s + a, w + b));
            if weight > 0.0 {
                middle = sum / weight;
            }
        }
        copies.seeds = copies.ops.iter().map(|op| op.transform_point3(middle)).collect();

        // Which piece each free vertex is in, once, at the press.
        let n = copies.ops.len() as u32;
        let mut region: Vec<u32> = verts
            .par_iter()
            .with_min_len(1 << 12)
            .map(|&v| match copies.region_of(mesh.positions[v as usize]) {
                Region::One(g) => g as u32,
                Region::Shared(_) => u32::MAX,
            })
            .collect();
        for (slot, code) in region.iter_mut().enumerate() {
            if *code == u32::MAX {
                let Region::Shared(set) = copies.region_of(mesh.positions[verts[slot] as usize]) else { unreachable!() };
                let k = match copies.shared.iter().position(|s| *s == set) {
                    Some(k) => k,
                    None => {
                        copies.shared.push(set);
                        copies.shared.len() - 1
                    }
                };
                *code = n + k as u32;
            }
        }
        copies.region = region;
        Some(copies)
    }

    /// The copy whose point `seeds` is nearest `p`, and how near, squared.
    /// Of copies as near as each other, the first.
    #[inline]
    fn nearest(&self, p: DVec3) -> (usize, f64) {
        let mut best = (0, f64::INFINITY);
        for (g, seed) in self.seeds.iter().enumerate() {
            let d = p.distance_squared(*seed);
            if d < best.1 {
                best = (g, d);
            }
        }
        best
    }

    /// Which copies a vertex that was at `p` at the press takes its move from.
    fn region_of(&self, p: Vec3) -> Region {
        let p = p.as_dvec3();
        let (best, least) = self.nearest(p);
        let mut set = [0u64; 4];
        let mut more = false;
        for (g, seed) in self.seeds.iter().enumerate() {
            // How much further a point is from one copy's point than from
            // another's is twice its distance from the border between them
            // times how far apart the two are; copies whose points are one
            // (as near as rounding) share everything between them.
            let apart = seed.distance(self.seeds[best]);
            if g != best && p.distance_squared(*seed) - least <= 2.0 * self.near * (apart + self.near) {
                set[g / 64] |= 1 << (g % 64);
                more = true;
            }
        }
        if !more {
            return Region::One(best);
        }
        set[best / 64] |= 1 << (best % 64);
        Region::Shared(set)
    }

    /// The move `region` names, of those `maps` gives, for a vertex that was
    /// at `p` at the press: worked out again, the same way, for when only
    /// the place is known.
    fn code(&self, p: Vec3) -> usize {
        match self.region_of(p) {
            Region::One(g) => g,
            Region::Shared(set) => match self.shared.iter().position(|s| *s == set) {
                Some(k) => self.ops.len() + k,
                // Every vertex was sorted at the press, so this is never
                // reached; the drag as made is the safest move to give.
                None => 0,
            },
        }
    }

    /// How many moves `maps` gives: one a copy, and one a shared set.
    fn moves(&self) -> usize {
        self.ops.len() + self.shared.len()
    }

    /// The move of each piece, from the move `local` of the leading one:
    /// that move as each copy carries it, and for a shared set the average of
    /// its copies' moves.
    fn maps(&self, local: DMat4) -> Vec<DMat4> {
        // No move is no move in every piece, exactly.
        if local == DMat4::IDENTITY {
            return vec![DMat4::IDENTITY; self.moves()];
        }
        let mut maps: Vec<DMat4> = self.ops.iter().zip(&self.inverses).map(|(op, inverse)| *op * local * *inverse).collect();
        for set in &self.shared {
            let members = (0..self.ops.len()).filter(|&g| set[g / 64] & (1 << (g % 64)) != 0);
            let (sum, count) = members.fold((DMat4::ZERO, 0u32), |(s, c), g| (s + maps[g], c + 1));
            maps.push(sum * (1.0 / f64::from(count.max(1))));
        }
        maps
    }
}

/// "6 times round Y and mirrored across X": what the status line says a
/// drag under radial symmetry was repeated as.
fn words(axes: [bool; 3], radial: Radial) -> String {
    let round = match radial.axis {
        RadialAxis::Custom => "round the custom axis".to_owned(),
        axis => format!("round {}", axis.name()),
    };
    let planes: Vec<&str> = (0..3).filter(|&a| axes[a]).map(|a| ["X", "Y", "Z"][a]).collect();
    let count = radial.repeats().clamp(Radial::MIN_COUNT, Radial::MAX_COUNT);
    match planes.is_empty() {
        true => format!("{count} times {round}"),
        false => format!("{count} times {round} and mirrored across {}", planes.join(" and ")),
    }
}

/// Which side of each mirror plane a vertex is on.
struct Sides {
    axes: [bool; 3],
    /// The mesh's own space into the planes' space, for World planes.
    world: Option<Mat4>,
    /// The planes in the mesh's own space, as reflections: one for each
    /// ticked axis, `None` for the others.
    flips: [Option<DMat4>; 3],
    /// Along which axes the minus half is the one that follows the gizmo.
    minus_leads: [bool; 3],
}

impl Sides {
    fn new(mirror: Mirror) -> Option<Sides> {
        if mirror.axes == [false; 3] {
            return None;
        }
        let flips = std::array::from_fn(|a| {
            mirror.axes[a].then(|| {
                let mut signs = DVec3::ONE;
                signs[a] = -1.0;
                let flip = DMat4::from_scale(signs);
                match mirror.world {
                    None => flip,
                    Some(model) => {
                        let model = model.as_dmat4();
                        model.inverse() * flip * model
                    }
                }
            })
        });
        Some(Sides { axes: mirror.axes, world: mirror.world, flips, minus_leads: [false; 3] })
    }

    /// Where a vertex is: for each axis in turn, a digit of a number in base
    /// three: 0 on the half that follows the gizmo, 1 on the other, 2 on the
    /// plane itself. 0 along axes that are not mirrored.
    fn code(&self, p: Vec3) -> usize {
        let q = self.world.map_or(p, |m| m.transform_point3(p));
        let mut code = 0;
        for a in (0..3).rev() {
            let digit = match q[a] {
                _ if !self.axes[a] => 0,
                x if x == 0.0 => 2,
                x => usize::from((x < 0.0) != self.minus_leads[a]),
            };
            code = code * 3 + digit;
        }
        code
    }

    /// The move of each place `code` names, from the move `local` of the
    /// half that follows the gizmo: that move reflected across the planes
    /// between, or the average of those on a plane.
    fn maps(&self, local: DMat4) -> Vec<DMat4> {
        // No move is no move on every side, exactly: the world's planes in
        // the mesh's space are not their own inverses to the bit.
        if local == DMat4::IDENTITY {
            return vec![DMat4::IDENTITY; 27];
        }
        (0..27)
            .map(|code| {
                let digits = [code % 3, code / 3 % 3, code / 9];
                // Every way across the planes this place is reached by.
                let mut ways = vec![DMat4::IDENTITY];
                for a in 0..3 {
                    let Some(flip) = self.flips[a] else { continue };
                    match digits[a] {
                        0 => {}
                        1 => ways.iter_mut().for_each(|w| *w = flip * *w),
                        _ => ways = ways.iter().flat_map(|&w| [w, flip * w]).collect(),
                    }
                }
                let sum = ways.iter().map(|&w| w * local * w).fold(DMat4::ZERO, |s, m| s + m);
                if ways.len() == 1 { sum } else { sum * (1.0 / ways.len() as f64) }
            })
            .collect()
    }
}

/// One gizmo drag on the unmasked part of a mesh, press to release.
pub struct Deform {
    /// `Mesh::uid` of the mesh the stroke was opened on.
    uid: u64,
    /// Every vertex that is not fully masked, in ascending order (which lets
    /// the mesh share them between threads). Listed once at the press: a
    /// mesh can have tens of millions of vertices and the mask cannot change
    /// during the drag.
    verts: Vec<u32>,
    /// The parts of the mesh's spatial index those vertices touch.
    leaves: Vec<u32>,
    /// The mirror planes the drag is repeated across, if any, with no
    /// radial symmetry.
    sides: Option<Sides>,
    /// Every copy the drag is repeated as, under radial symmetry. `sides`
    /// is then `None`: the copies have the planes among them.
    copies: Option<Copies>,
    /// The drag as last applied, in the mesh's own space: one move for
    /// each place `Sides::code` names, or just the one with no mirror. Their
    /// turn and stretch is what the mesh's sculpt layers are carried by when
    /// the drag ends (`Mesh::end_stroke_carried`).
    maps: std::cell::RefCell<Vec<Affine3A>>,
}

impl Deform {
    /// Opens the stroke. `None`, and nothing is begun, when the mesh has no
    /// mask to speak of: the gizmo then moves the object as a whole, which
    /// looks the same and costs nothing.
    pub fn begin(mesh: &mut Mesh, mirror: Mirror) -> Option<Deform> {
        let mask = &mesh.mask;
        if !mask.par_iter().with_min_len(1 << 16).any(|&m| m > MASKED) {
            return None;
        }
        let verts: Vec<u32> =
            (0..mask.len() as u32).into_par_iter().with_min_len(1 << 14).filter(|&v| mask[v as usize] < 1.0).collect();
        let copies = mirror.radial.and_then(|turns| Copies::new(mesh, &verts, mirror.axes, turns));
        let mut sides = if copies.is_some() { None } else { Sides::new(mirror) };
        if let Some(sides) = &mut sides {
            // How much free surface each half holds, by vertex: [plus, minus] per axis.
            let free = verts
                .par_iter()
                .with_min_len(1 << 14)
                .fold(
                    || [[0.0f64; 2]; 3],
                    |mut free, &v| {
                        let p = mesh.positions[v as usize];
                        let q = sides.world.map_or(p, |m| m.transform_point3(p));
                        for a in 0..3 {
                            if q[a] != 0.0 {
                                free[a][usize::from(q[a] < 0.0)] += f64::from(1.0 - mesh.mask[v as usize]);
                            }
                        }
                        free
                    },
                )
                .reduce(|| [[0.0; 2]; 3], |a, b| std::array::from_fn(|i| [a[i][0] + b[i][0], a[i][1] + b[i][1]]));
            sides.minus_leads = std::array::from_fn(|a| sides.axes[a] && free[a][1] > free[a][0] * LEADS);
        }
        let leaves = mesh.leaves_of(&verts);
        mesh.begin_stroke();
        let moves = match (&copies, &sides) {
            (Some(copies), _) => copies.moves(),
            (None, Some(_)) => 27,
            (None, None) => 1,
        };
        let maps = std::cell::RefCell::new(vec![Affine3A::IDENTITY; moves]);
        Some(Deform { uid: mesh.uid, verts, leaves, sides, copies, maps })
    }

    /// How many vertices the drag can move.
    #[cfg(test)]
    pub fn free(&self) -> usize {
        self.verts.len()
    }

    /// The planes the drag is mirrored across, as the status line names
    /// them: "X", "X and Z". `None` with no mirror.
    pub fn mirrored(&self) -> Option<String> {
        let axes = self.sides.as_ref()?.axes;
        let names: Vec<&str> = (0..3).filter(|&a| axes[a]).map(|a| ["X", "Y", "Z"][a]).collect();
        Some(names.join(" and "))
    }

    /// How the status line says the drag was repeated, to follow the
    /// object's name: ", each side the mirror image of the other across X",
    /// ", repeated 6 times round Y", or nothing with no symmetry.
    pub fn how(&self) -> String {
        match (&self.copies, self.mirrored()) {
            (Some(copies), _) => format!(", repeated {}", copies.words),
            (None, Some(planes)) => format!(", each side the mirror image of the other across {planes}"),
            (None, None) => String::new(),
        }
    }

    /// Puts the surface where the drag has it now. `world` is the whole drag
    /// so far as one change to the world (`gizmo::Drag::world_delta`), and
    /// `model` the object's placement. Every vertex is worked out again from
    /// where it was at the press, so nothing drifts however long the drag
    /// lasts, and dragging back to the start puts the surface back.
    pub fn apply(&self, mesh: &mut Mesh, model: Mat4, world: DMat4) {
        if mesh.uid != self.uid {
            return;
        }
        // The same change in the object's own space, where the vertices
        // live. In doubles, so that going there and back loses nothing.
        let local = if world == DMat4::IDENTITY {
            DMat4::IDENTITY
        } else {
            let model = model.as_dmat4();
            model.inverse() * world * model
        };
        let maps: Vec<Affine3A> = match (&self.copies, &self.sides) {
            (Some(copies), _) => copies.maps(local).iter().map(|m| Affine3A::from_mat4(m.as_mat4())).collect(),
            (None, None) => vec![Affine3A::from_mat4(local.as_mat4())],
            (None, Some(sides)) => sides.maps(local).iter().map(|m| Affine3A::from_mat4(m.as_mat4())).collect(),
        };
        let (sides, copies) = (self.sides.as_ref(), self.copies.as_ref());
        mesh.move_listed(&self.verts, |slot, vertex| {
            let map = match (copies, sides) {
                (Some(copies), _) => &maps[copies.region[slot] as usize],
                (None, None) => &maps[0],
                (None, Some(sides)) => &maps[sides.code(vertex.start)],
            };
            let moved = map.transform_point3(vertex.start);
            // A half-masked vertex goes half the way, which gives a pulled
            // limb a smooth root instead of a torn ring. Written as a step
            // from the start so that no change is exactly no change.
            let weight = 1.0 - vertex.mask;
            Some(if weight >= 1.0 { moved } else { vertex.start + (moved - vertex.start) * weight })
        });
        mesh.positions_changed(&self.leaves);
        *self.maps.borrow_mut() = maps;
    }

    /// Closes the stroke and returns its undo record. `None` if the surface
    /// ended where it began.
    pub fn end(self, mesh: &mut Mesh) -> Option<StrokeDelta> {
        if mesh.uid != self.uid {
            return None;
        }
        let maps = self.maps.into_inner();
        let (sides, copies) = (self.sides, self.copies);
        let linear = |start: Vec3| {
            let at = match (&copies, &sides) {
                (Some(copies), _) => copies.code(start),
                (None, Some(sides)) => sides.code(start),
                (None, None) => 0,
            };
            Mat3::from(maps[at].matrix3)
        };
        match mesh.end_stroke_carried(&linear) {
            Some(StrokeDelta::Positions { before, after, .. }) if before == after => None,
            delta => delta,
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec2, Vec3};

    use super::*;
    use crate::camera::Camera;
    use crate::gizmo::{Drag, Frame, Handle, Handles, Orientation, Placement, Target};
    use crate::history::History;
    use crate::mesh::primitives::{self, Primitive};

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    /// A unit ball with its lower part fully masked, a band above that
    /// half masked, and the top free.
    fn masked_ball(triangles: usize) -> Mesh {
        let mut mesh = primitives::build(Primitive::Sphere, triangles, 0.5);
        for v in 0..mesh.vertex_count() {
            let y = mesh.positions[v].y;
            mesh.mask[v] = if y < -0.05 { 1.0 } else if y < 0.1 { 0.5 } else { 0.0 };
        }
        mesh
    }

    fn matrix(p: Placement) -> Mat4 {
        Mat4::from_scale_rotation_translation(p.scale, p.rotation, p.position)
    }

    /// The gizmo on one object, as the app sets it up, and a view of it.
    fn gizmo(placement: Placement, orientation: Orientation) -> (crate::camera::View, [Target; 1], Frame) {
        let mut camera = Camera::default();
        (camera.yaw, camera.pitch, camera.distance) = (25.0, 20.0, 6.0);
        let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
        let target = [Target { placement, radius: 0.87, turns: true, scales: true }];
        let frame = Frame::on(&target, orientation, Handles::ALL, &view, 1.0).unwrap();
        (view, target, frame)
    }

    #[test]
    fn a_masked_move_pins_the_masked_part_and_undo_restores_every_vertex() {
        let mut mesh = masked_ball(20_000);
        let before = mesh.positions.clone();
        let mask = mesh.mask.clone();
        let mut history = History::default();
        // Turned and stretched, so the object's own space is not the world's.
        let placement = Placement {
            position: Vec3::new(0.3, -0.1, 0.2),
            rotation: Quat::from_rotation_z(0.4) * Quat::from_rotation_y(0.5),
            scale: Vec3::new(1.0, 1.4, 0.7),
        };
        let model = matrix(placement);
        let (view, target, frame) = gizmo(placement, Orientation::Global);
        let px = |p: Vec3| view.project(p).unwrap();

        let deform = Deform::begin(&mut mesh, Mirror::default()).expect("part of it is masked");
        let free = mask.iter().filter(|&&m| m < 1.0).count();
        assert!(free > 0 && free < mask.len());
        assert_eq!(deform.free(), free);

        // Up the world's Y by 0.3.
        let grab = placement.position + Vec3::Y * frame.arm;
        let mut drag = Drag::on(Handle::Move(1), frame, &target, &view, px(grab));
        drag.update(&view, px(grab + Vec3::Y * 0.3));
        deform.apply(&mut mesh, model, drag.world_delta());

        let world = |p: Vec3| model.transform_point3(p);
        let (mut pinned, mut whole, mut half) = (0, 0, 0);
        for v in 0..before.len() {
            let travelled = world(mesh.positions[v]) - world(before[v]);
            match mask[v] {
                m if m >= 1.0 => {
                    assert_eq!(mesh.positions[v], before[v], "a masked vertex moved");
                    pinned += 1;
                }
                m if m <= 0.0 => {
                    assert!(travelled.distance(Vec3::Y * 0.3) < 1e-4, "{travelled}");
                    whole += 1;
                }
                _ => {
                    assert!(travelled.distance(Vec3::Y * 0.15) < 1e-4, "{travelled}");
                    half += 1;
                }
            }
        }
        assert!(pinned > 0 && whole > 0 && half > 0);
        assert_eq!(pinned + whole + half, before.len());

        // It is an ordinary stroke: one undo step puts every vertex back.
        let delta = deform.end(&mut mesh).expect("the surface moved");
        history.push_stroke(delta);
        assert_eq!(history.steps(), (1, 0));
        let moved = mesh.positions.clone();
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(mesh.positions, before);
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert_eq!(mesh.positions, moved);
        assert_eq!(mesh.mask, mask, "the mask is not touched");
    }

    #[test]
    fn a_masked_turn_and_stretch_go_about_the_objects_own_origin() {
        let placement = Placement {
            position: Vec3::new(0.3, -0.1, 0.2),
            rotation: Quat::from_rotation_y(0.5),
            scale: Vec3::new(1.0, 1.4, 0.7),
        };
        let model = matrix(placement);
        let world = |p: Vec3| model.transform_point3(p);

        // A quarter turn about the world's Y (Global), which is not one of
        // the object's own axes' turns once it is stretched.
        let mut mesh = masked_ball(20_000);
        let before = mesh.positions.clone();
        let (view, target, frame) = gizmo(placement, Orientation::Global);
        let px = |p: Vec3| view.project(p).unwrap();
        let deform = Deform::begin(&mut mesh, Mirror::default()).unwrap();
        let (u, v) = (Vec3::Z * frame.arm * 0.63, Vec3::X * frame.arm * 0.63);
        let mut drag = Drag::on(Handle::Rotate(1), frame, &target, &view, px(placement.position + u));
        for k in 1..=30 {
            let a = k as f32 / 30.0 * std::f32::consts::FRAC_PI_2;
            drag.update(&view, px(placement.position + u * a.cos() + v * a.sin()));
            deform.apply(&mut mesh, model, drag.world_delta());
        }
        let turned = drag.placements()[0].rotation * placement.rotation.inverse();
        assert!(turned.angle_between(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)) < 0.02);
        for v in 0..before.len() {
            if mesh.mask[v] == 0.0 {
                let want = placement.position + turned * (world(before[v]) - placement.position);
                assert!(world(mesh.positions[v]).distance(want) < 1e-4);
            } else if mesh.mask[v] == 1.0 {
                assert_eq!(mesh.positions[v], before[v]);
            }
        }
        // Brought back round to the press, the surface is back as it was.
        drag.update(&view, px(placement.position + u));
        deform.apply(&mut mesh, model, drag.world_delta());
        let off = mesh.positions.iter().zip(&before).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        assert!(off < 1e-5, "{off}");
        deform.end(&mut mesh);

        // Pulling the cube on the object's own X stretches its own X alone.
        let mut mesh = masked_ball(20_000);
        let (view, target, frame) = gizmo(placement, Orientation::Auto);
        let px = |p: Vec3| view.project(p).unwrap();
        let deform = Deform::begin(&mut mesh, Mirror::default()).unwrap();
        let own = frame.scale_axis(0);
        let grab = placement.position + own * frame.arm * 0.82;
        let mut drag = Drag::on(Handle::Scale(0), frame, &target, &view, px(grab));
        drag.update(&view, px(placement.position + own * frame.arm * 1.64));
        deform.apply(&mut mesh, model, drag.world_delta());
        for v in 0..before.len() {
            let want = match mesh.mask[v] {
                m if m >= 1.0 => before[v],
                m => before[v] * Vec3::new(1.0 + (1.0 - m), 1.0, 1.0),
            };
            assert!(mesh.positions[v].distance(want) < 1e-3, "{} {want}", mesh.positions[v]);
        }
        // Back at the press nothing is left to undo.
        drag.update(&view, px(grab));
        deform.apply(&mut mesh, model, drag.world_delta());
        assert_eq!(mesh.positions, before);
        assert!(deform.end(&mut mesh).is_none());
    }

    fn flip(p: Vec3) -> Vec3 {
        Vec3::new(-p.x, p.y, p.z)
    }

    /// A ball with its middle masked across X, a half-masked band either
    /// side of that, and both sides free.
    fn middle_masked_ball() -> Mesh {
        let mut mesh = primitives::build(Primitive::Sphere, 20_000, 0.5);
        for v in 0..mesh.vertex_count() {
            let x = mesh.positions[v].x.abs();
            mesh.mask[v] = if x < 0.1 { 1.0 } else if x < 0.2 { 0.5 } else { 0.0 };
        }
        mesh
    }

    /// Pulls the gizmo's X arrow (Global axes) `by` along the world's X and
    /// puts the surface there. Gives back the drag as a change to the world.
    fn pull_x(mesh: &mut Mesh, deform: &Deform, placement: Placement, by: f32) -> DMat4 {
        let (view, target, frame) = gizmo(placement, Orientation::Global);
        let px = |p: Vec3| view.project(p).unwrap();
        let grab = placement.position + Vec3::X * frame.arm;
        let mut drag = Drag::on(Handle::Move(0), frame, &target, &view, px(grab));
        drag.update(&view, px(grab + Vec3::X * by));
        deform.apply(mesh, matrix(placement), drag.world_delta());
        drag.world_delta()
    }

    const ACROSS_X: Mirror = Mirror { axes: [true, false, false], world: None, radial: None };

    #[test]
    fn under_mirror_symmetry_pulling_one_side_out_pulls_the_other_out_as_far() {
        // Standing square to the world: the arrow pulls the plus side out
        // along X, and the minus side goes out the other way as far.
        let mut mesh = middle_masked_ball();
        let (before, mask) = (mesh.positions.clone(), mesh.mask.clone());
        let placement = Placement { position: Vec3::new(0.3, -0.1, 0.2), rotation: Quat::IDENTITY, scale: Vec3::ONE };
        let deform = Deform::begin(&mut mesh, ACROSS_X).unwrap();
        assert_eq!(deform.mirrored().as_deref(), Some("X"));
        pull_x(&mut mesh, &deform, placement, 0.3);
        let (mut plus, mut minus) = (0, 0);
        for v in 0..before.len() {
            let travelled = mesh.positions[v] - before[v];
            let share = 1.0 - mask[v];
            if mask[v] >= 1.0 {
                assert_eq!(travelled, Vec3::ZERO);
            } else if before[v].x > 0.0 {
                assert!(travelled.distance(Vec3::X * 0.3 * share) < 1e-4, "{travelled}");
                plus += 1;
            } else {
                assert!(travelled.distance(-Vec3::X * 0.3 * share) < 1e-4, "{travelled}");
                minus += 1;
            }
        }
        assert!(plus > 1000 && minus > 1000, "{plus} {minus}");
        let mut history = History::default();
        history.push_stroke(deform.end(&mut mesh).expect("both sides moved"));
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(mesh.positions, before, "one undo step puts both sides back");

        // Turned and stretched: each half moves as the other's mirror image in
        // the object's own space, and a ball mirrored to the bit stays so.
        let mut mesh = middle_masked_ball();
        let placement = Placement {
            position: Vec3::new(0.3, -0.1, 0.2),
            rotation: Quat::from_rotation_z(0.4) * Quat::from_rotation_y(0.5),
            scale: Vec3::new(1.0, 1.4, 0.7),
        };
        let model = matrix(placement).as_dmat4();
        let deform = Deform::begin(&mut mesh, ACROSS_X).unwrap();
        let world = pull_x(&mut mesh, &deform, placement, 0.3);
        let local = Affine3A::from_mat4((model.inverse() * world * model).as_mat4());
        for v in 0..before.len() {
            let p = before[v];
            let full = if p.x > 0.0 { local.transform_point3(p) } else { flip(local.transform_point3(flip(p))) };
            let want = p + (full - p) * (1.0 - mask[v]);
            assert!(mesh.positions[v].distance(want) < 1e-5, "{} {want}", mesh.positions[v]);
        }
        let bits = |p: Vec3| p.to_array().map(f32::to_bits);
        let at: std::collections::HashMap<[u32; 3], usize> = before.iter().enumerate().map(|(v, &p)| (bits(p), v)).collect();
        let mut pairs = 0;
        for v in (0..before.len()).filter(|&v| before[v].x > 0.0) {
            if let Some(&w) = at.get(&bits(flip(before[v]))) {
                assert_eq!(bits(mesh.positions[w]), bits(flip(mesh.positions[v])), "a mirrored pair came apart");
                pairs += 1;
            }
        }
        assert!(pairs > 1000, "the ball is mirrored to the bit: {pairs} pairs");
        deform.end(&mut mesh);
    }

    #[test]
    fn with_one_side_left_free_that_side_follows_the_gizmo() {
        let mut mesh = primitives::build(Primitive::Sphere, 20_000, 0.5);
        for v in 0..mesh.vertex_count() {
            mesh.mask[v] = if mesh.positions[v].x < -0.25 { 0.0 } else { 1.0 };
        }
        let before = mesh.positions.clone();
        let placement = Placement { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        let deform = Deform::begin(&mut mesh, ACROSS_X).unwrap();
        pull_x(&mut mesh, &deform, placement, 0.3);
        let mut free = 0;
        for v in (0..before.len()).filter(|&v| mesh.mask[v] == 0.0) {
            let travelled = mesh.positions[v] - before[v];
            assert!(travelled.distance(Vec3::X * 0.3) < 1e-4, "the free side went where the arrow was pulled: {travelled}");
            free += 1;
        }
        assert!(free > 100);
        deform.end(&mut mesh);
    }

    #[test]
    fn a_vertex_on_the_mirror_plane_stays_on_it() {
        let mut mesh = masked_ball(20_000);
        let before = mesh.positions.clone();
        let placement = Placement { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        let deform = Deform::begin(&mut mesh, ACROSS_X).unwrap();
        pull_x(&mut mesh, &deform, placement, 0.3);
        let on: Vec<usize> = (0..before.len()).filter(|&v| before[v].x == 0.0 && mesh.mask[v] < 1.0).collect();
        assert!(on.len() > 10, "{}", on.len());
        for &v in &on {
            assert_eq!(mesh.positions[v].x, 0.0);
        }
        deform.end(&mut mesh);
    }

    #[test]
    fn under_world_symmetry_the_halves_are_mirror_images_across_the_worlds_plane() {
        // Off the world's origin and turned, so the world's plane is not the object's.
        let placement = Placement {
            position: Vec3::new(0.15, -0.1, 0.2),
            rotation: Quat::from_rotation_y(0.5),
            scale: Vec3::splat(1.2),
        };
        let model = matrix(placement);
        let world = |p: Vec3| model.transform_point3(p);
        let mut mesh = primitives::build(Primitive::Sphere, 20_000, 0.5);
        for v in 0..mesh.vertex_count() {
            let x = world(mesh.positions[v]).x.abs();
            mesh.mask[v] = if x < 0.1 { 1.0 } else { 0.0 };
        }
        let before = mesh.positions.clone();
        let mirror = Mirror { axes: [true, false, false], world: Some(model), radial: None };

        // Nothing moved is nothing moved, to the bit, on either side.
        let deform = Deform::begin(&mut mesh, mirror).unwrap();
        deform.apply(&mut mesh, model, DMat4::IDENTITY);
        assert_eq!(mesh.positions, before);
        assert!(deform.end(&mut mesh).is_none());

        let deform = Deform::begin(&mut mesh, mirror).unwrap();
        let drag = pull_x(&mut mesh, &deform, placement, 0.3);
        let (plus, minus) = (glam::DVec3::X * 0.3, -glam::DVec3::X * 0.3);
        assert!(drag.transform_point3(glam::DVec3::ZERO).distance(plus) < 1e-5);
        let mut sides = [0, 0];
        for v in (0..before.len()).filter(|&v| mesh.mask[v] == 0.0) {
            let travelled = (world(mesh.positions[v]) - world(before[v])).as_dvec3();
            let left = world(before[v]).x < 0.0;
            assert!(travelled.distance(if left { minus } else { plus }) < 1e-4, "{travelled}");
            sides[usize::from(left)] += 1;
        }
        assert!(sides[0] > 1000 && sides[1] > 1000, "{sides:?}");
        deform.end(&mut mesh);
    }

    #[test]
    fn with_no_mask_nothing_is_begun() {
        let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
        assert!(Deform::begin(&mut mesh, Mirror::default()).is_none());
        // What a wiped mask leaves behind does not count either.
        mesh.mask.fill(0.0005);
        assert!(Deform::begin(&mut mesh, Mirror::default()).is_none());
        // No stroke was left open: the next one can begin.
        mesh.begin_stroke();
        assert!(mesh.end_stroke(false).is_none());

        // Masked all over, the drag begins and has nothing it can move.
        mesh.mask.fill(1.0);
        let before = mesh.positions.clone();
        let deform = Deform::begin(&mut mesh, Mirror::default()).expect("it is masked");
        assert_eq!(deform.free(), 0);
        deform.apply(&mut mesh, Mat4::IDENTITY, DMat4::from_translation(glam::DVec3::X));
        assert_eq!(mesh.positions, before);
        assert!(deform.end(&mut mesh).is_none());
    }

    /// Radial symmetry about the object's own Y, `count` times round.
    fn round_y(count: u32) -> Turns {
        Turns { radial: Radial { on: true, count, axis: RadialAxis::Y, custom_axis: Vec3::Y }, world: None }
    }

    /// Which way round Y an angle (in degrees) points: 0 along X.
    fn spoke(degrees: f32) -> Vec3 {
        Quat::from_rotation_y(degrees.to_radians()) * Vec3::X
    }

    /// A ball masked all over but for a patch round the middle at each of
    /// `centres` (angles round Y, in degrees), each reaching `half` degrees
    /// either side: tentacles to pull, with the mask between them.
    fn patched_ball(centres: &[f32], half: f32) -> Mesh {
        let mut mesh = primitives::build(Primitive::Sphere, 80_000, 0.5);
        for v in 0..mesh.vertex_count() {
            let p = mesh.positions[v];
            let flat = Vec3::new(p.x, 0.0, p.z);
            let near = centres.iter().any(|&c| flat.angle_between(spoke(c)).to_degrees() < half);
            mesh.mask[v] = if p.y.abs() < 0.2 && near { 0.0 } else { 1.0 };
        }
        mesh
    }

    /// Checks that each patch of `patched_ball(centres)` went the way the
    /// copy that carries the first patch onto it carries a pull of 0.3 along
    /// X, that the masked part stayed, and that one undo puts it all back.
    fn each_patch_went_as_its_copy_carries_the_pull(centres: &[f32], half: f32, axes: [bool; 3], turns: Turns) {
        let mut mesh = patched_ball(centres, half);
        let (before, mask) = (mesh.positions.clone(), mesh.mask.clone());
        let placement = Placement { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        let deform = Deform::begin(&mut mesh, Mirror { axes, world: None, radial: Some(turns) }).unwrap();
        pull_x(&mut mesh, &deform, placement, 0.3);
        let mut settings = Settings::default();
        settings.set_symmetry(axes, turns.radial, None);
        let group = settings.symmetry_group();
        let mut moved = vec![0; centres.len()];
        for v in 0..before.len() {
            let travelled = mesh.positions[v] - before[v];
            if mask[v] >= 1.0 {
                assert_eq!(travelled, Vec3::ZERO, "a masked vertex moved");
                continue;
            }
            let flat = Vec3::new(before[v].x, 0.0, before[v].z);
            let patch = (0..centres.len()).min_by(|&a, &b| {
                flat.angle_between(spoke(centres[a])).total_cmp(&flat.angle_between(spoke(centres[b])))
            });
            let patch = patch.unwrap();
            let copy = group
                .iter()
                .min_by(|a, b| {
                    let off = |op: &&sculpt_geom::SymmetryOp| op.apply_point(spoke(centres[0])).distance(spoke(centres[patch]));
                    off(a).total_cmp(&off(b))
                })
                .unwrap();
            let want = copy.apply(Vec3::X * 0.3);
            assert!(travelled.distance(want) < 1e-4, "patch at {}: went {travelled}, not {want}", centres[patch]);
            moved[patch] += 1;
        }
        assert!(moved.iter().all(|&n| n > 20), "every patch is free and moved: {moved:?}");
        let mut history = History::default();
        history.push_stroke(deform.end(&mut mesh).expect("the patches moved"));
        assert_eq!(history.steps(), (1, 0));
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(mesh.positions, before, "one undo step puts every copy back");
    }

    #[test]
    fn under_radial_symmetry_every_copy_round_the_axis_moves_as_the_others_do() {
        // Six tentacles, the first just off X: it follows the arrow out
        // along X, and each of the others goes out along its own spoke.
        let centres: Vec<f32> = (0..6).map(|k| 10.0 + 60.0 * k as f32).collect();
        each_patch_went_as_its_copy_carries_the_pull(&centres, 12.0, [false; 3], round_y(6));

        let mut mesh = patched_ball(&centres, 12.0);
        let mirror = Mirror { radial: Some(round_y(6)), ..Mirror::default() };
        let deform = Deform::begin(&mut mesh, mirror).unwrap();
        assert_eq!(deform.how(), ", repeated 6 times round Y");
        deform.end(&mut mesh);
    }

    #[test]
    fn under_radial_and_mirror_symmetry_the_mirror_images_move_too() {
        // Mirror X on top of six turns round Y: twelve patches, each turn of
        // the first and the mirror image of each.
        let mut centres: Vec<f32> = (0..6).map(|k| 10.0 + 60.0 * k as f32).collect();
        centres.extend((0..6).map(|k| 170.0 + 60.0 * k as f32));
        each_patch_went_as_its_copy_carries_the_pull(&centres, 6.0, [true, false, false], round_y(6));

        let mut mesh = patched_ball(&centres, 6.0);
        let mirror = Mirror { axes: [true, false, false], world: None, radial: Some(round_y(6)) };
        let deform = Deform::begin(&mut mesh, mirror).unwrap();
        assert_eq!(deform.how(), ", repeated 6 times round Y and mirrored across X");
        deform.end(&mut mesh);
    }

    #[test]
    fn under_radial_symmetry_one_copy_left_free_follows_the_gizmo() {
        // The one free tentacle points away from X, and still goes where
        // the arrow is pulled.
        let mut mesh = patched_ball(&[190.0], 12.0);
        let before = mesh.positions.clone();
        let placement = Placement { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        let deform = Deform::begin(&mut mesh, Mirror { radial: Some(round_y(6)), ..Mirror::default() }).unwrap();
        pull_x(&mut mesh, &deform, placement, 0.3);
        let mut free = 0;
        for v in (0..before.len()).filter(|&v| mesh.mask[v] == 0.0) {
            let travelled = mesh.positions[v] - before[v];
            assert!(travelled.distance(Vec3::X * 0.3) < 1e-4, "{travelled}");
            free += 1;
        }
        assert!(free > 20);
        deform.end(&mut mesh);
    }

    #[test]
    fn under_radial_symmetry_a_vertex_on_the_axis_stays_on_it() {
        // The top of the ball free: its pole is on the axis, where every
        // copy meets, and is moved by what all of them agree on.
        let mut mesh = masked_ball(80_000);
        let before = mesh.positions.clone();
        let on: Vec<usize> = (0..before.len())
            .filter(|&v| mesh.mask[v] == 0.0 && before[v].x.abs() < 1e-6 && before[v].z.abs() < 1e-6)
            .collect();
        assert!(!on.is_empty(), "the ball has a vertex at its pole");
        let placement = Placement { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        let deform = Deform::begin(&mut mesh, Mirror { radial: Some(round_y(5)), ..Mirror::default() }).unwrap();
        pull_x(&mut mesh, &deform, placement, 0.3);
        for &v in &on {
            let p = mesh.positions[v];
            assert!(p.x.abs() < 1e-5 && p.z.abs() < 1e-5, "the pole left the axis: {p}");
        }
        deform.end(&mut mesh);

        // A pull along the axis is the same for every copy, so every free
        // vertex goes all the way, on a border or not.
        let mut mesh = masked_ball(80_000);
        let (view, target, frame) = gizmo(placement, Orientation::Global);
        let px = |p: Vec3| view.project(p).unwrap();
        let deform = Deform::begin(&mut mesh, Mirror { radial: Some(round_y(5)), ..Mirror::default() }).unwrap();
        let grab = Vec3::Y * frame.arm;
        let mut drag = Drag::on(Handle::Move(1), frame, &target, &view, px(grab));
        drag.update(&view, px(grab + Vec3::Y * 0.3));
        deform.apply(&mut mesh, Mat4::IDENTITY, drag.world_delta());
        for v in (0..before.len()).filter(|&v| mesh.mask[v] == 0.0) {
            let travelled = mesh.positions[v] - before[v];
            assert!(travelled.distance(Vec3::Y * 0.3) < 1e-4, "{travelled}");
        }
        deform.end(&mut mesh);
    }

    #[test]
    fn with_radial_symmetry_off_the_drag_is_what_it_was_to_the_bit() {
        let placement = Placement {
            position: Vec3::new(0.3, -0.1, 0.2),
            rotation: Quat::from_rotation_z(0.4) * Quat::from_rotation_y(0.5),
            scale: Vec3::new(1.0, 1.4, 0.7),
        };
        let off = Turns { radial: Radial { on: false, ..round_y(6).radial }, world: None };
        let mut results = Vec::new();
        for mirror in [ACROSS_X, Mirror { radial: Some(off), ..ACROSS_X }] {
            let mut mesh = middle_masked_ball();
            let deform = Deform::begin(&mut mesh, mirror).unwrap();
            pull_x(&mut mesh, &deform, placement, 0.3);
            assert_eq!(deform.how(), ", each side the mirror image of the other across X");
            deform.end(&mut mesh);
            results.push(mesh.positions);
        }
        assert!(results[0] == results[1], "radial symmetry that is off changed the drag");
    }

    /// `cargo test --release masked -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn timing_on_four_million_triangles() {
        let mut mesh = masked_ball(4_000_000);
        let t = std::time::Instant::now();
        let deform = Deform::begin(&mut mesh, Mirror::default()).unwrap();
        let began = t.elapsed();
        let t = std::time::Instant::now();
        for i in 1..=10 {
            deform.apply(&mut mesh, Mat4::IDENTITY, DMat4::from_translation(glam::DVec3::Y * 0.01 * i as f64));
        }
        let applied = t.elapsed() / 10;
        let t = std::time::Instant::now();
        mesh.flush_normals();
        let normals = t.elapsed();
        let t = std::time::Instant::now();
        let delta = deform.end(&mut mesh);
        println!(
            "{} of {} vertices free: begin {began:?}, each frame {applied:?} (+ normals {normals:?}), end {:?}",
            delta.as_ref().map_or(0, |d| d.bytes() / 28),
            mesh.vertex_count(),
            t.elapsed()
        );
    }
}
