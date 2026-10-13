//! The Tube tool: tubes drawn along curves and fused into the model.
//!
//! Draw a stroke and get a smooth tube along it, for blocking out horns,
//! arms, legs and tails; then shape it by its points and fuse it into the
//! model. A tube stays live after it is drawn: its points can be dragged,
//! added, removed, made sharp and sized, and its radius, taper, ends and
//! smoothness changed, with the tube rebuilding as you go. Merge fuses every
//! live tube into the object it was drawn on (one voxel union, with a smooth
//! fillet); Keep Separate makes each its own sculptable object. Either is one
//! step on the scene's history, and undoing it hands the tubes back live.
//!
//! A tube drawn on an object belongs to it: its points are kept in that
//! object's own space (it moves with it) and it follows the brushes'
//! symmetry, so one stroke with X mirroring on makes both arms, and one with
//! radial symmetry on makes a tube for each repeat round the axis (twice as
//! many with a mirror as well).
//!
//! While the tool is up it has the pointer:
//!
//! | Input | Action |
//! | --- | --- |
//! | Drag from the model | Draw a new tube (Ctrl + drag draws one in mid-air) |
//! | Drag a point | Move it (a point on the surface slides over the model) |
//! | Shift + drag an end point | Extend the tube from that end |
//! | Click the selected tube's curve | Add a point there, and keep dragging it |
//! | Click another tube | Select it |
//! | Double-click a point | Sharp / smooth |
//! | Right-click a point | Delete it (Delete / Backspace deletes the selected one) |
//! | Drag the square beside the selected point | Thicker / thinner there |
//! | Wheel over a point | The same |
//! | S held, move sideways | Size the pointed-at point, else the tube's radius |
//! | Enter | Merge |
//! | Esc | Cancel the gesture, then deselect, then leave |
//!
//! T brings the tool up and puts it away. Grabbing a copy's point (mirrored
//! or turned) edits the tube through the mirror or the turn. A drag on empty background orbits, as
//! everywhere in the app; in the Unity tool it drew a tube in mid-air, which
//! is now Ctrl + drag.
//!
//! # The Muscle tool
//!
//! The same tool with another shape in hand, on its own button (B): [`muscle()`]. A muscle is thin at
//! both ends and full at its belly, wider than it is deep, and where its points are on the model it
//! lies on the model along its whole length, sunk in as far as Depth says (see `muscle`). Everything
//! above holds for it, with three differences. A stroke that starts just off the model's outline is
//! pulled onto the model. A rough stroke is taken as the clean line it meant: a straight line is two
//! points however the surface curves under it. And with Free placement a stroke started on the
//! model floats in front of it rather than growing out of it.
//!
//! The two tools keep their own shapes, settings and undo steps: a tube is the Tube tool's, a muscle
//! the Muscle tool's.
//!
//! The geometry is plain data and stands on its own: [`curve`] and [`spine`]
//! turn points into a centre line, [`field`] is the tube as a distance field
//! and its mesh, [`merge`] fuses that into a model.

pub mod curve;
mod demo;
pub mod field;
mod finish;
mod input;
mod live;
pub mod merge;
pub mod muscle;
mod panel;
pub mod spine;
mod tool;
pub mod types;

use glam::{Vec2, Vec3};
use sculpt_toolkit::{Arrival, Button, Ctx, Draw, Event, Info, Key, Loss, ObjectId, RadialChange, Saved, Tool};

pub use field::TubeField;
pub use live::{Symmetry, TubeState};
pub use tool::TubeTool;
pub use types::{CapStyle, ControlPoint, Kind, Sample, TubePoint, TubeSettings};

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn Tool> {
    Box::new(TubeTool::default())
}

/// The Muscle tool, ready to register with the app.
pub fn muscle() -> Box<dyn Tool> {
    Box::new(TubeTool::muscle())
}

impl Tool for TubeTool {
    fn info(&self) -> Info {
        if self.kind() == Kind::Muscle {
            return Info {
                id: "muscle",
                name: "Muscle",
                icon: "tool_muscle",
                tip: "Draw muscles on the model to block out a body: each lies on the surface, full at its belly and thin at \
                      its ends. Shape them by their points, then fuse them into the model (B).",
                hotkey: Some(Key::B),
            };
        }
        Info {
            id: "tube",
            name: "Tube",
            icon: "tool_tube",
            tip: "Draw tubes along curves for horns, limbs and tails, shape them by their points, then fuse them into the model (T).",
            hotkey: Some(Key::T),
        }
    }

    fn enter(&mut self, cx: &mut Ctx) {
        self.entered(cx);
    }

    fn leave(&mut self, cx: &mut Ctx) {
        self.left(cx);
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        self.now = cx.time;
        // With the panels hidden there are no handles to aim at: the view is
        // for looking.
        if cx.clean_view && matches!(event, Event::Press(_) | Event::Wheel(_)) {
            return false;
        }
        let taken = match event {
            Event::Press(Button::Left) => self.press(cx, cx.pointer, cx.mods.shift, cx.mods.ctrl),
            Event::Press(Button::Right) => self.right_press(cx, cx.pointer),
            Event::Drag => {
                self.drag_to(cx, cx.pointer);
                true
            }
            Event::Release => {
                self.release(cx);
                true
            }
            Event::Wheel(notches) => self.wheel(cx, cx.pointer, notches),
            Event::KeyDown(key) => self.key_down(cx, key),
            Event::KeyUp(key) => {
                self.key_up(key);
                false
            }
            // While a merge runs the tubes hold still, and Z is the scene's.
            Event::Undo => !self.is_merging() && self.can_undo() && self.undo(),
            Event::Redo => !self.is_merging() && self.can_redo() && self.redo(),
            _ => false,
        };
        // A step of the scene's history can put a model back the way it was, under the muscles on it.
        self.scene_stepped |= !taken && matches!(event, Event::Undo | Event::Redo);
        // Whatever changed, the handles are where the points now are.
        self.refresh(cx, false);
        self.flush(cx);
        taken
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        self.each_frame(cx, draw);
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        self.show_panel(ui, cx);
    }

    fn can_undo(&self, redo: bool) -> bool {
        if redo { TubeTool::can_redo(self) } else { TubeTool::can_undo(self) }
    }

    /// Live tubes are in no file. Every one was drawn by hand, so every one
    /// counts; see [`TubeTool::unmerged`] for which a new scene costs.
    fn scaffold_at_risk(&self, loss: Loss) -> Option<String> {
        let kind = self.kind();
        match self.unmerged(loss) {
            0 => None,
            1 => Some(format!("1 unmerged {} (Merge or Keep Separate first)", kind.one())),
            count => Some(format!("{count} unmerged {} (Merge or Keep Separate first)", kind.many())),
        }
    }

    /// A file keeps nothing of this tool's. What matters here is how the
    /// objects came: a scene put in place of this one takes the tubes drawn
    /// on its objects with it (`TubeTool::scene_replaced`).
    fn load_objects(&mut self, _objects: Vec<(ObjectId, Saved)>, arrival: Arrival) {
        if arrival == Arrival::Replaced {
            self.scene_replaced();
        }
    }

    /// A tube drawn out of the selected object and left mid-edit, one point
    /// selected and thickened so its radius handle shows. `mirror` draws it
    /// to one side with X mirroring on, for a pair; `radial` with six repeats
    /// about Y, for a ring of them, and `radial-mirror` with three and a
    /// mirror as well, for six; `surface` lays it on the model instead;
    /// `object` goes on to Keep Separate and `merged` to Merge (`radial-merged`
    /// the six repeats of `radial`).
    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        if self.kind() == Kind::Muscle {
            return self.demo_muscle(cx, what);
        }
        let scene = cx.scene();
        let bounds = scene.bounds(scene.active());
        if bounds.is_empty() {
            return;
        }
        let (centre, reach) = (bounds.center(), bounds.size().max_element() * 0.5);
        let view = cx.view;
        let at = |right: f32, up: f32| view.project(centre + (view.right * right + view.up * up) * reach);
        let turned = what.starts_with("radial");
        cx.set_mirror([what == "mirror" || what == "radial-mirror", false, false]);
        let repeats = if what == "radial-mirror" { 3 } else { 6 };
        cx.set_radial(RadialChange { on: Some(turned), count: Some(repeats), axis: Some(Vec3::Y) });
        self.set_on_surface(what == "surface");

        // A stroke from on the model out past its outline, curling over.
        let waypoints: &[(f32, f32)] = match what {
            "surface" => &[(-0.55, -0.35), (-0.2, 0.1), (0.15, 0.0), (0.5, 0.4)],
            "mirror" | "radial" | "radial-merged" => &[(0.45, 0.3), (0.85, 0.75), (1.25, 0.85), (1.5, 0.55)],
            // Up from the model between the mirror plane and the next repeat's, so that no tube lands on
            // another of the six.
            "radial-mirror" => &[(0.4, 0.25), (0.5, 0.8), (0.6, 1.15), (0.72, 1.3)],
            _ => &[(-0.3, 0.4), (-0.55, 0.95), (-1.0, 1.2), (-1.45, 0.95)],
        };
        let mut path: Vec<Vec2> = Vec::new();
        for pair in waypoints.windows(2) {
            for step in 0..12 {
                let t = step as f32 / 12.0;
                let (a, b) = (pair[0], pair[1]);
                path.extend(at(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            }
        }
        path.extend(waypoints.last().and_then(|&(right, up)| at(right, up)));
        if !self.draw_stroke(cx, &path, false) && !self.draw_stroke(cx, &path, true) {
            return;
        }

        match what {
            "object" => self.keep_separate(cx),
            "merged" | "radial-merged" => {
                self.merge_all(cx);
                self.wait_for_merge(cx);
            }
            _ => {
                let tube = self.tube_count() - 1;
                self.select_tube(tube, Some(self.point_count(tube) / 2));
                self.set_selected_point_size(1.7);
                self.commit_edit();
            }
        }
        self.refresh(cx, false);
        self.flush(cx);
    }
}

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
