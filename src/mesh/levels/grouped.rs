//! Face groups through a change of subdivision level (see `mesh/groups.rs`).
//!
//! **Up**, a face is in the group of the face it was made from. **Down**, a
//! face takes the group most of the faces made from it are in (the first of
//! them, where two groups tie). The first child alone would be simpler and is
//! what the stack offers (`face_values_down`), but it always picks the same
//! corner of the coarse face, so every border would creep one way by up to a
//! whole coarse face on each step down; by majority the border goes to
//! whichever side has more of the face, which is as near as a coarse face can
//! get.
//!
//! **A round trip leaves the groups unchanged.** Neither rule can give back
//! a border finer than the coarse faces, so each level's list is remembered
//! when the level is left, and put back as it was on the way back. That is
//! right only while nothing has changed the groups in between: once a command
//! has, on any level, the lists remembered for the others describe groups
//! that no longer exist, and they are let go. So what is kept with a level's
//! mesh is the lists and the list the mesh arrived with; a step compares the
//! mesh's groups with the latter, which is cheap and cannot be fooled by a
//! list being carried and settled (a quad's two triangles made to agree)
//! slightly differently on the way.
//!
//! All lists here are in the stack's own triangle order, which does not
//! depend on how a mesh happened to be numbered when it was built.

use std::sync::Arc;

use rayon::prelude::*;
use sculpt_multires::Levels;

/// What a mesh with levels keeps about its face groups on the other levels.
#[derive(Clone, Default)]
pub(super) struct Kept {
    /// The list each level was left with; `None` for a level without groups,
    /// never left, or left before the groups last changed.
    remembered: Vec<Option<Arc<[u16]>>>,
    /// The mesh's own list as it was when the mesh was built on this level.
    arrived: Option<Arc<[u16]>>,
}

/// The group most of `ids` are, the first of them where two tie; 0 for none.
fn most(ids: &[u16]) -> u16 {
    let mut best = (0u16, 0usize);
    for (k, &id) in ids.iter().enumerate() {
        // Counted once, where the number is first met.
        if ids[..k].contains(&id) {
            continue;
        }
        let count = ids[k..].iter().filter(|&&other| other == id).count();
        if count > best.1 {
            best = (id, count);
        }
    }
    best.0
}

/// A level's groups as the groups of the level above are: one step down.
fn below(parents: &[u32], fine: &[u16], coarse_count: usize) -> Vec<u16> {
    // Each coarse face's children, side by side in their own order.
    let mut start = vec![0u32; coarse_count + 1];
    for &parent in parents {
        start[parent as usize + 1] += 1;
    }
    for face in 0..coarse_count {
        start[face + 1] += start[face];
    }
    let mut next = start.clone();
    let mut children = vec![0u16; parents.len()];
    for (&parent, &id) in parents.iter().zip(fine) {
        let at = &mut next[parent as usize];
        children[*at as usize] = id;
        *at += 1;
    }
    start.par_windows(2).with_min_len(1 << 14).map(|of| most(&children[of[0] as usize..of[1] as usize])).collect()
}

/// The groups of level `from` carried to level `to` by the rules in the
/// module notes.
fn carried(stack: &Levels, from: usize, to: usize, ids: &[u16]) -> Vec<u16> {
    if to >= from {
        return stack.face_values(from, to, ids, 0).expect("groups of the level they were read from");
    }
    let mut now = ids.to_vec();
    for level in (to..from).rev() {
        let parents = stack.parent_triangles(level + 1).expect("a level below the one the groups are of");
        now = below(&parents, &now, stack.triangle_count(level));
    }
    now
}

impl Kept {
    pub(super) fn bytes(&self) -> usize {
        // Shared with every mesh kept for undo since the list was made.
        let shared = |list: &Arc<[u16]>| list.len() * 2 / Arc::strong_count(list);
        self.remembered.iter().flatten().map(shared).sum::<usize>() + self.arrived.as_ref().map_or(0, shared)
    }

    /// Whether the mesh's groups, `now`, are the ones it was built with.
    fn unchanged(&self, now: &Option<Arc<[u16]>>) -> bool {
        self.arrived.as_deref() == now.as_deref()
    }

    /// The groups for level `to` of `stack` on leaving level `from` with
    /// `now` (empty for none), and what to keep from here on, short of the
    /// list the new mesh arrives with ([`Self::arrive`]).
    pub(super) fn step(&self, stack: &Levels, from: usize, to: usize, now: Option<Arc<[u16]>>) -> (Vec<u16>, Kept) {
        let mut remembered = if self.unchanged(&now) { self.remembered.clone() } else { Vec::new() };
        remembered.resize(stack.level_count(), None);
        let kept = remembered[to].as_ref().filter(|kept| kept.len() == stack.triangle_count(to));
        let there = match (kept, &now) {
            (Some(kept), _) => kept.to_vec(),
            (None, None) => Vec::new(),
            (None, Some(now)) => carried(stack, from, to, now),
        };
        remembered[from] = now;
        (there, Kept { remembered, arrived: None })
    }

    /// Notes the list the mesh of the new level was built with.
    pub(super) fn arrive(&mut self, list: Option<Arc<[u16]>>) {
        self.arrived = list;
    }

    /// What is kept once every level below `level` has gone (`lower`), or
    /// every level above it. The mesh is the same, so what it arrived with is.
    pub(super) fn without(&self, level: usize, lower: bool) -> Kept {
        let mut remembered = self.remembered.clone();
        if lower {
            remembered.drain(..level.min(remembered.len()));
        } else {
            remembered.truncate(level + 1);
        }
        Kept { remembered, arrived: self.arrived.clone() }
    }

    /// The remembered lists as a file keeps them: none when the mesh's
    /// groups, `now`, have changed since they were made.
    pub(super) fn saved(&self, now: &Option<Arc<[u16]>>) -> Vec<Option<Vec<u16>>> {
        if !self.unchanged(now) {
            return Vec::new();
        }
        self.remembered.iter().map(|list| list.as_ref().map(|list| list.to_vec())).collect()
    }

    /// The lists a file kept, for a mesh whose own groups are `now`. A list
    /// that is not its level's size is left out.
    pub(super) fn restored(saved: Vec<Option<Vec<u16>>>, stack: &Levels, now: Option<Arc<[u16]>>) -> Kept {
        let remembered = (0..stack.level_count())
            .map(|level| saved.get(level).cloned().flatten().filter(|list| list.len() == stack.triangle_count(level)))
            .map(|list| list.map(Arc::from))
            .collect();
        Kept { remembered, arrived: now }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coarse_face_takes_the_group_most_of_its_children_are_in() {
        assert_eq!(most(&[]), 0);
        assert_eq!(most(&[4, 4, 9, 4]), 4);
        assert_eq!(most(&[9, 4, 4, 4]), 4);
        // A tie goes to the group met first, so the answer does not depend on which number is larger.
        assert_eq!(most(&[9, 4, 4, 9]), 9);
        assert_eq!(most(&[4, 9, 9, 4]), 4);
        assert_eq!(most(&[0, 0, 7, 0]), 0);
        // Children listed out of order still land with their parent.
        let parents = [1, 0, 1, 0, 1, 1, 0, 0];
        let fine = [5, 2, 5, 2, 6, 5, 3, 2];
        assert_eq!(below(&parents, &fine, 3), vec![2, 5, 0]);
    }
}
