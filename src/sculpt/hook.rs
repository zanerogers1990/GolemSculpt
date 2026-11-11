//! What a Snake Hook pull has hold of. A pull moves the surface joined to
//! what it took hold of, not everything inside its ring: hooking one finger
//! leaves the finger beside it where it is, and pulling an ear leaves the
//! skull behind it alone, though both are inside the ring.
//!
//! The rule, for each copy of the pull (the pull as drawn, and each mirror
//! image or turn of it, every one on its own side):
//!
//! - Its first dab takes hold of the surface under the tip: the vertex
//!   nearest its centre, and everything joined to that within the ring.
//! - Every dab after takes hold of what the dab before held that is still
//!   inside the ring, and everything joined to that within the ring.
//! - A dab with nothing of that inside its ring takes hold afresh, as a
//!   first dab does.
//!
//! "Joined within the ring" is a walk along the mesh's edges that never takes
//! an edge with both ends outside the ring: a vertex just outside it can be
//! passed through, so the odd vertex near the rim whose only way in goes out
//! and back is not lost, but a part joined to the pull only somewhere further
//! off is not reached. The walk goes no further than the ring and one edge
//! beyond it, so it costs what the dab does, whatever the size of the mesh.
//!
//! What is held travels with the tip. The tip moves on by a fraction of the
//! radius each dab and the surface it pulls comes with it, so the rings of
//! two dabs in a row overlap almost entirely, and what the last dab held is
//! nearly all still inside: the walk from there picks up the rest of the
//! surface joined to it, the end of the pull and whatever the tip is
//! dragged across, and nothing else. A part that the pull swings past is
//! never joined to what it holds within the ring, so it is never taken.
//!
//! The pull lets go only where it has nothing left in its ring: the surface
//! fell so far behind the tip (a light touch, a low Strength) or the tip
//! went so far off it that nothing it held is inside any more. That pull is
//! over, and the dab takes hold of whatever is under the tip then, as a
//! fresh stroke there would. So a stroke that skims across a form, lifting
//! off it and coming back down, works it as it always did; only while it
//! holds something does it keep to it.
//!
//! Each copy keeps its own hold, so on a model whose two sides are mirror
//! images the two copies take hold of mirror images, and the result stays a
//! mirror image.
//!
//! What is kept between dabs is vertex numbers. Snake Hook does not refine
//! as it goes (`sculpt::dyntopo::refines`), so nothing changes them in a
//! pull; and should that change, dynamic topology never renumbers a vertex
//! that a dab has seen (mesh/dyntopo.rs): a split adds vertices, joined to
//! the ones it split between, which the next walk picks up; a merge leaves
//! the vertex it folded away with no neighbours, which is not taken hold of
//! again (on a surface refined before the pull, too).
//!
//! The walk is made a ring of edges at a time, each ring on all threads once
//! it is big enough. Most of what a dab holds was held by the dab before, and
//! is taken up at once, on all threads; only the sliver the tip has moved on
//! to is walked out from it, a few edges deep. Which vertices are held does
//! not depend on how the work was shared out, only the order they are found
//! in, and what is kept for the next dab is put in order.

use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;

use super::kernels::{Dab, SymOp};
use crate::mesh::Mesh;

/// What each copy of a pull holds, and the walk that finds it. Kept for the
/// stroke in its scratch buffers, so a dab costs no fresh memory to speak of.
#[derive(Default)]
pub struct Hooked {
    /// What each copy held after its last dab.
    held: Vec<(SymOp, Vec<u32>)>,
    /// A flag per vertex: held by the copy of the dab in hand. Clear between
    /// dabs (each dab clears what it set).
    on: Vec<AtomicU64>,
    /// What the copy of the dab in hand holds, a ring of the walk after
    /// another: the walk's queue.
    queue: Vec<u32>,
    /// The ring of the walk being found.
    found: Vec<u32>,
}

/// A ring of the walk this long or longer is shared out between threads.
const PAR_RING: usize = 4096;

/// How many vertices of a ring one task takes.
const RUN: usize = 2048;

impl Hooked {
    /// Takes hold, for the copy of a dab `d` with ring `radius`, of the
    /// surface joined to what that copy held after its last dab (see the
    /// rule above). `leaves` are the leaves the copy reaches, where a copy
    /// with nothing of that left looks for the vertex nearest its centre.
    pub fn take_hold(&mut self, mesh: &Mesh, radius: f32, d: &Dab, leaves: &[u32]) {
        let Hooked { held, on, queue, found } = self;
        let words = mesh.vertex_count().div_ceil(64);
        if on.len() < words {
            on.resize_with(words, || AtomicU64::new(0));
        }
        let on = &on[..];
        queue.clear();
        let (centre, r2) = (d.center, radius * radius);
        let inside = |v: u32| !(mesh.positions[v as usize].distance_squared(centre) >= r2);
        // A vertex dynamic topology merged away joins nothing.
        let joins = |v: u32| !mesh.neighbours(v).is_empty();
        let hidden = mesh.hidden_vertices();
        let shown = |v: u32| hidden.is_none_or(|hidden| !hidden[v as usize]);
        #[cfg(test)]
        if HOLD_ALL.get() {
            for &l in leaves {
                let leaf = &mesh.bvh.leaves[l as usize];
                queue.extend((leaf.vert_start..leaf.vert_end).filter(|&v| inside(v) && claim(on, v)));
            }
            return;
        }
        if let Some((_, before)) = held.iter().find(|(copy, _)| *copy == d.copy) {
            each_found(before, queue, |v, out| {
                if inside(v) && joins(v) && claim(on, v) {
                    out.push(v);
                }
            });
        }
        if queue.is_empty() {
            // Nothing held, or nothing of it left inside the ring: hold of
            // the surface under the tip, as at the start of a stroke.
            let mut nearest = (f32::INFINITY, None);
            for &l in leaves {
                let leaf = &mesh.bvh.leaves[l as usize];
                for v in leaf.vert_start..leaf.vert_end {
                    let d2 = mesh.positions[v as usize].distance_squared(centre);
                    if d2 < r2 && d2 < nearest.0 && shown(v) && joins(v) {
                        nearest = (d2, Some(v));
                    }
                }
            }
            if let Some(v) = nearest.1 {
                claim(on, v);
                queue.push(v);
            }
        }
        // Out from what is held so far, a ring of edges at a time. An edge
        // with an end inside the ring is walked; one with both ends outside
        // it is not.
        let mut from = 0;
        while from < queue.len() {
            let to = queue.len();
            found.clear();
            each_found(&queue[from..to], found, |v, out| {
                let from_inside = inside(v);
                for &o in mesh.neighbours(v) {
                    if (from_inside || inside(o)) && claim(on, o) {
                        out.push(o);
                    }
                }
            });
            queue.extend_from_slice(found);
            from = to;
        }
    }

    /// Whether the copy of the dab in hand holds vertex `v`.
    #[inline]
    pub fn holds(&self, v: u32) -> bool {
        self.on[v as usize >> 6].load(Ordering::Relaxed) & bit(v) != 0
    }

    /// Whether the copy of the dab in hand holds anything at all.
    pub fn holds_any(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Once the copy's dab is done: keeps what it held for its next dab, and
    /// clears the flags for the next copy.
    pub fn keep(&mut self, copy: SymOp) {
        let Hooked { held, on, queue, .. } = self;
        let on = &on[..];
        // In order, so that the threads taking it up at the next dab, and
        // clearing it here, each work on a run of flags of their own: spread
        // through the flags in the order the walk found them, they would
        // fight over the same words of memory.
        // Every flag set is a vertex in the queue, so the whole word goes.
        let clear = |v: &u32| on[*v as usize >> 6].store(0, Ordering::Relaxed);
        if queue.len() < PAR_RING {
            queue.sort_unstable();
            queue.iter().for_each(clear);
        } else {
            queue.par_sort_unstable();
            queue.par_iter().with_min_len(RUN).for_each(clear);
        }
        match held.iter_mut().find(|(c, _)| *c == copy) {
            Some((_, before)) => std::mem::swap(before, queue),
            // Still nothing found: the next dab looks again.
            None if queue.is_empty() => {}
            None => held.push((copy, std::mem::take(queue))),
        }
    }
}

/// Runs `find` over every vertex of `ring`, on all threads if it is long
/// enough, adding what it finds to `out`.
fn each_found(ring: &[u32], out: &mut Vec<u32>, find: impl Fn(u32, &mut Vec<u32>) + Sync) {
    if ring.len() < PAR_RING {
        for &v in ring {
            find(v, out);
        }
    } else {
        let runs: Vec<Vec<u32>> = ring
            .par_chunks(RUN)
            .map(|run| {
                let mut found = Vec::new();
                for &v in run {
                    find(v, &mut found);
                }
                found
            })
            .collect();
        for run in runs {
            out.extend(run);
        }
    }
}

#[inline]
fn bit(v: u32) -> u64 {
    1 << (v & 63)
}

/// Sets vertex `v`'s flag; whether it was clear (so this is the one walk
/// step that found it).
#[inline]
fn claim(on: &[AtomicU64], v: u32) -> bool {
    let word = &on[v as usize >> 6];
    let b = bit(v);
    word.load(Ordering::Relaxed) & b == 0 && word.fetch_or(b, Ordering::Relaxed) & b == 0
}

#[cfg(test)]
thread_local! {
    /// For tests: every pull takes hold of everything inside its ring, as
    /// every pull did before this file, to measure the rule against.
    pub static HOLD_ALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
