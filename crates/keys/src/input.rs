//! The tool's pointer and keys: pick a spot on the cut face, add, drag, turn
//! and remove joints, flip the view between the halves.
//!
//! Picking is against the joint's PLANE, not the mesh: the cut face sits
//! between the two halves where no ray from outside can reach it, and the
//! plane is where the keys live anyway (see `KeyJoint`). A point counts as on
//! the face when the joint's map says a cut face covers it there.

use std::time::Duration;

use glam::Vec2;
use sculpt_toolkit::{Ctx, Key};

use crate::planner::{self, JointPlan, PlannedKey};
use crate::tool::Keys;

const ROTATE_STEP_DEGREES: f32 = 15.0;
const FINE_ROTATE_STEP_DEGREES: f32 = 5.0;

/// Which half is on screen while the joints are edited: the one with the
/// pegs (they stand on its face, visible), the one with the sockets, or both
/// (the face is then hidden between them, and only the outlines drawn over
/// the model show the joints).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewSide {
    #[default]
    Pegs,
    Holes,
    Both,
}

impl ViewSide {
    fn next(self) -> ViewSide {
        match self {
            ViewSide::Pegs => ViewSide::Holes,
            ViewSide::Holes => ViewSide::Both,
            ViewSide::Both => ViewSide::Pegs,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ViewSide::Pegs => "Pegs",
            ViewSide::Holes => "Holes",
            ViewSide::Both => "Both",
        }
    }
}

/// What is under the pointer: a point of a joint's plane, and the key there
/// if there is one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub joint: usize,
    /// In the joint's own plane.
    pub plane: Vec2,
    pub key: Option<usize>,
}

/// A key being carried by the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dragging {
    pub joint: usize,
    pub key: usize,
    /// From where the key was taken hold of to its middle.
    offset: Vec2,
    /// The drag is a step of the joints' history already: it began by adding
    /// its key, or it has moved it.
    stepped: bool,
}

impl Keys {
    /// Which half's placement a joint's keys are drawn and picked through:
    /// the one on screen.
    pub(crate) fn shown_side_is_a(&self, jp: &JointPlan) -> bool {
        self.view == ViewSide::Both || (self.view == ViewSide::Pegs) == jp.peg_on_a
    }

    /// Whether each half is on screen in the current view. Joints can put
    /// pegs on different halves; the largest joint decides what "the peg
    /// side" is.
    pub(crate) fn halves_shown(&self) -> (bool, bool) {
        let peg_on_a = self.pair.as_ref().and_then(|pair| pair.plan.main_joint()).is_none_or(|jp| jp.peg_on_a);
        let pegs = self.view == ViewSide::Pegs;
        (self.view == ViewSide::Both || pegs == peg_on_a, self.view == ViewSide::Both || pegs != peg_on_a)
    }

    // ---------------------------------------------------------------- picking

    /// The cut face under a pixel: the nearest joint plane the view ray
    /// crosses where a face covers it, and the key there if any.
    /// `only_joint` keeps a drag on its own joint, and lets it leave the
    /// face (the key turns red there).
    pub(crate) fn pick(&self, cx: &Ctx, px: Vec2, only_joint: Option<usize>) -> Option<Hit> {
        let pair = self.editable()?;
        let ray = cx.view.ray(px);
        let mut best: Option<(f32, Hit)> = None;
        for (j, jp) in pair.plan.joints.iter().enumerate() {
            if only_joint.is_some_and(|only| only != j) {
                continue;
            }
            // Into the world the joints were planned in, through whichever
            // half is on screen.
            let to_plan = pair.display(cx.scene(), self.shown_side_is_a(jp)).inverse();
            let origin = to_plan.transform_point3(ray.origin);
            let dir = to_plan.transform_vector3(ray.dir);
            let along = dir.dot(jp.joint.axis);
            if along.abs() < 1e-6 {
                continue;
            }
            let t = (jp.joint.origin - origin).dot(jp.joint.axis) / along;
            if t <= 0.0 || best.is_some_and(|(nearest, _)| t >= nearest) {
                continue;
            }
            let plane = jp.joint.to_plane(origin + dir * t);
            let key = jp.key_at(plane);
            if !jp.map.on_face(plane) && key.is_none() && only_joint.is_none() {
                continue;
            }
            best = Some((t, Hit { joint: j, plane, key }));
        }
        best.map(|(_, hit)| hit)
    }

    // ----------------------------------------------------------------- events

    pub(crate) fn press(&mut self, cx: &mut Ctx) -> bool {
        self.press_at(cx, cx.pointer)
    }

    /// The left button went down at `px`. On a key: take hold of it. On the
    /// face: add a key there and keep hold of it, so click-drag places it in
    /// one gesture. Anywhere else the press is not the tool's, and the
    /// camera orbits.
    pub(crate) fn press_at(&mut self, cx: &mut Ctx, px: Vec2) -> bool {
        let Some(hit) = self.pick(cx, px, None) else { return false };
        match hit.key {
            Some(key) => self.start_drag(hit.joint, key, hit.plane),
            None => self.add_key(hit.joint, hit.plane),
        }
        true
    }

    pub(crate) fn right_press(&mut self, cx: &mut Ctx) -> bool {
        match self.pick(cx, cx.pointer, None) {
            Some(Hit { joint, key: Some(key), .. }) => {
                self.remove_key(cx, joint, key);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn drag_to(&mut self, cx: &mut Ctx) {
        self.drag_at(cx, cx.pointer);
    }

    pub(crate) fn drag_at(&mut self, cx: &mut Ctx, px: Vec2) {
        let Some(drag) = self.drag else { return };
        if let Some(hit) = self.pick(cx, px, Some(drag.joint)) {
            self.move_dragged_key(hit.plane + drag.offset);
        }
    }

    /// A click on something that is not a joint or a cut face. With no pair
    /// yet, a click on another object offers it as the second piece.
    pub(crate) fn click(&mut self, cx: &mut Ctx) -> bool {
        if self.pair.is_some() || self.job.is_some() {
            return false;
        }
        match cx.pick(cx.pointer) {
            Some(hit) if hit.object != cx.scene().active() => self.pair_with(cx, hit.object),
            _ => false,
        }
    }

    /// The wheel over a key turns it (Shift for fine steps). Over anything
    /// else it is the camera's.
    pub(crate) fn wheel(&mut self, cx: &mut Ctx, notches: f32) -> bool {
        let Some(Hit { joint, key: Some(key), .. }) = self.pick(cx, cx.pointer, None) else { return false };
        let step = if cx.mods.shift { FINE_ROTATE_STEP_DEGREES } else { ROTATE_STEP_DEGREES };
        self.rotate_key(cx, joint, key, notches.signum() * step);
        true
    }

    pub(crate) fn key(&mut self, cx: &mut Ctx, key: Key) -> bool {
        let over = self.pick(cx, cx.pointer, None).and_then(|hit| Some((hit.joint, hit.key?)));
        match key {
            // Esc lets go of a key being carried, putting it back; then stops
            // a build; and only then is the app's, which puts the tool away
            // with the joints kept as they are.
            Key::Escape if self.drag.is_some() => {
                self.cancel_drag();
                true
            }
            Key::Escape => match &self.job {
                Some(job) => {
                    job.cancel();
                    true
                }
                None => false,
            },
            Key::Enter => {
                self.end_drag();
                self.apply(cx, Duration::ZERO);
                true
            }
            Key::V => {
                self.set_view(cx, self.view.next());
                true
            }
            Key::Delete | Key::Backspace => match over {
                Some((joint, key)) if self.drag.is_none() => {
                    self.remove_key(cx, joint, key);
                    true
                }
                _ => false,
            },
            Key::R => match over {
                Some((joint, key)) => {
                    self.rotate_key(cx, joint, key, 90.0);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    pub(crate) fn set_view(&mut self, cx: &mut Ctx, view: ViewSide) {
        self.view = view;
        self.pegs_stale = true;
        cx.toast(match view {
            ViewSide::Pegs => "Peg Side",
            ViewSide::Holes => "Socket Side",
            ViewSide::Both => "Both Halves",
        });
    }

    // ------------------------------------------------------------- operations

    fn joint(&self, joint: usize) -> Option<&JointPlan> {
        self.editable()?.plan.joints.get(joint)
    }

    fn joint_mut(&mut self, joint: usize) -> Option<&mut JointPlan> {
        self.editable_mut()?.plan.joints.get_mut(joint)
    }

    fn add_key(&mut self, joint: usize, plane: Vec2) {
        let settings = self.settings;
        if self.joint_mut(joint).is_none_or(|jp| jp.profile.is_none()) {
            return;
        }
        self.push_edit();
        let Some(jp) = self.joint_mut(joint) else { return };
        jp.keys.push(PlannedKey::new(plane, settings.rotation_degrees));
        // Every key of the joint: whether one crowds its neighbour is decided
        // by both.
        planner::validate_all(jp, &settings);
        let index = jp.keys.len() - 1;
        self.pegs_stale = true;
        self.drag = Some(Dragging { joint, key: index, offset: Vec2::ZERO, stepped: true });
    }

    pub(crate) fn remove_key(&mut self, cx: &mut Ctx, joint: usize, index: usize) {
        let settings = self.settings;
        if self.joint_mut(joint).is_none_or(|jp| index >= jp.keys.len()) {
            return;
        }
        self.push_edit();
        let Some(jp) = self.joint_mut(joint) else { return };
        jp.keys.remove(index);
        planner::validate_all(jp, &settings);
        self.pegs_stale = true;
        self.end_drag();
        self.say(cx, format!("Joint removed. {} left.", self.key_count()), true);
    }

    fn rotate_key(&mut self, cx: &mut Ctx, joint: usize, index: usize, degrees: f32) {
        let settings = self.settings;
        // A round key looks the same however it is turned.
        let turns = |jp: &mut JointPlan| {
            index < jp.keys.len() && jp.profile.as_ref().is_some_and(|p| !p.is_rotationally_symmetric())
        };
        if !self.joint_mut(joint).is_some_and(turns) {
            return;
        }
        self.push_edit();
        let Some(jp) = self.joint_mut(joint) else { return };
        jp.keys[index].angle = (jp.keys[index].angle + degrees).rem_euclid(360.0);
        planner::validate_all(jp, &settings);
        self.pegs_stale = true;
        self.report_keys(cx);
    }

    fn start_drag(&mut self, joint: usize, index: usize, grabbed_at: Vec2) {
        let Some(position) = self.joint(joint).and_then(|jp| Some(jp.keys.get(index)?.position)) else { return };
        // Kept aside rather than pushed: a click on a key that does not move
        // it is not an edit worth an undo step.
        self.held = self.pair.as_ref().map(|pair| pair.snapshot());
        self.drag = Some(Dragging { joint, key: index, offset: position - grabbed_at, stepped: false });
    }

    fn move_dragged_key(&mut self, position: Vec2) {
        let settings = self.settings;
        let Some(drag) = self.drag else { return };
        let here = self.joint(drag.joint).and_then(|jp| Some(jp.keys.get(drag.key)?.position));
        if !here.is_some_and(|here| here.distance_squared(position) >= 1e-12) {
            return;
        }
        if !drag.stepped {
            if let Some(before) = self.held.take() {
                self.push_snapshot(before);
            }
            self.drag = Some(Dragging { stepped: true, ..drag });
        }
        let Some(jp) = self.joint_mut(drag.joint) else { return };
        jp.keys[drag.key].position = position;
        // The other keys too: crowding is mutual.
        planner::validate_all(jp, &settings);
        self.pegs_stale = true;
    }

    /// The button came up: the key stays where it is. Says whether a key was
    /// being carried.
    pub(crate) fn end_drag(&mut self) -> bool {
        self.held = None;
        self.drag.take().is_some()
    }

    /// Lets go of the key being carried and puts things back as they were
    /// before it was taken hold of (or added).
    fn cancel_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        self.held = None;
        if !drag.stepped {
            return;
        }
        let settings = self.settings;
        if let (Some(snapshot), Some(pair)) = (self.pop_edit(), self.pair.as_mut()) {
            pair.restore(snapshot, &settings);
        }
        self.pegs_stale = true;
    }

    /// Says how the joints stand after an edit: how many, or why the one
    /// just touched will not be built.
    pub(crate) fn report_keys(&mut self, cx: &mut Ctx) {
        let Some(pair) = self.editable() else { return };
        let unfit = pair.plan.joints.iter().flat_map(|jp| &jp.keys).find(|key| !key.valid);
        match unfit.and_then(|key| key.problem.clone()) {
            Some(problem) => self.say(cx, format!("A joint here will not be built: {problem}."), false),
            None => {
                let count = self.key_count();
                let s = if count == 1 { "" } else { "s" };
                self.say(cx, format!("{count} joint{s}. Enter builds them into both pieces."), true);
            }
        }
    }
}
