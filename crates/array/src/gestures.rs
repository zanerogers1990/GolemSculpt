//! What the pointer and the keys do while the tool is up: picking and dragging
//! the handles of the path, adding and removing points, drawing a curve.
//! Ported from `ArrayController.Input.cs`.
//!
//! Ground rules shared with the Unity tools: a drag is worked out from the
//! press, not summed up (so nothing creeps and Esc puts it back exactly), and
//! a handle that was grabbed stays under the cursor. Dragging a point moves it
//! on the plane facing the camera through where it was.
//!
//! Dragging the square start point is different from the Unity tool: there it
//! moved the object and left the rest of the path where it was. The toolkit
//! has no way to move an object that is not a scene commit, and a commit per
//! pointer sample (or one the tool's own Z could not take back in order) is
//! not an option, so the start point moves the object and the whole array
//! with it, shown while it is dragged and committed as one scene step when it
//! is let go.

use glam::{Vec2, Vec3};
use sculpt_toolkit::{Command, Ctx, ObjectId};

use crate::make;
use crate::picture::{self, CurveHit, Picture};
use crate::tool::ArrayTool;

/// How far (interface points) the pointer must move before a press on a
/// handle is a drag.
const DRAG_THRESHOLD: f32 = 3.0;
/// A stroke takes a sample each time the pointer has moved this far.
const STROKE_SPACING: f32 = 3.0;
/// A stroke shorter than this is not a curve.
const MIN_STROKE: f32 = 24.0;
/// A stroke ending this close to where it began, after a long one, is a loop.
const CLOSE_LOOP: f32 = 26.0;

/// A handle being dragged.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    pub object: ObjectId,
    pub index: usize,
    pub press: Vec2,
    /// From the pointer to the handle's pixel at the press, so the handle
    /// stays under the pointer rather than jumping to it.
    pub grab: Vec2,
    /// The point the camera-facing plane the drag slides on goes through.
    pub plane_point: Vec3,
    pub moved: bool,
}

/// A curve being drawn.
#[derive(Clone, Debug)]
pub struct Stroke {
    pub object: ObjectId,
    /// The plane facing the camera through the object's position, so the path
    /// is drawn where the object is rather than at some depth of its own.
    pub plane_point: Vec3,
    pub world: Vec<Vec3>,
    pub start: Vec2,
    pub last: Vec2,
    /// Pixels of pen travel so far.
    pub pixels: f32,
}

#[derive(Default)]
pub enum Gesture {
    #[default]
    None,
    Drag(Drag),
    Draw(Stroke),
}

/// What is under the pointer while it only hovers.
#[derive(Default, Clone, Copy)]
pub struct Hover {
    pub point: Option<usize>,
    pub curve: Option<CurveHit>,
}

/// A message for the panel, and when it stops being news.
#[derive(Default)]
pub struct Note {
    pub text: String,
    pub ok: bool,
    pub until: f64,
}

impl ArrayTool {
    /// The stroke being drawn, in world space.
    pub(crate) fn stroke_world(&self) -> &[Vec3] {
        match &self.gesture {
            Gesture::Draw(stroke) => &stroke.world,
            _ => &[],
        }
    }

    /// The position of an object, and its box's rough size.
    fn subject(&self, cx: &Ctx, object: ObjectId) -> Option<(Vec3, Vec3)> {
        let scene = cx.scene();
        Some((scene.info(object)?.placement.position, scene.bounds(object).size()))
    }

    // ---- press, drag, release ----------------------------------------------

    /// A left press at `px`. Takes it when it grabs a handle, adds a point to
    /// the path, or starts a stroke; otherwise passes it on, so the camera
    /// orbits.
    pub(crate) fn press(&mut self, cx: &mut Ctx, px: Vec2) -> bool {
        if self.building.is_some() || self.is_dragging() {
            return false;
        }
        let object = self.target(cx);
        if self.armed {
            return self.begin_draw(cx, object, px);
        }
        let Some((origin, _)) = self.subject(cx, object) else { return false };
        let state = self.arrays.state(object);
        if state.is_none() {
            return false;
        }

        if let Some(mut index) = picture::pick_handle(&cx.view, &state, origin, px, self.picked) {
            self.picked = Some(index);
            let last = state.points.len() - 1;
            let extend =
                cx.mods.shift && state.settings.is_path() && !state.settings.closed && index == last && index > 0;
            let label = if index == 0 { "Move Arrayed Object" } else { "Move Array Point" };
            if extend {
                index = self.extend_from(object, last);
                self.begin_drag(cx, object, index, px, "Extend Array Path");
            } else {
                self.begin_drag(cx, object, index, px, label);
            }
            return true;
        }

        let picture = Picture::new(&state, origin);
        if let Some(hit) = picture::pick_curve(&cx.view, &state, &picture, px) {
            self.insert_point(cx, object, hit, px);
            return true;
        }
        false
    }

    /// The pointer moved with the button down.
    pub(crate) fn drag_to(&mut self, cx: &Ctx, px: Vec2) {
        match &mut self.gesture {
            Gesture::Drag(drag) => {
                if !drag.moved && px.distance(drag.press) < DRAG_THRESHOLD * cx.view.pixels_per_point {
                    return;
                }
                drag.moved = true;
                let drag = *drag;
                self.move_handle(cx, drag, px);
            }
            Gesture::Draw(stroke) => {
                let moved = px.distance(stroke.last);
                if moved < STROKE_SPACING * cx.view.pixels_per_point {
                    return;
                }
                stroke.pixels += moved;
                stroke.last = px;
                add_sample(cx, stroke, px);
            }
            Gesture::None => {}
        }
    }

    /// The button came up: the end of a drag or a stroke.
    pub(crate) fn release(&mut self, cx: &mut Ctx) {
        match std::mem::take(&mut self.gesture) {
            Gesture::None => {}
            Gesture::Drag(drag) if drag.index == 0 => self.end_object_move(cx, drag),
            Gesture::Drag(_) => self.commit_edit(),
            Gesture::Draw(stroke) => self.finish_stroke(cx, stroke),
        }
    }

    /// Esc, or anything that interrupts a gesture: puts back what it changed.
    /// False if there was none.
    pub(crate) fn cancel_gesture(&mut self) -> bool {
        match std::mem::take(&mut self.gesture) {
            Gesture::None => false,
            Gesture::Drag(drag) => {
                if drag.index == 0 {
                    self.shift = None;
                } else {
                    self.cancel_edit();
                }
                true
            }
            Gesture::Draw(_) => true,
        }
    }

    // ---- dragging a handle -------------------------------------------------

    /// Starts a drag of handle `index`. Anything but the start point is an
    /// edit of the path; the start point moves the object instead.
    pub(crate) fn begin_drag(&mut self, cx: &Ctx, object: ObjectId, index: usize, px: Vec2, label: &'static str) {
        let Some((origin, _)) = self.subject(cx, object) else { return };
        let state = self.arrays.state(object);
        if index >= state.points.len() {
            return;
        }
        if index > 0 {
            self.begin_edit(object, label, None);
        }
        let plane_point = origin + state.points[index];
        let grab = cx.view.project(plane_point).map_or(Vec2::ZERO, |screen| screen - px);
        self.gesture = Gesture::Drag(Drag { object, index, press: px, grab, plane_point, moved: false });
    }

    fn move_handle(&mut self, cx: &Ctx, drag: Drag, px: Vec2) {
        let Some(world) = cx.view.on_facing_plane(px + drag.grab, drag.plane_point) else { return };
        if drag.index == 0 {
            self.shift = Some((drag.object, world - drag.plane_point));
            return;
        }
        let Some((origin, _)) = self.subject(cx, drag.object) else { return };
        let mut state = self.arrays.state(drag.object);
        state.set_point_world(drag.index, world, origin);
        self.arrays.set(drag.object, state);
    }

    /// The start point was let go: the object moves where the drag took it, as
    /// one step of the scene's history.
    fn end_object_move(&mut self, cx: &mut Ctx, drag: Drag) {
        let Some((object, by)) = self.shift.take() else { return };
        if by == Vec3::ZERO {
            return;
        }
        let Some(info) = cx.scene().info(object) else { return };
        let mut placement = info.placement;
        placement.position += by;
        cx.commit("Move Arrayed Object", vec![Command::Place { object, placement }]);
        self.picked = Some(drag.index);
        // It is a step of the scene's history, not the tool's: say so, since Z
        // steps the array's own edits first.
        let text = "Moved the object and its array. That is a step of the scene's history: Z undoes the array's own \
                    edits first, then this.";
        self.say(cx, text, true);
    }

    /// Adds a point on top of the path's last one (a line becomes a curve) and
    /// returns its index, for the drag that then pulls it out.
    fn extend_from(&mut self, object: ObjectId, last: usize) -> usize {
        self.begin_edit(object, "Extend Array Path", None);
        let mut state = self.arrays.state(object);
        let point = state.points[last];
        state.points.push(point);
        state.settings.kind = crate::types::ArrayKind::Curve;
        let index = state.points.len() - 1;
        self.arrays.set(object, state);
        self.picked = Some(index);
        index
    }

    /// A new point on the path where it was clicked: its position the path's
    /// own at that spot (so nothing moves), then dragged like any other.
    fn insert_point(&mut self, cx: &Ctx, object: ObjectId, hit: CurveHit, px: Vec2) {
        self.begin_edit(object, "Add Array Point", None);
        let mut state = self.arrays.state(object);
        let index = state.insert_point(hit.segment, hit.t);
        self.arrays.set(object, state);
        self.picked = Some(index);
        self.hover = Default::default();
        self.begin_drag(cx, object, index, px, "Add Array Point");
    }

    // ---- removing points ---------------------------------------------------

    /// A right press: deletes the point under it.
    pub(crate) fn right_press(&mut self, cx: &mut Ctx, px: Vec2) -> bool {
        if self.building.is_some() || self.is_dragging() || self.armed {
            return false;
        }
        let object = self.target(cx);
        let Some((origin, _)) = self.subject(cx, object) else { return false };
        let state = self.arrays.state(object);
        match picture::pick_handle(&cx.view, &state, origin, px, self.picked) {
            Some(index) => {
                self.delete_point(cx, index);
                true
            }
            None => false,
        }
    }

    /// Removes a point from the path, or says why it cannot be.
    pub(crate) fn delete_point(&mut self, cx: &mut Ctx, index: usize) {
        let object = self.target(cx);
        let state = self.arrays.state(object);
        if state.is_none() {
            return;
        }
        if index == 0 {
            self.say(cx, "That point is the object itself. Use Remove Array to take the array off it.", false);
        } else if !state.settings.is_path() {
            self.say(cx, "A circle needs its centre. Drag it instead.", false);
        } else if state.points.len() <= 2 {
            self.say(cx, "The path needs at least an end point. Drag it, or Remove Array.", false);
        } else if index < state.points.len() {
            self.edit(cx, "Delete Array Point", false, |s| {
                s.points.remove(index);
            });
            self.picked = None;
            self.hover = Hover::default();
        }
    }

    /// Delete or Backspace: removes the picked point.
    pub(crate) fn delete_key(&mut self, cx: &mut Ctx) -> bool {
        if self.is_dragging() {
            return false;
        }
        match self.picked {
            Some(index) => self.delete_point(cx, index),
            None => self.say(
                cx,
                "Click a point to select it, then Delete removes it. Remove Array takes the whole array off.",
                true,
            ),
        }
        true
    }

    // ---- drawing a curve ---------------------------------------------------

    /// Arms (or disarms) drawing: the next drag in the viewport lays a new
    /// curve for the copies, replacing whatever path they had.
    pub(crate) fn set_armed(&mut self, cx: &mut Ctx, armed: bool) {
        self.armed = armed;
        if armed {
            self.picked = None;
            self.hover = Hover::default();
            self.say(
                cx,
                "Drag in the viewport to draw the curve. End where you started to close it into a loop.",
                true,
            );
        }
    }

    /// Starts a stroke at `px`.
    pub(crate) fn begin_draw(&mut self, cx: &Ctx, object: ObjectId, px: Vec2) -> bool {
        let Some((origin, _)) = self.subject(cx, object) else { return false };
        let mut stroke = Stroke { object, plane_point: origin, world: Vec::new(), start: px, last: px, pixels: 0.0 };
        add_sample(cx, &mut stroke, px);
        self.picked = None;
        self.gesture = Gesture::Draw(stroke);
        true
    }

    /// Draws the curve along screen positions exactly as a pen stroke would,
    /// for the demo and for scripts. True if the selected object ended up with
    /// a curve array.
    pub fn draw_stroke(&mut self, cx: &mut Ctx, points: &[Vec2]) -> bool {
        let Some((&first, rest)) = points.split_first() else { return false };
        self.cancel_gesture();
        let object = self.target(cx);
        if !self.begin_draw(cx, object, first) {
            return false;
        }
        for &px in rest {
            self.drag_to(cx, px);
        }
        self.release(cx);
        self.arrays.state(object).settings.kind == crate::types::ArrayKind::Curve
    }

    /// The stroke becomes the array's path: thinned to a handful of points
    /// (few enough to reshape by hand, and the curve through them follows the
    /// stroke), moved to start on the object, and closed into a loop if it
    /// ended where it began.
    fn finish_stroke(&mut self, cx: &mut Ctx, stroke: Stroke) {
        let ppp = cx.view.pixels_per_point;
        if stroke.world.len() < 3 || stroke.pixels < MIN_STROKE * ppp {
            self.say(cx, "Drag further to draw the curve.", false);
            return;
        }
        let closed = stroke.pixels > CLOSE_LOOP * 4.0 * ppp && stroke.last.distance(stroke.start) < CLOSE_LOOP * ppp;
        let object = stroke.object;
        let existing = self.arrays.state(object);
        let size = cx.scene().bounds(object).size();
        let Some(state) = make::curve_from_stroke(&existing, &stroke.world, closed, size) else {
            self.say(cx, "Drag further to draw the curve.", false);
            return;
        };

        let (count, closed, points) = (state.settings.count, state.settings.closed, state.points.len());
        self.begin_edit(object, "Draw Array Curve", None);
        self.arrays.set(object, state);
        self.commit_edit();
        self.armed = false;
        let what = if closed { "closed loop" } else { "curve" };
        self.say(cx, format!("{count} copies along a {what} of {points} points. Drag the points to reshape it."), true);
    }

    // ---- the rest of the input ---------------------------------------------

    /// A click on something that is not a handle (those press): another object
    /// is switched to, empty space lets go of the picked handle.
    pub(crate) fn click(&mut self, cx: &mut Ctx, px: Vec2) -> bool {
        if self.building.is_some() {
            return false;
        }
        let target = self.target(cx);
        if let Some(hit) = cx.pick(px).filter(|hit| hit.object != target) {
            let name = cx.scene().info(hit.object).map(|info| info.name.to_owned()).unwrap_or_default();
            cx.commit("Select", vec![Command::Select { object: hit.object }]);
            self.say(cx, format!("Now repeating {name}."), true);
            return true;
        }
        self.picked.take().is_some()
    }

    /// Esc: cancels the gesture, then stops drawing, then lets go of the
    /// picked point. False when there is nothing of that, so the app puts the
    /// tool away.
    pub(crate) fn escape(&mut self, cx: &mut Ctx) -> bool {
        if self.cancel_gesture() {
            self.say(cx, "Cancelled.", true);
            return true;
        }
        if self.armed {
            self.armed = false;
            self.say(cx, "Drawing cancelled.", true);
            return true;
        }
        self.picked.take().is_some()
    }
}

/// Adds the point of the plane under pixel `px` to a stroke.
fn add_sample(cx: &Ctx, stroke: &mut Stroke, px: Vec2) {
    if let Some(world) = cx.view.on_facing_plane(px, stroke.plane_point) {
        stroke.world.push(world);
    }
}
