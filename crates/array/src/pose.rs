//! Turning and moving every copy at once with the gizmo, putting the copies back
//! on the path, and mirroring the array: what changes the object in the scene
//! and the array in the tool together. Ported from `ArrayController.cs`
//! ("posing copies", "mirror") and `ArrayController.Input.cs` (picking a copy).
//!
//! **Posing.** Click a copy and the gizmo's arrows and rings sit on it; posing
//! that one copy poses them all, and nothing new is stored for it:
//!
//! - a copy's facing is the object's own rotation seen through the copy's place
//!   on the path, so turning a copy is turning the object (the copies turn in
//!   place, each about its own middle);
//! - how far the copies sit off the path is how far the object sits off the
//!   path's start (`points[0]`), so moving a copy moves the object off the path
//!   and leaves the path where it is in the world: the object goes by `local`
//!   and every point by `-local`, and each other copy goes the same way in its
//!   own frame (turned with the path, scaled with its size).
//!
//! The object's half of that is a `Place` in the scene; the array's half, the
//! points, is the tool's. A drag shows both live (the object is drawn where the
//! drag has it, the scene's own left out) and is committed at its end as one
//! step of the scene's history, with a step in the tool's own that says so (see
//! [`Scened`]): Z then takes the object back through the scene, in order with
//! the tool's other edits, and the tool puts the points back to match.
//!
//! **Copies Back onto the Path** is the same pair the other way: the object goes
//! back to the path's start, every point comes with it, and nothing but where
//! the copies sit changes.
//!
//! **Mirror.** The array is mirrored by the object's own live mirror, the one the
//! Mirror section of the Objects card edits; the three buttons are that switch
//! within reach, a step of the scene's history, and `copies.rs` draws the
//! array through it.

use glam::{Quat, Vec2, Vec3};
use sculpt_toolkit::{
    Command, Ctx, GizmoPhase, GizmoTarget, MirrorRepeater, ObjectId, ObjectInfo, Placement,
};

use crate::arrays::{Kind, Scened, Step};
use crate::copies::{self, Place};
use crate::make;
use crate::state::ArrayState;
use crate::tool::{ArrayTool, pose_of};
use crate::types::ArrayPlacement;

/// A move shorter than this (world units, per unit of distance from the origin) is the
/// gizmo's rounding, not a move.
const MOVE_EPSILON: f32 = 1e-6;
/// A turn smaller than this (radians) is no turn.
const TURN_EPSILON: f32 = 1e-5;
/// How much nearer (as a fraction of the distance) a copy of the array has to be than another
/// object under the pointer to be the one clicked.
const TIE: f32 = 1e-3;

/// The angle between two rotations, in radians, worked out from the vector part of their
/// difference: `Quat::angle_between` goes through `acos` of a number that rounds to 1 for two
/// rotations that are the same, and so reports 5e-4 for nothing.
fn turn_between(a: Quat, b: Quat) -> f32 {
    let d = a * b.inverse();
    2.0 * d.xyz().length().atan2(d.w.abs())
}

/// A copy picked in the viewport, the one the gizmo sits on: which copy of the
/// array (0 is the object itself) and which side of the object's live mirror it
/// is seen through (0 for the copy itself, else the mirror copy's flip mask).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyPick {
    pub object: ObjectId,
    pub copy: usize,
    pub side: u8,
}

/// What a click on the model landed on.
pub enum Clicked {
    /// A copy of the selected object's array (or the object itself).
    Copy(CopyPick),
    /// Some other object, which is switched to.
    Object(ObjectId),
    Nothing,
}

/// A gizmo drag of a copy, as it stands: what the object and the array were
/// when it began, and where the drag has put the object.
pub(crate) struct Posing {
    pick: CopyPick,
    /// The object, and the array's points, at the press.
    placement: Placement,
    state: ArrayState,
    /// The copy's place on the path: its turn and size are the same however far
    /// the whole array is carried, so they are taken once.
    frame: ArrayPlacement,
    /// Where the copy is at the press, and how it faces, on the straight side.
    at: Vec3,
    turned: Quat,
    /// The object as the drag has it.
    now: Placement,
}

impl ArrayTool {
    /// Where the object is drawn this frame: where the scene has it unless a
    /// drag of its start point or a gizmo drag of a copy is showing it somewhere
    /// else, which only the tool knows until the drag ends.
    pub(crate) fn drawn_placement(&self, info: &ObjectInfo<'_>) -> Placement {
        if let Some(posing) = self.posing.as_ref().filter(|p| p.pick.object == info.id) {
            return posing.now;
        }
        match self.shift.filter(|(moved, _)| *moved == info.id) {
            Some((_, by)) => Placement { position: info.placement.position + by, ..info.placement },
            None => info.placement,
        }
    }

    /// Whether the selected object's array has its copies off the path: the
    /// button that puts them back is only there to press then.
    pub(crate) fn has_copy_offset(&self, cx: &Ctx) -> bool {
        self.arrays.get(self.target(cx)).is_some_and(|live| live.state.has_copy_offset())
    }

    // ---- picking a copy ----------------------------------------------------------

    /// What is under a pixel: the nearest of the selected object's own copies
    /// (the object and its mirror copies among them), another object, or
    /// another object's copies. The scene finds objects and their mirror copies;
    /// the copies of an array are the tool's, so it casts against those itself.
    pub(crate) fn what_is_under(&self, cx: &Ctx, px: Vec2) -> Clicked {
        let (scene, ray) = (cx.scene(), cx.view.ray(px));
        let target = self.target(cx);

        let mut mine: Option<(f32, Place)> = None;
        let mut other = cx.pick(px).filter(|hit| hit.object != target).map(|hit| (hit.distance, hit.object));
        for (object, live) in self.arrays.iter() {
            let Some(info) = scene.info(object).filter(|info| info.visible) else { continue };
            let pose = pose_of(self.drawn_placement(&info));
            let mut places = copies::places(&live.layout, &pose, info.mirror);
            if object != target {
                // What the scene picks covers the other object's own place and its mirror copies.
                places.retain(|place| place.copy > 0);
            }
            let Some((distance, place)) = copies::pick(scene, object, &places, &ray) else { continue };
            if object == target {
                mine = Some((distance, place));
            } else if other.is_none_or(|(nearest, _)| distance < nearest) {
                other = Some((distance, object));
            }
        }
        // A copy has to be clearly nearer than another object to win: where they meet, the other
        // object is the one that was meant, since a copy cannot be told apart from it by the eye.
        match (mine, other) {
            (Some((near, place)), Some((far, _))) if near <= far * (1.0 - TIE) => {
                Clicked::Copy(CopyPick { object: target, copy: place.copy, side: place.side })
            }
            (Some((_, place)), None) => Clicked::Copy(CopyPick { object: target, copy: place.copy, side: place.side }),
            (_, Some((_, object))) => Clicked::Object(object),
            (None, None) => Clicked::Nothing,
        }
    }

    /// Puts the gizmo on a copy, as a click on it does. Picking a copy lets go
    /// of a picked point: the gizmo is on one or the other.
    pub(crate) fn select_copy(&mut self, cx: &mut Ctx, pick: CopyPick) {
        self.picked = None;
        self.copy = Some(pick);
        self.say(cx, "Turn or move this copy with the gizmo: every copy does the same.", true);
    }

    // ---- the gizmo ---------------------------------------------------------------

    /// Works out what the gizmo sits on this frame: the picked copy, with its
    /// arrows and rings but no cubes (a copy's size is the array's to set), or
    /// else the picked point of the path with arrows only (`arrows.rs`).
    /// Lets go of a copy that is no longer there: the array changed, was taken
    /// off, or the mirror side it was seen through is gone.
    pub(crate) fn refresh_gizmo(&mut self, cx: &Ctx) {
        self.pose_gizmo = None;
        let Some(pick) = self.copy else { return self.refresh_arrows(cx) };
        let scene = cx.scene();
        let live = self.arrays.get(pick.object);
        let info = scene.info(pick.object);
        let (Some(live), Some(info)) = (live, info) else {
            self.copy = None;
            return;
        };
        let gone = pick.object != self.target(cx)
            || pick.copy >= live.layout.len()
            || !copies::has_side(info.mirror, pick.side);
        if gone {
            self.copy = None;
            return;
        }
        if !cx.active || cx.clean_view || self.building.is_some() {
            return;
        }
        let frame = live.layout[pick.copy];
        let pose = frame.apply(&pose_of(self.drawn_placement(&info)));
        let Some(placement) = copies::seen_as(info.mirror, pick.side, &pose) else { return };
        let size = make::object_size(scene.bounds(pick.object).size());
        self.pose_gizmo = Some(GizmoTarget {
            placement,
            radius: size * 0.5 * frame.scale,
            moves: true,
            turns: true,
            scales: false,
        });
    }

    /// A drag of the gizmo on the picked copy, or on the picked point: where
    /// it belongs now.
    pub(crate) fn gizmo_event(&mut self, cx: &mut Ctx, placement: Placement, phase: GizmoPhase) {
        if self.arrows_event(cx, placement, phase) {
            return;
        }
        match phase {
            GizmoPhase::Begin => self.begin_pose(cx),
            GizmoPhase::Move => self.pose_to(cx, placement),
            GizmoPhase::End => {
                self.pose_to(cx, placement);
                self.finish_pose(cx);
            }
        }
    }

    fn begin_pose(&mut self, cx: &mut Ctx) {
        if self.building.is_some() {
            return;
        }
        self.commit_edit();
        let Some(pick) = self.copy else { return };
        let (Some(live), Some(info)) = (self.arrays.get(pick.object), cx.scene().info(pick.object)) else { return };
        let Some(&frame) = live.layout.get(pick.copy) else { return };
        let at = frame.apply(&pose_of(info.placement));
        self.posing = Some(Posing {
            pick,
            placement: info.placement,
            state: live.state.clone(),
            frame,
            at: at.position,
            turned: at.rotation,
            now: info.placement,
        });
    }

    /// Shows the object and the array as the drag has them. Always worked out
    /// from the press, never added to what the last sample gave, so nothing
    /// creeps. What a handle did not change is left as it was: a move leaves the
    /// turn alone and a turn the position (the gizmo turns the copy about its
    /// own middle).
    fn pose_to(&mut self, cx: &Ctx, placement: Placement) {
        let Some(posing) = self.posing.as_ref() else { return };
        let Some(info) = cx.scene().info(posing.pick.object) else { return };
        // The gizmo reports where the copy it is on is. Seen through a mirror
        // that is the reflection of the real copy, so it is reflected back.
        let (side, mirror) = (posing.pick.side, info.mirror);
        let Some((position, rotation)) = copies::reflected(mirror, side, placement.position, placement.rotation) else {
            return;
        };

        let mut next = posing.placement;
        let mut state = posing.state.clone();
        let delta = position - posing.at;
        // The gizmo's numbers carry rounding in proportion to how far from the origin they are.
        if delta.length() > MOVE_EPSILON * (1.0 + posing.at.length()) {
            let by = posing.frame.object_shift_for(delta);
            next.position += by;
            state.shift_object(by);
        }
        if turn_between(rotation, posing.turned) > TURN_EPSILON {
            next.rotation = posing.frame.object_rotation_for(rotation);
        }
        let object = posing.pick.object;
        if let Some(posing) = self.posing.as_mut() {
            posing.now = next;
        }
        self.arrays.set(object, state);
    }

    /// The drag is over: the object goes where it was taken, as one step of the
    /// scene's history, and the tool notes it in its own. A press that never
    /// moved is no edit.
    pub(crate) fn finish_pose(&mut self, cx: &mut Ctx) {
        let Some(posing) = self.posing.take() else { return };
        let object = posing.pick.object;
        let after = self.arrays.state(object);
        let restore = |tool: &mut ArrayTool| tool.arrays.set(object, posing.state.clone());
        if posing.now == posing.placement || !cx.scene().exists(object) {
            return restore(self);
        }
        let receipt = cx.commit("Pose Array Copies", vec![Command::Place { object, placement: posing.now }]);
        if !receipt.changed || receipt.failed > 0 {
            return restore(self);
        }

        // The step goes in the tool's history even when only the object turned, for the order
        // of Z (see `Scened`); there the array has nothing to change.
        let by = posing.now.position - posing.placement.position;
        let scened = Scened::Placed { from: posing.placement, to: posing.now, by };
        let what = if by == Vec3::ZERO { "Turned" } else { "Moved" };
        let (label, scene) = ("Pose Array Copies", Some(scened));
        self.history.push(Step { object, label, before: posing.state, after, scene });
        let text = format!("{what} every copy the same way. Z takes it back; Copies Back onto the Path undoes any move.");
        self.say(cx, text, true);
    }

    // ---- back onto the path ------------------------------------------------------

    /// Puts every copy back on the path, which stays where it is: the object
    /// goes back to the path's start and the points with it. Undoes any move made
    /// by posing a copy; a turn stays. One step.
    pub(crate) fn copies_onto_path(&mut self, cx: &mut Ctx) {
        let object = self.target(cx);
        let before = self.arrays.state(object);
        let Some(from) = cx.scene().info(object).map(|info| info.placement).filter(|_| before.has_copy_offset()) else {
            return;
        };
        self.cancel_gesture();
        self.commit_edit();

        let mut after = before.clone();
        let by = after.reset_copy_offset();
        let placement = Placement { position: from.position + by, ..from };
        let receipt = cx.commit("Array Copies Onto Path", vec![Command::Place { object, placement }]);
        if !receipt.changed || receipt.failed > 0 {
            return;
        }
        self.arrays.set(object, after.clone());
        let scened = Scened::Placed { from, to: placement, by };
        self.history.push(Step { object, label: "Array Copies Onto Path", before, after, scene: Some(scened) });
        self.say(cx, "Copies are back on the path.", true);
    }

    // ---- the object's centre moved under the array ------------------------------

    /// The app moved `object`'s centre onto its mesh (`Tool::origin_moved`):
    /// its position went from `from` to `to` and its mesh the other way, so
    /// the object did not move. The array is laid out from the object's
    /// position, so its points take the opposite of the move: the path stays
    /// where it is in the world, the object sits off the path's start by that
    /// much more, and every copy is where it was (a copy sits off the path the
    /// way the object does, in its own frame and size, and its mesh has gone
    /// the other way by as much).
    ///
    /// It is the pair a pose makes, so it is kept as one: when the scene's Z
    /// takes the object back, `notice_scene_steps` takes the points back.
    pub(crate) fn origin_moved(&mut self, object: ObjectId, from: Placement, to: Placement) {
        if !self.arrays.has(object) {
            return;
        }
        self.cancel_gesture();
        self.commit_edit();
        let before = self.arrays.state(object);
        let by = to.position - from.position;
        let mut after = before.clone();
        after.shift_object(by);
        self.arrays.set(object, after.clone());
        let scened = Scened::Placed { from, to, by };
        self.history.push(Step { object, label: "Set Origin to Mesh", before, after, scene: Some(scened) });
    }

    // ---- the mirror --------------------------------------------------------------

    /// The axes the selected object, and so its array, is mirrored across: X = 1,
    /// Y = 2, Z = 4.
    pub fn mirror_axes(&self, cx: &Ctx) -> u8 {
        cx.scene().info(self.target(cx)).and_then(|info| info.mirror).map_or(0, |m| m.axis_mask)
    }

    /// Switches the mirror across a world axis (0 X, 1 Y, 2 Z) on or off. One
    /// step of the scene's history, as in the Unity app: the object's live
    /// mirror, the same one the Mirror section edits, so it stays live.
    pub(crate) fn toggle_mirror(&mut self, cx: &mut Ctx, axis: usize) {
        let object = self.target(cx);
        let Some(info) = cx.scene().info(object) else {
            self.say(cx, "Select an object first.", false);
            return;
        };
        self.cancel_gesture();
        self.commit_edit();
        let center = info.mirror.map_or(Vec3::ZERO, |m| m.center);
        let axes = info.mirror.map_or(0, |m| m.axis_mask) ^ (1u8 << axis.clamp(0, 2));
        let (from, to) = (info.mirror, MirrorRepeater::new(axes, center));
        let receipt = cx.commit("Array Mirror", vec![Command::Mirror { object, mirror: to }]);
        if !receipt.changed || receipt.failed > 0 {
            return;
        }
        let scened = Scened::Mirrored { from, to };
        let state = self.arrays.state(object);
        let (label, scene) = ("Array Mirror", Some(scened));
        self.history.push(Step { object, label, before: state.clone(), after: state, scene });
        let text = match to {
            None => "Mirror off.".to_owned(),
            Some(m) => format!(
                "Mirrored across {}: every copy is repeated on the other side of the scene's centre.",
                MirrorRepeater::axis_name_of(m.axis_mask)
            ),
        };
        self.say(cx, text, true);
    }

    // ---- following the scene's history -------------------------------------------

    /// Notices that the scene's Z (or Shift+Z) has taken back or redone the
    /// object's half of a step, and does the array's half to match. The steps of
    /// a kind, newest first, stop at the first the scene still has done: older
    /// ones are done too. An object that something else has moved since is
    /// not one the tool can tell about, and is left alone.
    pub(crate) fn notice_scene_steps(&mut self, cx: &Ctx) {
        let scene = cx.scene();
        let mut settled: Vec<(ObjectId, Kind)> = Vec::new();
        let mut undone = Vec::new();
        for (i, step) in self.history.undo.iter().enumerate().rev() {
            let Some(scened) = step.scene else { continue };
            let key = (step.object, scened.kind());
            if settled.contains(&key) {
                continue;
            }
            match scened.in_effect(scene, step.object) {
                Some(false) => undone.push(i),
                _ => settled.push(key),
            }
        }
        for i in undone {
            let step = self.history.undo.remove(i);
            self.array_follows(&step, false);
            self.history.redo.push(step);
        }

        settled.clear();
        let mut redone = Vec::new();
        for (i, step) in self.history.redo.iter().enumerate().rev() {
            let Some(scened) = step.scene else { continue };
            let key = (step.object, scened.kind());
            if settled.contains(&key) {
                continue;
            }
            match scened.in_effect(scene, step.object) {
                Some(true) => redone.push(i),
                _ => settled.push(key),
            }
        }
        for i in redone {
            let step = self.history.redo.remove(i);
            self.array_follows(&step, true);
            self.history.undo.push(step);
        }
    }

    /// The array's half of a step the scene has just taken back (`in_effect`
    /// false) or done again. The state the step recorded is put back exactly if
    /// the array is as the step left it; if it has been edited since, the points
    /// are shifted by the same amount instead, which commutes with any edit.
    fn array_follows(&mut self, step: &Step, in_effect: bool) {
        if step.before == step.after {
            return;
        }
        let (was, becomes) = if in_effect { (&step.before, &step.after) } else { (&step.after, &step.before) };
        let current = self.arrays.state(step.object);
        if current == *was {
            self.arrays.set(step.object, becomes.clone());
        } else if let Some(Scened::Placed { by, .. }) = step.scene {
            let mut moved = current;
            moved.shift_object(if in_effect { by } else { -by });
            self.arrays.set(step.object, moved);
        }
    }
}
