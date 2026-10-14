//! Taking a change of level back.
//!
//! A step can change two things that stepping the other way does not put
//! back: the kept finest level (when the level being left was edited, the
//! edit is carried up into it) and the mesh of the level being left (which
//! the app handed in and no longer holds). Both changes are sparse, so the
//! record of a step is the vertices that changed with what they held before,
//! and a step that carried nothing up records nothing at all.
//!
//! Redoing a step is taking it again with the mesh [`Levels::undo_step`]
//! handed back: everything here is deterministic, so it lands where it did.

use std::sync::Arc;

use glam::Vec3;
use rayon::prelude::*;

use super::{Kept, Levels};
use crate::mesh::LevelMesh;
use crate::refine::{Channel, PER_TASK};

/// What [`Levels::step_to`] hands back.
#[derive(Clone, Debug, PartialEq)]
pub struct Stepped {
    /// The mesh of the level arrived at, for the app to carry on with.
    pub mesh: LevelMesh,
    /// What [`Levels::undo_step`] needs to take the step back.
    pub undo: StepUndo,
}

/// What one [`Levels::step_to`] changed; enough to take it back exactly.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepUndo {
    from: usize,
    to: usize,
    /// How many levels the stack had, to refuse a record from another time.
    levels: usize,
    /// Where the level left differed from the stack's copy of it: the app's edits.
    live_positions: Sparse<Vec3>,
    live_mask: Sparse<f32>,
    /// What the kept finest level held where carrying those edits up changed it.
    kept_positions: Sparse<Vec3>,
    kept_mask: Sparse<f32>,
}

/// Some entries of an array: which, and their values.
#[derive(Clone, Debug, Default, PartialEq)]
struct Sparse<T> {
    at: Vec<u32>,
    values: Vec<T>,
}

impl<T: Channel> Sparse<T> {
    /// The entries of `values` that differ from `other`'s.
    fn differing(values: &[T], other: &[T]) -> Sparse<T> {
        let at: Vec<u32> = (0..values.len() as u32)
            .into_par_iter()
            .with_min_len(PER_TASK)
            .filter(|&i| !values[i as usize].same(other[i as usize]))
            .collect();
        let values = at.iter().map(|&i| values[i as usize]).collect();
        Sparse { at, values }
    }

    /// The entries `at` (ascending) of `values`.
    fn at(values: &[T], at: Vec<u32>) -> Sparse<T> {
        let values = at.iter().map(|&i| values[i as usize]).collect();
        Sparse { at, values }
    }

    fn write_into(&self, array: &mut [T]) {
        for (&i, &value) in self.at.iter().zip(&self.values) {
            array[i as usize] = value;
        }
    }

    /// Whether every entry lies inside an array of `len`.
    fn fits(&self, len: usize) -> bool {
        self.at.iter().all(|&i| (i as usize) < len)
    }

    fn bytes(&self) -> usize {
        self.at.len() * (4 + std::mem::size_of::<T>())
    }
}

impl StepUndo {
    pub(super) fn new(from: usize, to: usize, levels: usize) -> StepUndo {
        StepUndo { from, to, levels, ..StepUndo::default() }
    }

    /// The level the step left.
    pub fn from(&self) -> usize {
        self.from
    }

    /// The level the step arrived at.
    pub fn to(&self) -> usize {
        self.to
    }

    /// Whether the step carried an edit of the level it left up into the
    /// finer levels. If not, it changed nothing but which level is current.
    pub fn carried_an_edit(&self) -> bool {
        !self.live_positions.at.is_empty() || !self.live_mask.at.is_empty()
    }

    /// What the record holds in memory, for an undo history's budget.
    pub fn bytes(&self) -> usize {
        self.live_positions.bytes() + self.live_mask.bytes() + self.kept_positions.bytes() + self.kept_mask.bytes()
    }

    /// Notes where the level left (`live`) differs from the stack's copy
    /// (the first entries of `kept`), and gives back where, ascending, if it
    /// does anywhere.
    pub(super) fn note_live_positions(&mut self, live: &[Vec3], kept: &[Vec3]) -> Option<&[u32]> {
        self.live_positions = Sparse::differing(live, &kept[..live.len()]);
        (!self.live_positions.at.is_empty()).then_some(&self.live_positions.at[..])
    }

    pub(super) fn note_live_mask(&mut self, live: &[f32], kept: &[f32]) -> Option<&[u32]> {
        self.live_mask = Sparse::differing(live, &kept[..live.len()]);
        (!self.live_mask.at.is_empty()).then_some(&self.live_mask.at[..])
    }

    /// Notes what the kept finest level held (`was`) at the vertices the
    /// step changed, `at`, ascending.
    pub(super) fn note_kept_positions(&mut self, was: &[Vec3], at: Vec<u32>) {
        self.kept_positions = Sparse::at(was, at);
    }

    pub(super) fn note_kept_mask(&mut self, was: &[f32], at: Vec<u32>) {
        self.kept_mask = Sparse::at(was, at);
    }
}

impl Levels {
    /// Takes back the step `undo` records. The app must be where that step
    /// left it: on level `undo.to()`, with `positions` and `mask` that
    /// level's mesh as the step handed it over (anything sculpted since
    /// undone first). Gives back the mesh of level `undo.from()` exactly as
    /// the app handed it to that step, edits and all, and the stack is as it
    /// was before the step.
    pub fn undo_step(&mut self, undo: &StepUndo, positions: &[Vec3], mask: &[f32]) -> Result<LevelMesh, String> {
        let top = self.top();
        let fits = undo.levels == self.level_count()
            && undo.to == self.current
            && undo.from <= top
            && undo.live_positions.fits(self.sizes[undo.from.min(top)].vertices)
            && undo.live_mask.fits(self.sizes[undo.from.min(top)].vertices)
            && undo.kept_positions.fits(self.sizes[top].vertices)
            && undo.kept_mask.fits(self.sizes[top].vertices);
        if !fits {
            return Err("This undo step does not belong to the subdivision levels as they are now.".into());
        }
        self.check_mesh(positions, mask)?;

        // Arriving at the finest level handed the kept arrays to the app;
        // they come back the same way.
        let mut kept = match self.kept.take() {
            Some(kept) => kept,
            None => Kept::of(positions.to_vec(), mask.to_vec()),
        };
        // Copied first if a clone of the stack shares them, and only then.
        if !undo.kept_positions.at.is_empty() {
            undo.kept_positions.write_into(Arc::make_mut(&mut kept.positions).as_mut_slice());
        }
        if !undo.kept_mask.at.is_empty() {
            undo.kept_mask.write_into(Arc::make_mut(&mut kept.mask).as_mut_slice());
        }
        self.current = undo.from;
        if undo.from == top {
            let (positions, mask) = kept.into_lists();
            return Ok(self.mesh(top, positions, mask));
        }
        let (mut live_positions, mut live_mask) = kept.first(self.sizes[undo.from]);
        undo.live_positions.write_into(&mut live_positions);
        undo.live_mask.write_into(&mut live_mask);
        self.kept = Some(kept);
        Ok(self.mesh(undo.from, live_positions, live_mask))
    }
}
