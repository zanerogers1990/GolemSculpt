//! Changing an object's array as steps of the tool's own history.
//!
//! An edit is opened (`begin_edit`) with the array as it was, changed freely
//! (a drag does it on every pointer sample), and closed (`commit_edit`) as one
//! step. Settings from the panel close by themselves a moment after the last
//! change, so a slider drag is one Z, as in the Unity app.

use sculpt_toolkit::{Ctx, ObjectId};

use crate::arrays::{Open, Step};
use crate::state::ArrayState;
use crate::tool::ArrayTool;

/// A slider that has been still this long has ended its step.
const COALESCE_SECONDS: f64 = 0.4;

impl ArrayTool {
    /// Opens an edit of `object`'s array, or carries on the one that is open
    /// if it is the same (`hold_until` is when it closes by itself).
    pub(crate) fn begin_edit(&mut self, object: ObjectId, label: &'static str, hold_until: Option<f64>) {
        let other = self.history.open.as_ref().is_some_and(|open| open.object != object || open.label != label);
        if other {
            self.commit_edit();
        }
        match &mut self.history.open {
            Some(open) => open.hold_until = hold_until,
            None => {
                self.history.open = Some(Open { object, label, before: self.arrays.state(object), hold_until });
            }
        }
    }

    /// Closes the open edit as one step. None when nothing actually changed.
    pub(crate) fn commit_edit(&mut self) {
        let Some(open) = self.history.open.take() else { return };
        let after = self.arrays.state(open.object);
        if open.before == after {
            return;
        }
        self.history.push(Step::array(open.object, open.label, open.before, after));
    }

    /// Puts back what the open edit changed: Esc during a drag.
    pub(crate) fn cancel_edit(&mut self) {
        if let Some(open) = self.history.open.take() {
            self.arrays.set(open.object, open.before);
        }
    }

    /// Whether the open edit has changed anything yet.
    pub(crate) fn edit_changed(&self) -> bool {
        self.history.open.as_ref().is_some_and(|open| open.before != self.arrays.state(open.object))
    }

    /// Closes a settings edit that has been still for a moment. Asks for
    /// another frame while one is waiting, since nothing else would.
    pub(crate) fn close_edit_when_due(&mut self, cx: &mut Ctx) {
        let Some(until) = self.history.open.as_ref().and_then(|open| open.hold_until) else { return };
        if cx.time >= until {
            self.commit_edit();
        } else {
            cx.repaint();
        }
    }

    /// Changes the selected object's array as one step. `slider` edits that
    /// arrive in a run (a slider being dragged) join into one.
    pub(crate) fn edit(
        &mut self,
        cx: &mut Ctx,
        label: &'static str,
        slider: bool,
        change: impl FnOnce(&mut ArrayState),
    ) {
        let object = self.target(cx);
        if !self.arrays.has(object) {
            return;
        }
        self.begin_edit(object, label, slider.then_some(cx.time + COALESCE_SECONDS));
        let mut state = self.arrays.state(object);
        change(&mut state);
        self.arrays.set(object, state);
        if !slider {
            self.commit_edit();
        }
        self.clamp_picked();
    }

    /// An undone handle (a point that is no longer there) is let go of.
    pub(crate) fn clamp_picked(&mut self) {
        // The picked handle belongs to the selected object's array; the
        // caller has only just changed it, so look at what the last target held.
        let Some(object) = self.last_target else { return };
        let count = self.arrays.get(object).map_or(0, |live| live.state.handle_count());
        if self.picked.is_some_and(|p| p >= count) {
            self.picked = None;
        }
    }

    /// Z: cancels a gesture in progress, else takes back the last edit.
    /// False (so the scene's history answers) when the tool has none, or when
    /// the newest is one the scene holds (see [`crate::arrays::Scened`]): the
    /// scene's Ctrl+Z takes it back, and the tool follows when it sees that.
    pub(crate) fn undo(&mut self, cx: &mut Ctx) -> bool {
        if self.cancel_gesture() {
            self.say(cx, "Cancelled.", true);
            return true;
        }
        self.commit_edit();
        if self.history.undo.last().is_none_or(|step| step.scene.is_some()) {
            return false;
        }
        let Some(step) = self.history.undo.pop() else { return false };
        self.arrays.set(step.object, step.before.clone());
        self.say(cx, format!("Undid {}. Ctrl+Shift+Z brings it back.", step.label), true);
        self.history.redo.push(step);
        self.clamp_picked();
        true
    }

    /// Ctrl+Shift+Z.
    pub(crate) fn redo(&mut self, cx: &mut Ctx) -> bool {
        self.commit_edit();
        if self.history.redo.last().is_none_or(|step| step.scene.is_some()) {
            return false;
        }
        let Some(step) = self.history.redo.pop() else { return false };
        self.arrays.set(step.object, step.after.clone());
        self.say(cx, format!("Redid {}.", step.label), true);
        self.history.undo.push(step);
        self.clamp_picked();
        true
    }
}
