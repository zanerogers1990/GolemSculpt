//! The arrays the tool holds, and the history of edits made to them.
//!
//! There is no array in the scene: the tool keeps one `ArrayState` per object
//! it has been given, with its layout worked out so a frame only multiplies
//! matrices. The history is the tool's own (the scene's has no room for this),
//! stepped by Z while the tool is up. An edit is a before and an after of one
//! object's whole array, which is small: a handful of settings and points.

use std::collections::BTreeMap;

use sculpt_toolkit::ObjectId;

use crate::layout::compute;
use crate::state::ArrayState;
use crate::types::ArrayPlacement;

/// Edits kept for undo.
const HISTORY_LIMIT: usize = 200;

/// One object's array and where each of its copies goes, relative to the object.
pub struct Live {
    pub state: ArrayState,
    pub layout: Vec<ArrayPlacement>,
}

/// Every object's array, by object.
#[derive(Default)]
pub struct Arrays {
    map: BTreeMap<ObjectId, Live>,
}

impl Arrays {
    /// The object's array, or the empty state when it has none.
    pub fn state(&self, object: ObjectId) -> ArrayState {
        self.map.get(&object).map_or_else(ArrayState::none, |live| live.state.clone())
    }

    pub fn get(&self, object: ObjectId) -> Option<&Live> {
        self.map.get(&object)
    }

    pub fn has(&self, object: ObjectId) -> bool {
        self.map.contains_key(&object)
    }

    /// Gives `object` exactly this array (put in range: see
    /// `ArrayState::sanitized`); the empty state takes it away.
    pub fn set(&mut self, object: ObjectId, state: ArrayState) {
        let state = state.sanitized();
        if state.is_none() {
            self.map.remove(&object);
            return;
        }
        let layout = compute(&state.settings, &state.points);
        self.map.insert(object, Live { state, layout });
    }

    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &Live)> {
        self.map.iter().map(|(&object, live)| (object, live))
    }
}

/// One finished edit.
#[derive(Clone, Debug)]
pub struct Step {
    pub object: ObjectId,
    pub label: &'static str,
    pub before: ArrayState,
    pub after: ArrayState,
}

/// An edit still being made: a drag, or a run of slider ticks that will be one
/// step once the slider has been still for a moment.
#[derive(Clone, Debug)]
pub struct Open {
    pub object: ObjectId,
    pub label: &'static str,
    pub before: ArrayState,
    /// When the edit closes by itself (`Ctx::time`), pushed back by each tick
    /// of a slider; `None` for an edit something else closes.
    pub hold_until: Option<f64>,
}

#[derive(Default)]
pub struct History {
    pub undo: Vec<Step>,
    pub redo: Vec<Step>,
    pub open: Option<Open>,
}

impl History {
    /// A new edit: whatever was undone can no longer be redone.
    pub fn push(&mut self, step: Step) {
        self.redo.clear();
        self.undo.push(step);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Drops every step that belongs to `object`: its array is gone for good.
    pub fn forget(&mut self, object: ObjectId) {
        self.undo.retain(|s| s.object != object);
        self.redo.retain(|s| s.object != object);
        if self.open.as_ref().is_some_and(|o| o.object == object) {
            self.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::types::{ArrayKind, ArraySettings};

    fn line() -> ArrayState {
        ArrayState { settings: ArraySettings::new(ArrayKind::Line), points: vec![Vec3::ZERO, Vec3::X * 4.0] }
    }

    #[test]
    fn an_array_is_stored_with_its_layout_and_none_takes_it_away() {
        let mut arrays = Arrays::default();
        let a = ObjectId(1);
        assert!(arrays.state(a).is_none() && !arrays.has(a));
        arrays.set(a, line());
        assert_eq!(arrays.get(a).unwrap().layout.len(), 5);
        assert_eq!(arrays.state(a), line());
        arrays.set(a, ArrayState::none());
        assert_eq!(arrays.iter().count(), 0);
    }

    #[test]
    fn what_is_stored_is_put_in_range() {
        let mut arrays = Arrays::default();
        let mut wild = line();
        wild.settings.count = 5000;
        wild.points.truncate(1);
        arrays.set(ObjectId(1), wild);
        let live = arrays.get(ObjectId(1)).unwrap();
        assert_eq!(live.state.settings.count, ArraySettings::MAX_COUNT);
        assert_eq!(live.state.points.len(), 2, "every array has a point to aim at");
        assert_eq!(live.layout.len(), ArraySettings::MAX_COUNT as usize);
    }

    #[test]
    fn a_new_step_clears_the_redo_stack_and_forgetting_an_object_drops_only_its_steps() {
        let mut history = History::default();
        let step =
            |object: u64| Step { object: ObjectId(object), label: "x", before: ArrayState::none(), after: line() };
        history.push(step(1));
        history.push(step(2));
        history.redo.push(step(1));
        history.push(step(1));
        assert!(history.redo.is_empty());
        history.forget(ObjectId(1));
        assert_eq!(history.undo.len(), 1);
        assert_eq!(history.undo[0].object, ObjectId(2));
    }

    #[test]
    fn the_history_is_bounded() {
        let mut history = History::default();
        for _ in 0..HISTORY_LIMIT + 50 {
            history.push(Step { object: ObjectId(1), label: "x", before: ArrayState::none(), after: line() });
        }
        assert_eq!(history.undo.len(), HISTORY_LIMIT);
    }
}
