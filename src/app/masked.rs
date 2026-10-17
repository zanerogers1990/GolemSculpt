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

use glam::{Affine3A, DMat4, DVec3, Mat3, Mat4, Vec3};
use rayon::prelude::*;

use crate::mesh::{Mesh, StrokeDelta};

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
    /// The mirror planes the drag is repeated across, if any.
    sides: Option<Sides>,
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
        let mut sides = Sides::new(mirror);
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
        let maps = std::cell::RefCell::new(vec![Affine3A::IDENTITY; if sides.is_some() { 27 } else { 1 }]);
        Some(Deform { uid: mesh.uid, verts, leaves, sides, maps })
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
        let maps: Vec<Affine3A> = match &self.sides {
            None => vec![Affine3A::from_mat4(local.as_mat4())],
            Some(sides) => sides.maps(local).iter().map(|m| Affine3A::from_mat4(m.as_mat4())).collect(),
        };
        let sides = self.sides.as_ref();
        mesh.move_listed(&self.verts, |_, vertex| {
            let map = match sides {
                None => &maps[0],
                Some(sides) => &maps[sides.code(vertex.start)],
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
        let sides = self.sides;
        let linear = |start: Vec3| {
            let at = sides.as_ref().map_or(0, |sides| sides.code(start));
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

    const ACROSS_X: Mirror = Mirror { axes: [true, false, false], world: None };

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
        let mirror = Mirror { axes: [true, false, false], world: Some(model) };

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
