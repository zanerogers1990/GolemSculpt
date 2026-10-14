//! A stack taken apart into plain lists, and put back together.
//!
//! The app writes these into its own file format; nothing here touches a
//! file. What is written is small because the finer levels' faces and tables
//! follow by rule from the coarsest level's faces: only those faces, how each
//! level was split, and the kept finest level go out. Coming back in, the
//! tables are rebuilt and everything is checked against them, so a file that
//! was cut short or put together wrongly is refused here and not found out
//! later by an index out of range.
//!
//! This is also why the numbering rules in [`crate::loop_scheme`] and
//! [`crate::catmull_clark`] must never change: a saved position belongs to a
//! vertex only by its number.

use glam::Vec3;

use super::{Kept, Levels};
use crate::mesh::Polys;
use crate::refine::Split;

/// Everything a [`Levels`] holds, as plain lists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelsParts {
    /// Vertices of the coarsest level.
    pub vertex_count: usize,
    /// The coarsest level's triangles.
    pub triangles: Vec<u32>,
    /// The coarsest level's polygon faces, for a polygon mesh.
    pub polys: Option<Polys>,
    /// How each level above the coarsest was made, coarsest first.
    pub splits: Vec<Split>,
    /// The level the app's mesh is of.
    pub current: usize,
    /// The finest level's positions, as the stack keeps them while the app
    /// is on a coarser level. Empty when `current` is the finest level: the
    /// app's own mesh is the finest level then, and is the app's to save.
    pub positions: Vec<Vec3>,
    /// The finest level's mask, kept and left empty as `positions` is.
    pub mask: Vec<f32>,
}

impl Levels {
    /// The stack as plain lists. The app's own mesh, the current level's, is
    /// not in them: on a coarser level it may hold edits the stack has not
    /// been handed yet, and those are found again after loading the same way
    /// they would have been, by comparing it with what the stack holds.
    pub fn to_parts(&self) -> LevelsParts {
        let (positions, mask) = self.kept.as_ref().map_or((Vec::new(), Vec::new()), |kept| {
            (kept.positions.to_vec(), kept.mask.to_vec())
        });
        LevelsParts {
            vertex_count: self.sizes[0].vertices,
            triangles: self.faces[0].triangles(),
            polys: self.faces[0].polys(),
            splits: self.steps.iter().map(|step| step.split).collect(),
            current: self.current,
            positions,
            mask,
        }
    }

    /// [`Self::to_parts`] without copying the kept level.
    pub fn into_parts(mut self) -> LevelsParts {
        let kept = self.kept.take();
        let mut parts = self.to_parts();
        if let Some(kept) = kept {
            (parts.positions, parts.mask) = kept.into_lists();
        }
        parts
    }

    /// Puts a stack back together, or says what does not fit.
    pub fn from_parts(parts: LevelsParts) -> Result<Levels, String> {
        let LevelsParts { vertex_count, triangles, polys, splits, current, positions, mask } = parts;
        let mut levels = Levels::new(vertex_count, triangles, polys)?;
        for split in splits {
            levels.grow(split)?;
        }
        let top = levels.top();
        if current > top {
            return Err(format!("The saved subdivision level {current} is past the finest level, {top}."));
        }
        // On the finest level nothing is kept; below it, all of the finest level is.
        let expected = if current == top { 0 } else { levels.sizes[top].vertices };
        if positions.len() != expected || mask.len() != expected {
            return Err(format!(
                "The saved subdivision levels hold {} positions and {} mask values where {expected} belong.",
                positions.len(),
                mask.len()
            ));
        }
        levels.current = current;
        levels.kept = (current < top).then(|| Kept::of(positions, mask));
        Ok(levels)
    }
}
