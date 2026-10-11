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
//! The geometry is plain data and stands on its own: [`curve`] and [`spine`]
//! turn points into a centre line, [`field`] is the tube as a distance field
//! and its mesh, [`merge`] fuses that into a model.

pub mod curve;
pub mod field;
mod finish;
mod input;
mod live;
pub mod merge;
mod panel;
pub mod spine;
mod tool;
pub mod types;

use glam::{Vec2, Vec3};
use sculpt_toolkit::{Button, Ctx, Draw, Event, Info, Key, Loss, RadialChange, Tool};

pub use field::TubeField;
pub use live::{Symmetry, TubeState};
pub use tool::TubeTool;
pub use types::{CapStyle, ControlPoint, Sample, TubePoint, TubeSettings};

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn Tool> {
    Box::new(TubeTool::default())
}

impl Tool for TubeTool {
    fn info(&self) -> Info {
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
        match self.unmerged(loss) {
            0 => None,
            1 => Some("1 unmerged tube (Merge or Keep Separate first)".to_owned()),
            count => Some(format!("{count} unmerged tubes (Merge or Keep Separate first)")),
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
