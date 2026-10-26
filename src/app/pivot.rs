//! Edit Pivot, on the app's side: the Pivot part of the Transform card.
//!
//! An object's pivot is its centre (scene/origin.rs): the point the gizmo
//! sits on, what the object turns and resizes about, and where its own
//! (Local) mirror planes and radial axis pass, along the axes it is turned
//! by. A model brought in from elsewhere often has it far off, or tipped
//! over. These put it right without moving anything in the world, as
//! Nomad's pivot editing does:
//!
//! - **Edit Pivot** gives the gizmo to the pivot: an arrow moves it, a ring
//!   turns its axes, and a click on the object puts it on the surface there
//!   (a drag still orbits). The scale cubes are put away: a pivot has no
//!   size. It goes off again when the Transform tool is put away, so the
//!   gizmo next time moves the object, as it says it does.
//! - **Pivot to Centre** is Set Origin to Mesh (app/origin.rs).
//! - **Align to World** turns the pivot's axes back to the world's.
//!
//! Each is one undo step, made as Set Origin to Mesh makes its own, and
//! refused for the same reasons (`origin_refusal`). A turn is refused as
//! well while the mesh is on a subdivision level below its finest (the finer
//! levels are kept in the axes they were made in) and while an object that
//! shows the mesh is stretched more one way than another (it would shear).

use glam::{Quat, Vec2, Vec3};

use super::Running;
use crate::gizmo::{Handles, Placement, Target};
use crate::scene::{OriginMoved, Space};

/// A turn of the pivot smaller than this, in radians, is none: a ring
/// clicked and not dragged, or rounding.
const LEAST_TURN: f32 = 1e-5;

/// A move of the pivot shorter than this fraction of the mesh's size is none.
const LEAST_MOVE: f32 = 1e-6;

/// How far the pointer may wander, in points, between pressing on the object
/// and letting go, for it to be a click that places the pivot, not an orbit.
const CLICK_SLOP: f32 = 4.0;

/// The handles a pivot has: no size of its own to change.
const PIVOT_HANDLES: Handles = Handles { moves: true, turns: true, scales: false };

/// The status line after the pivot of `name` was moved or turned.
fn placed_line(name: &str, moved: &OriginMoved, turned: bool, shifted: bool, local_symmetry: Option<String>) -> String {
    let what = match (shifted, turned) {
        (true, true) => "Moved and turned",
        (false, true) => "Turned",
        _ => "Moved",
    };
    let mut text = format!("{what} the pivot of \"{name}\". Nothing moved in the scene.");
    match moved.objects.len().saturating_sub(1) {
        0 => {}
        1 => text += " Its instance shares the mesh, and stays where it is too.",
        count => text += &format!(" Its {count} instances share the mesh, and stay where they are too."),
    }
    if let Some(summary) = local_symmetry {
        text += &format!(" Its own symmetry ({summary}) goes through the pivot, along its axes.");
    }
    text + " Undo (Ctrl+Z) puts it back."
}

impl Running {
    /// Whether the gizmo is on the active object's pivot (Edit Pivot).
    pub(super) fn pivot_on(&self) -> bool {
        self.transform_active() && self.doc.opts.edit_pivot
    }

    /// The gizmo's target while Edit Pivot is on: the active object's own
    /// placement, which is its pivot, with no scale handles.
    pub(super) fn pivot_target(&self) -> Option<(Target, Handles)> {
        let (scene, slot) = (&self.doc.scene, &self.doc.mesh);
        let object = scene.active();
        if !object.visible {
            return None;
        }
        let bounds = scene.bounds_of(object, slot);
        let radius = if bounds.is_empty() { 0.5 } else { bounds.size().length() * 0.5 };
        let target = Target { placement: Placement::of(object), radius, turns: true, scales: false };
        Some((target, PIVOT_HANDLES))
    }

    /// Why the pivot cannot be turned just now, if it cannot (see the top of
    /// this file).
    fn pivot_turn_refusal(&self) -> Option<String> {
        let doc = &self.doc;
        let name = &doc.scene.active().name;
        if doc.mesh.levels.as_ref().is_some_and(|levels| levels.current() < levels.top()) {
            return Some(format!(
                "The pivot was not turned: \"{name}\" is below its finest subdivision level, and the finer levels \
                 keep the axes they were made in. Step up to the finest level (the Levels card), then turn it. \
                 Moving it works on any level."
            ));
        }
        let stretched = doc.scene.stretched_unevenly();
        if stretched.is_empty() {
            return None;
        }
        let who = match stretched.as_slice() {
            [one] if one == name => format!("\"{name}\" is"),
            [one] => format!("its instance \"{one}\" is"),
            several => format!("{} are", crate::ui::question::listed(several)),
        };
        Some(format!(
            "The pivot was not turned: {who} stretched more one way than another, and turning the axes would \
             shear it. Make the Scale the same on all three axes (the Objects card), then turn it. Moving it works."
        ))
    }

    /// Puts the active object's pivot at `position`, turned by `rotation`,
    /// leaving the object and its instances where they are. One undo step,
    /// called `what`, as its toast is. Nothing at all when neither changes.
    pub(super) fn place_pivot(&mut self, position: Vec3, rotation: Quat, what: &str) {
        let (shift, turn) = self.doc.scene.origin_for(position, rotation);
        let size = self.doc.mesh.bounds().size().max_element().max(f32::MIN_POSITIVE);
        let shifted = shift.length() > size * LEAST_MOVE;
        let turned = turn.angle_between(Quat::IDENTITY) > LEAST_TURN;
        if !shifted && !turned {
            return;
        }
        if let Some(why) = self.origin_refusal() {
            return self.status(why, false);
        }
        if let Some(why) = turned.then(|| self.pivot_turn_refusal()).flatten() {
            return self.status(why, false);
        }
        let shift = if shifted { shift } else { Vec3::ZERO };
        let turn = if turned { turn } else { Quat::IDENTITY };

        let name = self.doc.scene.active().name.clone();
        let doc = &mut self.doc;
        let before = doc.scene.state();
        let moved = doc.scene.reorigin(shift, turn, &mut doc.mesh);
        doc.history.name_next(Some(what.to_owned()));
        doc.history.push_scene(before);
        doc.history.name_next(None);
        self.origin_was_moved(&moved);
        let symmetry = self.doc.scene.active_symmetry();
        let local = (symmetry.space == Space::Local && !symmetry.is_off()).then(|| symmetry.summary().to_string());
        self.toast(what);
        self.status(placed_line(&name, &moved, turned, shifted, local), true);
    }

    /// Align to World: the pivot's axes back to the world's, the pivot where
    /// it is and the object too.
    pub(super) fn align_pivot(&mut self) {
        let object = self.doc.scene.active();
        if object.rotation.angle_between(Quat::IDENTITY) <= LEAST_TURN {
            let text = format!("The pivot of \"{}\" is already lined up with the world.", object.name);
            return self.status(text, true);
        }
        self.place_pivot(object.position, Quat::IDENTITY, "Align Pivot to World");
    }

    /// A press on the view with Edit Pivot on and off the handles. On the
    /// active object itself (not a mirror copy or an instance) it is kept,
    /// and true: if it is let go without wandering, the pivot goes there
    /// (`pivot_released`). Either way the drag orbits.
    pub(super) fn pivot_pressed(&mut self, pos: Vec2) -> bool {
        let (origin, dir) = self.view.ray(pos);
        let doc = &self.doc;
        let picked = doc.scene.pick(origin, dir, &doc.mesh);
        let hit = picked.filter(|p| p.id == doc.scene.active_id() && p.copy == 0).map(|p| p.pos);
        self.transform.pivot_click = hit.map(|at| (pos, at));
        hit.is_some()
    }

    /// The button came up: a click kept by `pivot_pressed` puts the pivot on
    /// the surface where it was, its axes as they are.
    pub(super) fn pivot_released(&mut self) {
        let Some((pressed, at)) = self.transform.pivot_click.take() else { return };
        let slop = CLICK_SLOP * self.egui_ctx.pixels_per_point();
        if self.pivot_on() && pressed.distance(self.input.pointer) <= slop {
            let rotation = self.doc.scene.active().rotation;
            self.place_pivot(at, rotation, "Pivot to Surface");
        }
    }

    // ---- for visual checks ---------------------------------------------------

    /// The box round the active object in the world, as drawn.
    fn demo_world_box(&self) -> (Vec3, Vec3) {
        let model = self.doc.scene.active().matrix();
        let points = self.doc.mesh.positions.iter().map(|&p| model.transform_point3(p));
        points.fold((Vec3::MAX, Vec3::MIN), |(low, high), p| (low.min(p), high.max(p)))
    }

    /// `SCULPT_DEMO_GIZMO=pivot` (with `--demo-gizmo`): works Edit Pivot the
    /// way a person would, through the real pointer and undo routines, and
    /// prints a line per check (ok / FAIL): an arrow drag, a ring drag, a
    /// click on the surface, Align to World, Pivot to Centre, Undo, and Edit
    /// Pivot going off with the tool. At every step the object must not have
    /// moved in the world. The picture is the gizmo on the pivot at the end.
    pub(super) fn pivot_play(&mut self) {
        use winit::event::MouseButton;

        use super::transform::green_ring;
        use crate::doc::{Action, Tool};

        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let steps = |app: &Running| app.doc.history.steps().0;

        // The cube of three, alone, seen from a little above.
        let ids = self.demo_three();
        self.act(Action::SelectObject(ids[1]));
        self.doc.camera.pitch = 35.0;
        self.sync_view();
        let start = self.doc.scene.active().placement();
        let world = self.demo_world_box();
        let size = (world.1 - world.0).max_element();
        let stayed = |app: &Running| {
            let now = app.demo_world_box();
            (now.0 - world.0).abs().max((now.1 - world.1).abs()).max_element() < size * 1e-4
        };

        self.doc.opts.edit_pivot = true;
        let Some(frame) = self.demo_frame() else { return say("the gizmo is up on the pivot", false) };
        say("the gizmo sits on the object's pivot", frame.pivot.distance(start.position) < 1e-4);

        // An arrow: the pivot goes, the object stays.
        let count = steps(self);
        let grab = frame.pivot + frame.axis(0) * frame.arm * 1.1;
        let pull = frame.axis(0) * 0.4;
        say("a press on the red arrow takes the drag", self.demo_drag(&[grab, grab + pull], false));
        say("nothing has moved mid-drag", self.doc.scene.active().placement() == start && stayed(self));
        self.pointer_released(MouseButton::Left);
        let now = self.doc.scene.active();
        println!("     {}", self.ui.status.0);
        say("the pivot went along the arrow", (now.position - start.position).distance(pull) < 0.02);
        say("and the object stayed where it was", stayed(self));
        say("as one undo step", steps(self) == count + 1);

        // A ring: the pivot's axes turn, the object stays.
        let Some(frame) = self.demo_frame() else { return say("the gizmo is still up", false) };
        let was = self.doc.scene.active().rotation;
        say("a press on the green ring takes the drag", {
            self.demo_drag(&green_ring(&frame, std::f32::consts::FRAC_PI_2), true)
        });
        let turned = self.doc.scene.active().rotation.angle_between(was);
        println!("     turned by {:.1} degrees; {}", turned.to_degrees(), self.ui.status.0);
        say("the pivot's axes turned a quarter", (turned - std::f32::consts::FRAC_PI_2).abs() < 0.05);
        say("and the object stayed where it was", stayed(self));
        say("as one more undo step", steps(self) == count + 2);

        // Align to World: back on the world's axes, still in place.
        self.act(Action::AlignPivot);
        let aligned = self.doc.scene.active().rotation.angle_between(glam::Quat::IDENTITY) < 1e-4;
        say("Align to World lines the pivot up with the world", aligned && stayed(self));

        // A click on the surface puts the pivot there; the click's ray is
        // aimed through the middle of the object, so it meets the near side.
        let middle = (world.0 + world.1) * 0.5;
        let px = self.view.project(middle).unwrap_or(self.view.size * 0.5);
        let (origin, dir) = self.view.ray(px);
        let surface = self.doc.scene.pick(origin, dir, &self.doc.mesh).map(|p| p.pos);
        self.pointer_moved(px, None);
        self.pointer_pressed(MouseButton::Left, None);
        self.pointer_released(MouseButton::Left);
        let at = self.doc.scene.active().position;
        say("a click on the object puts the pivot on its surface there", {
            surface.is_some_and(|s| s.distance(at) < size * 1e-3) && stayed(self)
        });
        println!("     {}", self.ui.status.0);

        // Pivot to Centre, then all the way back with Undo.
        self.act(Action::SetOriginToMesh);
        say("Pivot to Centre brings it back into the middle", {
            self.doc.scene.origin_off_mesh(&self.doc.mesh).is_none() && stayed(self)
        });
        while steps(self) > count {
            self.act(Action::Undo);
        }
        let back = self.doc.scene.active().placement();
        say("Undo puts the pivot back where it began", {
            back.position.distance(start.position) < 1e-5 && back.rotation.angle_between(start.rotation) < 1e-5 && stayed(self)
        });

        // Stretched unevenly, a turn is refused and says why; a move is not.
        self.doc.scene.active_mut().scale = Vec3::new(1.0, 2.0, 1.0);
        let count = steps(self);
        self.place_pivot(start.position, glam::Quat::from_rotation_x(0.5), "Turn Pivot");
        say("a turn through an uneven stretch is refused", {
            steps(self) == count && self.ui.status.1 != crate::ui::theme::OK
        });
        println!("     {}", self.ui.status.0);
        self.doc.scene.active_mut().scale = Vec3::ONE;

        // The gizmo moves the object again once the tool is put away.
        self.set_tool(Tool::Sculpt);
        say("Edit Pivot goes off with the Transform tool", !self.doc.opts.edit_pivot);
        self.set_tool(Tool::Transform);
        self.doc.opts.edit_pivot = true;
        self.input.inside = false;
        self.sync_view();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sculpt_symmetry::Placement as Stand;

    #[test]
    fn the_status_line_says_what_happened_and_what_came_with_it() {
        let stand = Stand::new(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
        let one = OriginMoved { shift: Vec3::X, objects: vec![(1, stand, stand)] };
        assert_eq!(
            placed_line("Head", &one, false, true, None),
            "Moved the pivot of \"Head\". Nothing moved in the scene. Undo (Ctrl+Z) puts it back."
        );
        let two = OriginMoved { shift: Vec3::X, objects: vec![(1, stand, stand); 2] };
        assert_eq!(
            placed_line("Head", &two, true, false, Some("X".into())),
            "Turned the pivot of \"Head\". Nothing moved in the scene. Its instance shares the mesh, and stays \
             where it is too. Its own symmetry (X) goes through the pivot, along its axes. Undo (Ctrl+Z) puts it back."
        );
        assert!(placed_line("Head", &one, true, true, None).starts_with("Moved and turned the pivot"));
    }
}
