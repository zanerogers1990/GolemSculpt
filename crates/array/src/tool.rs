//! The Array tool: the object's copies laid out along a line, round a circle or
//! along a curve you draw, live, until they are made real.
//!
//! The geometry is the rest of this crate. This is the part that talks to the
//! app: the pointer and keys while the tool is up, the panel, the copies it
//! asks to have drawn every frame (up or not: they stay on screen when the
//! user goes back to sculpting, and follow every stroke because they are the
//! object's own mesh drawn again), and the commit that makes them real.
//!
//! While the tool is up:
//!
//! | Input | Action |
//! | --- | --- |
//! | Line, Circle | Make an array of the selected object in one click |
//! | Draw Curve, then drag | The copies follow the curve you draw; end where you began to close it |
//! | Drag a point | Move it, on the plane facing you |
//! | Drag the square start point | Move the object, and its array with it |
//! | Click the path | Add a point there, and keep dragging it |
//! | Shift + drag the last point | Extend the path from there |
//! | Right-click a point, or Delete | Delete it |
//! | Click another object | Switch to it |
//! | Esc | Cancel the gesture, then stop drawing, then deselect, then leave |
//! | Z, Shift + Z | Step the array's own history |
//!
//! Y brings the tool up and puts it away. A drag on empty background orbits.
//! Arrays are the tool's, not the scene's: they are not saved in `.sclp` files
//! yet, and they have their own Z while the tool is up (the scene's takes over
//! when the tool's history is empty, or the tool is away).

use glam::{Mat4, Vec3};
use sculpt_toolkit::{Button, Ctx, Draw, Event, Info, Key, ObjectId, Placement, Tool};

use crate::arrays::{Arrays, History};
use crate::finish::{Building, Retired};
use crate::gestures::{Gesture, Hover, Note};
use crate::picture::{self, Marks, Picture};
use crate::state::ArrayState;
use crate::types::Pose;

/// How long a message stays in the panel before the hint comes back.
const NOTE_SECONDS: f64 = 6.0;

#[derive(Default)]
pub struct ArrayTool {
    pub(crate) arrays: Arrays,
    pub(crate) history: History,
    /// Arrays that were made real, kept so Z can hand them back.
    pub(crate) retired: Vec<Retired>,
    pub(crate) gesture: Gesture,
    /// While the square start point is dragged: the move so far, shown but
    /// not committed to the scene until the drag ends.
    pub(crate) shift: Option<(ObjectId, Vec3)>,
    /// The handle picked in the viewport.
    pub(crate) picked: Option<usize>,
    pub(crate) hover: Hover,
    /// The next drag in the viewport draws a curve.
    pub(crate) armed: bool,
    pub(crate) last_target: Option<ObjectId>,
    /// Making the copies real, on a background job.
    pub(crate) building: Option<Building>,
    pub(crate) note: Note,
}

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn Tool> {
    Box::new(ArrayTool::default())
}

impl ArrayTool {
    /// The array of an object, or the empty state.
    pub fn state(&self, object: ObjectId) -> ArrayState {
        self.arrays.state(object)
    }

    pub fn has_array(&self, object: ObjectId) -> bool {
        self.arrays.has(object)
    }

    /// Whether the copies are being made real in the background.
    pub fn is_building(&self) -> bool {
        self.building.is_some()
    }

    /// The handle that is picked, if any.
    pub fn picked(&self) -> Option<usize> {
        self.picked
    }

    /// Whether the next drag draws a curve.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Whether a drag or a stroke is in progress.
    pub fn is_dragging(&self) -> bool {
        !matches!(self.gesture, Gesture::None)
    }

    /// The selected object: the one whose array the tool edits.
    pub(crate) fn target(&self, cx: &Ctx) -> ObjectId {
        cx.scene().active()
    }

    /// Says what happened: in the panel for a few seconds, and in the status
    /// line of the Scene panel.
    pub(crate) fn say(&mut self, cx: &mut Ctx, text: impl Into<String>, ok: bool) {
        let text = text.into();
        self.note = Note { text: text.clone(), ok, until: cx.time + NOTE_SECONDS };
        cx.status(text, ok);
    }

    /// The line the panel shows when there is no message.
    pub(crate) fn hint(&self, cx: &Ctx) -> &'static str {
        let target = self.target(cx);
        if self.armed {
            "Drag in the viewport to draw the curve. End where you started to close it into a loop."
        } else if self.building.is_some() {
            "Making the copies real. Cancel it from the jobs list if it is taking too long."
        } else if !self.arrays.has(target) {
            "Pick Line, Circle or Draw Curve to repeat the selected object."
        } else {
            "Drag the points to reshape the array. Click the path to add a point."
        }
    }

    /// Notices that another object was selected: the picked handle, a drag
    /// and a half-made edit belonged to the one before.
    pub(crate) fn sync_target(&mut self, cx: &mut Ctx) {
        let target = self.target(cx);
        if self.last_target == Some(target) {
            return;
        }
        self.cancel_gesture();
        self.commit_edit();
        self.picked = None;
        self.armed = false;
        self.hover = Hover::default();
        self.last_target = Some(target);
    }

    /// What this frame draws of every live array: the copies, as the object's
    /// own mesh drawn again. Whether the tool is up or not, so they stay on
    /// screen once it is put away.
    fn draw_copies(&self, cx: &Ctx, draw: &mut Draw) {
        let scene = cx.scene();
        for (object, live) in self.arrays.iter() {
            let Some(info) = scene.info(object) else { continue };
            if !info.visible {
                continue;
            }
            let mut pose = pose_of(info.placement);
            // The square start point being dragged: the object is drawn where
            // the drag has it (as copy 0), the real one left out.
            let first = match self.shift.filter(|(moved, _)| *moved == object) {
                Some((_, by)) => {
                    pose.position += by;
                    draw.hide(object);
                    0
                }
                None => 1,
            };
            let matrices: Vec<Mat4> =
                live.layout[first.min(live.layout.len())..].iter().map(|p| p.world_matrix(&pose)).collect();
            draw.copies(object, &matrices);
        }
    }

    /// The path and handles of the selected object's array, and a stroke being
    /// drawn, over the viewport.
    fn draw_overlay(&self, cx: &Ctx, draw: &mut Draw) {
        let object = self.target(cx);
        if let (Some(live), Some(info)) = (self.arrays.get(object), cx.scene().info(object)) {
            let mut origin = info.placement.position;
            if let Some((_, by)) = self.shift.filter(|(moved, _)| *moved == object) {
                origin += by;
            }
            let dragging = self.is_dragging();
            let marks = Marks {
                picked: self.picked,
                hovered: self.hover.point.filter(|_| !dragging),
                insert: self.hover.curve.filter(|_| !dragging).map(|hit| hit.world),
            };
            let size = crate::make::object_size(cx.scene().bounds(object).size());
            picture::draw(cx, draw, &live.state, origin, &Picture::new(&live.state, origin), &marks, size);
        }
        picture::draw_stroke(cx, draw, self.stroke_world());
    }

    /// Where the handles and the path under the pointer are, for the hover
    /// highlight and the click that adds a point.
    fn refresh_hover(&mut self, cx: &Ctx) {
        self.hover = Hover::default();
        if !cx.active || !cx.hovering || self.is_dragging() || self.armed || self.building.is_some() {
            return;
        }
        let object = self.target(cx);
        let (Some(live), Some(info)) = (self.arrays.get(object), cx.scene().info(object)) else { return };
        let origin = info.placement.position;
        self.hover.point = picture::pick_handle(&cx.view, &live.state, origin, cx.pointer, self.picked);
        if self.hover.point.is_none() {
            let picture = Picture::new(&live.state, origin);
            self.hover.curve = picture::pick_curve(&cx.view, &live.state, &picture, cx.pointer);
        }
    }
}

/// An object's placement as the geometry of this crate wants it.
pub(crate) fn pose_of(placement: Placement) -> Pose {
    Pose::new(placement.position, placement.rotation, placement.scale)
}

impl Tool for ArrayTool {
    fn info(&self) -> Info {
        Info {
            id: "array",
            name: "Array",
            icon: "tool_array",
            tip: "Repeat the selected object along a line, round a circle or along a curve you draw, live, then make \
                  the copies real (Y).",
            hotkey: Some(Key::Y),
        }
    }

    fn enter(&mut self, cx: &mut Ctx) {
        self.sync_target(cx);
        let hint = self.hint(cx);
        cx.status(hint, true);
    }

    fn leave(&mut self, _cx: &mut Ctx) {
        self.cancel_gesture();
        self.commit_edit();
        self.armed = false;
        self.picked = None;
        self.hover = Hover::default();
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::Press(Button::Left) => self.press(cx, cx.pointer),
            Event::Press(Button::Right) => self.right_press(cx, cx.pointer),
            Event::Drag => {
                self.drag_to(cx, cx.pointer);
                true
            }
            Event::Release => {
                self.release(cx);
                true
            }
            Event::Click(Button::Left) => self.click(cx, cx.pointer),
            Event::KeyDown(Key::Escape) => self.escape(cx),
            Event::KeyDown(Key::Delete | Key::Backspace) => self.delete_key(cx),
            Event::Undo => self.undo(cx),
            Event::Redo => self.redo(cx),
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        // A job started here keeps running if the user goes back to
        // sculpting, so its result is collected whether or not the tool is up.
        self.land_finished(cx);
        self.notice_undone_applies(cx);
        self.sync_target(cx);
        self.close_edit_when_due(cx);
        self.refresh_hover(cx);
        self.draw_copies(cx, draw);
        if cx.active && !cx.clean_view {
            self.draw_overlay(cx, draw);
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        crate::panel::show(self, ui, cx);
    }

    fn can_undo(&self, redo: bool) -> bool {
        if redo { !self.history.redo.is_empty() } else { !self.history.undo.is_empty() || self.edit_changed() }
    }

    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        crate::demo::demo(self, cx, what);
    }
}
