//! Scripted runs of the deformers: `--demo-deform <what>`.
//!
//! A deformer's name (`bend`, `arc`, `twist`, `taper`, `flatten`, `inflate`,
//! `stretch`, `lattice`) puts a capsule in the scene with that deformer
//! picked and set to something worth seeing, not yet applied, so the
//! screenshot shows the model, the handles and the card together. `masked`
//! is a bend of a capsule with its lower part masked; `three` a bend of three
//! posts in a row, deformed as one.
//!
//! `play` works them through the routines the window and the keys feed,
//! prints a line per check, and leaves the last state up for a picture.

use glam::{Quat, Vec3};
use serde_json::json;
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::super::Running;
use super::handles::Handle;
use crate::deform::{Anchor, Kind, Params};
use crate::doc::{Action, MaskOp, Tool};
use crate::mesh::primitives::{self, Primitive};
use crate::ui::deform::Ask;

fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

fn bits(points: &[Vec3]) -> Vec<[u32; 3]> {
    points.iter().map(|p| p.to_array().map(f32::to_bits)).collect()
}

/// The settings each picture shows.
fn showing(kind: Kind) -> Params {
    let mut p = Params { kind, ..Params::default() };
    match kind {
        Kind::Bend => p.angle = 75.0,
        Kind::BendArc => p.tip = [0.0, -0.45, 0.75],
        Kind::Twist => p.angle = 160.0,
        Kind::Taper => p.scales = [1.0, 0.35],
        Kind::Flatten => p.flatten = 0.45,
        Kind::Inflate => (p.inflate, p.range) = (0.04, [0.3, 0.8]),
        Kind::Stretch => p.stretch = 1.6,
        Kind::Lattice => {
            p.lattice = p.lattice.with_count([3, 4, 3]);
            let top = p.lattice.index([2, 3, 1]);
            p.lattice.moved[top] = [0.45, 0.15, 0.0];
            p.lattice.picked = top;
        }
    }
    p
}

/// A capsule three times as tall as it is wide: something to bend.
fn tall_capsule(triangles: usize) -> crate::mesh::Mesh {
    let mesh = primitives::build(Primitive::Capsule, triangles, 0.3);
    let positions = mesh.positions.iter().map(|p| *p * Vec3::new(1.0, 1.6, 1.0)).collect();
    crate::mesh::Mesh::new(positions, mesh.indices.clone(), None)
}

impl Running {
    pub(in crate::app) fn deform_demo(&mut self, what: &str) {
        if what == "play" {
            return self.deform_play();
        }
        let kind = match what {
            "arc" => Kind::BendArc,
            "masked" | "three" => Kind::Bend,
            other => Kind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(other)).unwrap_or(Kind::Bend),
        };
        self.deform_capsule();
        if kind == Kind::Twist {
            // A round shape turns into itself; a square one shows the twist.
            let cube = primitives::build(Primitive::Cube, 200_000, 0.3);
            let positions = cube.positions.iter().map(|p| *p * Vec3::new(1.0, 2.5, 1.0)).collect();
            self.replace_mesh(crate::mesh::Mesh::new(positions, cube.indices.clone(), None), false);
        }
        if what == "masked" {
            self.deform_mask_below(-0.3);
        }
        if what == "three" {
            self.deform_posts();
        }
        self.act(Action::Deform(Ask::Pick(Some(kind))));
        self.doc.opts.deform.params = Params { anchor: Anchor::Start, ..showing(kind) };
        self.deform_frame();
        self.deform_look();
    }

    /// A tall capsule in place of the scene's object, standing up, with nothing to undo.
    fn deform_capsule(&mut self) {
        self.replace_mesh(tall_capsule(200_000), false);
        let object = self.doc.scene.active_mut();
        (object.position, object.rotation, object.scale) = (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
        self.scene_changed();
    }

    /// Masks everything of the active object below `y`, with a soft edge.
    fn deform_mask_below(&mut self, y: f32) {
        if let Some(delta) =
            self.doc.mesh.remap_mask(|mesh, v, _| ((y + 0.1 - mesh.positions[v as usize].y) / 0.2).clamp(0.0, 1.0))
        {
            self.doc.history.push_stroke(delta);
        }
    }

    /// Two more capsules beside the first, all three selected, the first active.
    fn deform_posts(&mut self) -> [u64; 3] {
        let first = self.doc.scene.active_id();
        let mut ids = [first; 3];
        for (i, x) in [0.9f32, 1.8].into_iter().enumerate() {
            let id = self.add_object("Post", tall_capsule(60_000));
            if let Some(object) = self.doc.scene.get_mut(id) {
                object.position = Vec3::new(x, 0.0, 0.0);
            }
            ids[i + 1] = id;
        }
        self.act(Action::SelectObject(first));
        self.select_also(ids[1]);
        self.select_also(ids[2]);
        // Several objects take the world's axes; the bend is along Y toward Z.
        self.scene_changed();
        ids
    }

    /// Frames what is on screen from a little to the side and above.
    fn deform_look(&mut self) {
        self.doc.camera.yaw = 35.0;
        self.doc.camera.pitch = 12.0;
        self.frame_view();
        self.doc.camera.update(10.0);
        self.sync_view();
        println!("{}", self.ui.status.0);
    }

    /// Where a handle of the deformer is in the world.
    fn deform_handle(&self, handle: Handle) -> Option<Vec3> {
        self.deform_handles().into_iter().find(|h| h.0 == handle).map(|h| h.1)
    }

    /// Works the deformers as a person would, and prints what came of it.
    fn deform_play(&mut self) {
        let steps = |app: &Running| app.doc.history.steps().0;
        self.deform_capsule();
        let rest = bits(&self.doc.mesh.positions);

        // Picking a deformer brings the gizmo up with its handles.
        self.doc.tool = Tool::Sculpt;
        self.act(Action::Deform(Ask::Pick(Some(Kind::Bend))));
        self.deform_frame();
        self.deform_look();
        say("picking Bend brings the Transform tool up", self.doc.tool == Tool::Transform && self.deform_on());
        say("with the range's two handles where the gizmo's arrows were", {
            self.demo_frame().is_none()
                && self.deform_handle(Handle::Start).is_some()
                && !self.deform_shapes().is_empty()
        });
        say("and nothing moved yet, nor the mesh held", {
            bits(&self.doc.mesh.positions) == rest && !self.deform_pending() && !self.deform_holds_mesh()
        });

        // A slider: the model follows, and Esc puts it back to the bit.
        self.doc.opts.deform.params.angle = 60.0;
        self.deform_frame();
        say("an angle bends the model", bits(&self.doc.mesh.positions) != rest && self.doc.opts.deform.pending);
        let count = steps(self);
        self.deform_key(KeyCode::Escape);
        self.deform_frame();
        say("Esc puts every vertex back as it was", bits(&self.doc.mesh.positions) == rest);
        say(
            "leaving nothing to undo, and the angle at 0",
            steps(self) == count && self.doc.opts.deform.params.angle == 0.0,
        );

        // Enter keeps it; one Ctrl+Z takes it back exactly.
        self.doc.opts.deform.params.angle = 45.0;
        self.deform_frame();
        let bent = bits(&self.doc.mesh.positions);
        self.deform_key(KeyCode::Enter);
        self.deform_frame();
        say("Enter keeps it as one undo step", steps(self) == count + 1 && bits(&self.doc.mesh.positions) == bent);
        println!("     {}", self.ui.status.0);
        self.act(Action::Undo);
        self.deform_frame();
        say("which Ctrl+Z takes back to the bit", bits(&self.doc.mesh.positions) == rest);
        self.act(Action::Redo);
        self.deform_frame();
        say("and Ctrl+Shift+Z puts back", bits(&self.doc.mesh.positions) == bent);
        self.act(Action::Undo);
        self.deform_frame();

        // A deformation on the model is applied before any other command.
        self.doc.opts.deform.params.angle = 30.0;
        self.deform_frame();
        let count = steps(self);
        self.act(Action::Mask(MaskOp::Invert));
        say("a command finds the deformation applied first", steps(self) == count + 2 && !self.deform_pending());
        self.act(Action::Undo);
        self.act(Action::Undo);
        self.deform_frame();
        say("and two Z take both back", bits(&self.doc.mesh.positions) == rest);

        // The range's end handle, dragged down the axis with the pointer.
        self.sync_view();
        let (Some(end), Some(start)) = (self.deform_handle(Handle::End), self.deform_handle(Handle::Start)) else {
            return say("the range has its handles", false);
        };
        let took = self.demo_drag(&[end, end.lerp(start, 0.4)], false);
        self.deform_frame();
        let range = self.doc.opts.deform.params.range;
        say("a press on the end handle takes the drag", took && self.transform_dragging());
        println!("     the range is now {:.0}% to {:.0}%", range[0] * 100.0, range[1] * 100.0);
        say("and pulling it down the axis shortens the range", (range[1] - 0.6).abs() < 0.05 && range[0] == 0.0);
        self.pointer_released(MouseButton::Left);
        say("letting go ends the drag", !self.transform_dragging());

        // Bend Arc: the tip handle goes where it is dragged, and the model's end with it.
        self.act(Action::Deform(Ask::Pick(Some(Kind::BendArc))));
        self.doc.opts.deform.params.range = [0.0, 1.0];
        self.deform_frame();
        self.sync_view();
        if let Some(tip) = self.deform_handle(Handle::Tip) {
            let to = tip + Vec3::new(0.0, -0.3, 0.5);
            say("a press on the tip handle takes the drag", self.demo_drag(&[tip, to], true));
            self.deform_frame();
            let top = self.doc.mesh.positions.iter().copied().max_by(|a, b| a.z.total_cmp(&b.z)).unwrap_or_default();
            let landed = self.deform_handle(Handle::Tip).unwrap_or_default();
            println!("     the tip is at {landed}, the model reaches {top}");
            say("and the end of the capsule comes round toward it", top.z > 0.3);
            self.deform_cancel();
        } else {
            say("Bend Arc has a tip handle", false);
        }

        // Lattice: a point of the cage dragged pulls the clay near it.
        self.act(Action::Deform(Ask::Pick(Some(Kind::Lattice))));
        self.deform_frame();
        self.sync_view();
        let top = self.doc.opts.deform.params.lattice.index([1, 2, 1]);
        if let Some(point) = self.deform_handle(Handle::Point(top)) {
            let before = self.doc.mesh.bounds().max.y;
            self.demo_drag(&[point, point + Vec3::Y * 0.3], true);
            self.deform_frame();
            let picked = self.doc.opts.deform.params.lattice.picked == top;
            say(
                "dragging the cage's top point picks it and raises the top",
                picked && self.doc.mesh.bounds().max.y > before + 0.05,
            );
            self.deform_cancel();
        } else {
            say("the cage has its points", false);
        }

        // Symmetry: with Mirror X on, a twist about Y is said to break it.
        self.doc.edit_symmetry(|symmetry| symmetry.mirror[0] = true);
        self.act(Action::Deform(Ask::Pick(Some(Kind::Twist))));
        self.doc.opts.deform.params.angle = 90.0;
        self.deform_frame();
        let warned = self.ui.status.0.contains("not mirror-symmetric");
        say(
            "a twist of a mirrored model is said to break the mirror",
            warned && self.ui.status.1 == crate::ui::theme::ERROR,
        );
        self.deform_cancel();
        self.act(Action::Deform(Ask::Pick(Some(Kind::Bend))));
        self.doc.opts.deform.params.angle = 90.0;
        self.deform_frame();
        say("a bend toward Z is not", !self.ui.status.0.contains("not mirror-symmetric"));
        self.deform_cancel();

        // A point of the Lattice's cage takes its mirror image across X with it.
        self.act(Action::Deform(Ask::Pick(Some(Kind::Lattice))));
        self.deform_frame();
        self.sync_view();
        let cage = &self.doc.opts.deform.params.lattice;
        let (side, other) = (cage.index([0, 2, 1]), cage.index([2, 2, 1]));
        if let Some(point) = self.deform_handle(Handle::Point(side)) {
            self.demo_drag(&[point, point + Vec3::new(-0.15, 0.2, 0.0)], true);
            self.deform_frame();
            let cage = &self.doc.opts.deform.params.lattice;
            let [a, b] = [cage.moved[side], cage.moved[other]];
            println!("     the point moved {a:?}, its mirror image {b:?}");
            say("a cage point dragged takes its mirror image across X with it", a[0] != 0.0 && b == [-a[0], a[1], a[2]]);
            say("and the mirrored model is not said to come out lopsided", !self.ui.status.0.contains("not mirror-symmetric"));
            self.deform_cancel();
        } else {
            say("the cage has its points", false);
        }
        // The bend is what the next steps go on with.
        self.act(Action::Deform(Ask::Pick(Some(Kind::Bend))));
        self.doc.edit_symmetry(|symmetry| symmetry.mirror[0] = false);

        // A sculpt layer that records but is switched off refuses it, as it refuses a stroke.
        let (id, _) = self.doc.mesh.add_layer(None);
        let _ = self.doc.mesh.set_layer(id, false, 100);
        self.doc.opts.deform.params.angle = 40.0;
        self.deform_frame();
        let refused = bits(&self.doc.mesh.positions) == rest && self.ui.status.1 == crate::ui::theme::ERROR;
        println!("     {}", self.ui.status.0);
        say("a hidden recording layer refuses the bend, in words", refused && self.doc.opts.deform.params.angle == 0.0);
        self.doc.mesh.layers = None;

        // The Claude bridge's tool: applied, one step.
        let count = steps(self);
        let answer = self.call("deform", &json!({ "kind": "taper", "start_size": 100, "end_size": 40, "axis": "y" }));
        println!("     {answer:?}");
        say("the bridge's deform tool tapers it as one undo step", answer.is_ok() && steps(self) == count + 1);
        self.act(Action::Undo);
        self.deform_frame();
        say("which Ctrl+Z takes back", bits(&self.doc.mesh.positions) == rest);

        // Three posts bent as one row, and one Z for all three.
        let ids = self.deform_posts();
        let shapes = |app: &mut Running| -> Vec<Vec<[u32; 3]>> {
            ids.iter()
                .map(|&id| {
                    let mesh = app.doc.scene.get(id).map(|o| o.mesh).unwrap_or_default();
                    super::mesh_of(&mut app.doc, mesh).map_or(Vec::new(), |m| bits(&m.positions))
                })
                .collect()
        };
        let before = shapes(self);
        let count = steps(self);
        self.act(Action::Deform(Ask::Pick(Some(Kind::Bend))));
        self.doc.opts.deform.params.angle = 60.0;
        self.deform_frame();
        let bent = shapes(self);
        say("a bend of three selected posts bends all three", bent.iter().zip(&before).all(|(a, b)| a != b));
        self.deform_key(KeyCode::Enter);
        say("kept as one step for each, tied together", steps(self) == count + 3);
        self.act(Action::Undo);
        self.deform_frame();
        say("one Ctrl+Z puts all three back to the bit", shapes(self) == before);
        self.act(Action::Redo);
        self.deform_frame();
        say("one Ctrl+Shift+Z bends all three again", shapes(self) == bent);
        self.deform_look();
    }
}
