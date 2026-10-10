//! The transform gizmo's arrows on a picked point of the path, for sliding it
//! along one axis. Ported from `ArrayController.RefreshGizmoTarget` and
//! `ArrayPointTarget`.
//!
//! Click a point and it is picked: the arrows sit on it, and dragging the dot
//! itself is still the free move on the plane facing the camera (the middle of
//! the gizmo is the tool's, see `GizmoTarget::radius`). A point can only be
//! moved, so the gizmo offers no rings and no cubes. The arrows are on a point
//! or on a copy, never both: picking a copy lets go of the point, and picking a
//! point lets go of the copy (`select_copy`, `press`).
//!
//! What a drag does is what dragging the dot does, so the copies follow as the
//! layout says and nothing is rebuilt:
//!
//! - any point but the first goes where the arrow takes it, an edit of the
//!   array that is one step of the tool's own history, however long the drag;
//! - the first point is the object's own place (the square), which the scene
//!   holds, so there the whole array is carried with the object, shown while
//!   the arrow is dragged and committed as one step of the scene's history when
//!   it is let go, exactly as for the square dragged by hand (`gestures.rs`).
//!
//! The drag is worked out from the press every time, never added to the last
//! sample, so a point lands exactly the gizmo's distance from where it was and
//! a press that never moved is no edit.

use glam::Vec3;
use sculpt_toolkit::{Ctx, GizmoPhase, GizmoTarget, ObjectId, Placement};

use crate::state::ArrayState;
use crate::tool::ArrayTool;

/// A move shorter than this (world units, per unit of distance from the
/// origin) is the gizmo's rounding, not a move.
const MOVE_EPSILON: f32 = 1e-6;

/// A drag of the arrows on a point, as it stands.
pub(crate) struct Pointing {
    object: ObjectId,
    index: usize,
    /// Where the gizmo had the point at the press, in the world.
    from: Vec3,
    /// The array at the press.
    state: ArrayState,
}

impl ArrayTool {
    /// Works out where the arrows sit this frame: on the picked point, with
    /// the object's place as it is drawn (a drag of the square carries the
    /// whole path). Lets go of a point that is no longer there: the array
    /// changed, was taken off, or the point was deleted or undone.
    pub(crate) fn refresh_arrows(&mut self, cx: &Ctx) {
        let Some(index) = self.picked else { return };
        let object = self.target(cx);
        let (live, info) = (self.arrays.get(object), cx.scene().info(object));
        let (Some(live), Some(info)) = (live, info) else {
            self.picked = None;
            return;
        };
        if index >= live.state.handle_count() {
            self.picked = None;
            return;
        }
        if !cx.active || cx.clean_view || self.building.is_some() {
            return;
        }
        let origin = self.drawn_placement(&info).position;
        self.pose_gizmo = Some(GizmoTarget::point(origin + live.state.points[index]));
    }

    /// An event of the gizmo that may be about a point rather than a copy.
    /// True when it was: which of the two the gizmo is on is settled when the
    /// drag begins, and holds until it ends.
    pub(crate) fn arrows_event(&mut self, cx: &mut Ctx, placement: Placement, phase: GizmoPhase) -> bool {
        match phase {
            GizmoPhase::Begin => {
                let Some(index) = self.picked.filter(|_| self.copy.is_none()) else { return false };
                self.begin_arrows(cx, index, placement.position);
                true
            }
            GizmoPhase::Move if self.pointing.is_some() => {
                self.arrows_to(placement.position);
                true
            }
            GizmoPhase::End if self.pointing.is_some() => {
                self.arrows_to(placement.position);
                self.finish_arrows(cx);
                true
            }
            _ => false,
        }
    }

    fn begin_arrows(&mut self, cx: &Ctx, index: usize, from: Vec3) {
        // A slider's edit that is still open is its own step.
        self.commit_edit();
        let object = self.target(cx);
        let Some(live) = self.arrays.get(object).filter(|live| index < live.state.handle_count()) else { return };
        let state = live.state.clone();
        if index > 0 {
            self.begin_edit(object, "Move Array Point", None);
        }
        self.pointing = Some(Pointing { object, index, from, state });
    }

    /// Shows the point where the arrow has taken it.
    fn arrows_to(&mut self, world: Vec3) {
        let Some(pointing) = self.pointing.as_ref() else { return };
        let mut by = world - pointing.from;
        if by.length() <= MOVE_EPSILON * (1.0 + pointing.from.length()) {
            by = Vec3::ZERO;
        }
        let (object, index) = (pointing.object, pointing.index);
        if index == 0 {
            self.shift = Some((object, by)).filter(|_| by != Vec3::ZERO);
            return;
        }
        let mut state = pointing.state.clone();
        state.points[index] += by;
        self.arrays.set(object, state);
    }

    /// The arrow was let go, or the tool was put away with it held. Every
    /// drag is one step: the point is an edit of the array, the square a move
    /// of the object in the scene. None if nothing moved.
    pub(crate) fn finish_arrows(&mut self, cx: &mut Ctx) {
        let Some(pointing) = self.pointing.take() else { return };
        if pointing.index == 0 {
            self.end_object_move(cx, pointing.index);
        } else {
            self.commit_edit();
        }
    }
}
