//! Deforming around a mask with the gizmo. With part of an object masked, a
//! gizmo drag does not move the object: it moves, turns or stretches the
//! unmasked part of its surface and leaves the masked part exactly where it
//! is. Mask the body, then pull a limb out of it.
//!
//! It runs as a stroke, like a brush: the vertices are written through the
//! mesh's stroke calls, so the renderer follows each frame and the whole drag
//! is one ordinary undo step.

use glam::{Affine3A, DMat4, Mat4};
use rayon::prelude::*;

use crate::mesh::{Mesh, StrokeDelta};

/// Mask values at or below this count as no mask at all, as in the Unity
/// app: what an erased or blurred-out mask leaves behind must not switch the
/// gizmo from moving the object to reshaping it.
const MASKED: f32 = 0.001;

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
}

impl Deform {
    /// Opens the stroke. `None`, and nothing is begun, when the mesh has no
    /// mask to speak of: the gizmo then moves the object as a whole, which
    /// looks the same and costs nothing.
    pub fn begin(mesh: &mut Mesh) -> Option<Deform> {
        let mask = &mesh.mask;
        if !mask.par_iter().with_min_len(1 << 16).any(|&m| m > MASKED) {
            return None;
        }
        let verts: Vec<u32> =
            (0..mask.len() as u32).into_par_iter().with_min_len(1 << 14).filter(|&v| mask[v as usize] < 1.0).collect();
        let leaves = mesh.leaves_of(&verts);
        mesh.begin_stroke();
        Some(Deform { uid: mesh.uid, verts, leaves })
    }

    /// How many vertices the drag can move.
    #[cfg(test)]
    pub fn free(&self) -> usize {
        self.verts.len()
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
            Affine3A::IDENTITY
        } else {
            let model = model.as_dmat4();
            Affine3A::from_mat4((model.inverse() * world * model).as_mat4())
        };
        mesh.move_listed(&self.verts, |_, vertex| {
            let moved = local.transform_point3(vertex.start);
            // A half-masked vertex goes half the way, which gives a pulled
            // limb a smooth root instead of a torn ring. Written as a step
            // from the start so that no change is exactly no change.
            let weight = 1.0 - vertex.mask;
            Some(if weight >= 1.0 { moved } else { vertex.start + (moved - vertex.start) * weight })
        });
        mesh.positions_changed(&self.leaves);
    }

    /// Closes the stroke and returns its undo record. `None` if the surface
    /// ended where it began.
    pub fn end(self, mesh: &mut Mesh) -> Option<StrokeDelta> {
        if mesh.uid != self.uid {
            return None;
        }
        match mesh.end_stroke(false) {
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

        let deform = Deform::begin(&mut mesh).expect("part of it is masked");
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
        let deform = Deform::begin(&mut mesh).unwrap();
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
        let deform = Deform::begin(&mut mesh).unwrap();
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

    #[test]
    fn with_no_mask_nothing_is_begun() {
        let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
        assert!(Deform::begin(&mut mesh).is_none());
        // What a wiped mask leaves behind does not count either.
        mesh.mask.fill(0.0005);
        assert!(Deform::begin(&mut mesh).is_none());
        // No stroke was left open: the next one can begin.
        mesh.begin_stroke();
        assert!(mesh.end_stroke(false).is_none());

        // Masked all over, the drag begins and has nothing it can move.
        mesh.mask.fill(1.0);
        let before = mesh.positions.clone();
        let deform = Deform::begin(&mut mesh).expect("it is masked");
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
        let deform = Deform::begin(&mut mesh).unwrap();
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
