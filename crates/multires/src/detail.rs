//! Carrying an edit of a coarse level up into the finer level's detail.
//!
//! The finer level is its base surface (the coarser level split by the rule
//! the level was made with) plus detail: what sculpting at the finer level
//! added on top. When the coarser level is edited the base moves, and the
//! detail has to go with it: a wart on an arm that is bent must stay on the
//! arm, the same height, still standing out of the skin.
//!
//! So each fine vertex's detail is a vector from the old base surface to
//! where the vertex was, and it is turned by however the base surface turned
//! at that vertex before being added to the new base. How the surface turned
//! is read off the fine vertex's own fan of base faces, before and after:
//! [`crate::turn`] says how, and why it counts every edge of the fan alike
//! rather than picking one tangent edge.
//!
//! Detail is carried at its own size. Scale the coarse level up and a pore
//! stays a pore on a larger head; that is what a sculptor means by keeping
//! the detail, and what ZBrush and Blender do.
//!
//! Only what the edit reaches is recomputed. A fine vertex whose base did not
//! move, and none of whose fan moved, keeps its position to the bit: adding a
//! detail back to the base it was measured from does not in general return
//! the same float, and a model that shifted by rounding each time a level was
//! visited would not be the model that was sculpted.
//!
//! # Riders
//!
//! The app may keep vectors of its own on the surface: a sculpt layer is an
//! offset for every vertex, part of the position the stack holds. When a
//! coarse level is edited those have to be carried up too, and **by the same
//! rule as the positions**, with the same turn at every vertex: a rider is
//! split into its own base (its coarse values put through the level's split,
//! which is the part of the position's base it accounts for) and its own
//! detail, and the detail is turned by the turn the position's detail was
//! turned by. Then the positions less a rider are what is left of the
//! positions: the base and the detail of everything else, carried by those
//! same turns, with nothing of the rider in it. ([`carry_with`].)
//!
//! On the level that was edited there is no base to split against, and
//! what became of a rider there depends on what kind of edit it was (a pose
//! turns it, a push leaves it be), which only the app knows: the app says
//! what each rider's values on that level are now, and this carries them up.
//!
//! That is not quite what the positions would have been had the rider never
//! been there, because the turns are read off faces the rider is part of: a
//! rider as rough as the mesh is fine tilts them. The difference is that
//! change of turn applied to the rest's own detail over its base, small times
//! small; `tests/riders.rs` measures it (a few hundredths of the rider under
//! a hard twist, none at all over one level).

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use crate::refine::{Channel, PER_TASK, Refiner, Split, place};
use crate::turn::{Fan, Turn};

/// The finer level after its coarser level was edited from `coarse_was` to
/// `coarse_now`, given the finer level as it was.
pub(crate) fn carry<R: Refiner>(
    r: &R,
    split: Split,
    coarse_was: &[Vec3],
    coarse_now: &[Vec3],
    fine_was: &[Vec3],
) -> Vec<Vec3> {
    let (was, now) = (place(r, split, coarse_was), place(r, split, coarse_now));
    let moved = differing(&was, &now);
    (0..r.total())
        .into_par_iter()
        .with_min_len(PER_TASK)
        .map(|i| {
            if !reached(r, &moved, i) {
                return fine_was[i];
            }
            carried_one(r, i, fine_was[i], |v| (was[v], now[v]))
        })
        .collect()
}

/// A list of vectors riding on the surface through one step up: its values
/// on the coarser level before the edit and after it, and on the finer level
/// before.
pub(crate) struct Ride<'a> {
    pub coarse_was: &'a [Vec3],
    pub coarse_now: &'a [Vec3],
    pub fine_was: &'a [Vec3],
}

/// [`carry`], with each rider's values on the finer level after the edit as
/// well: see the module notes. The positions are the ones `carry` gives, to
/// the bit, and the riders are turned by the turns those were turned by.
///
/// A rider's value comes back to the bit where neither the surface nor the
/// rider's own base changed.
pub(crate) fn carry_with<R: Refiner>(
    r: &R,
    split: Split,
    coarse_was: &[Vec3],
    coarse_now: &[Vec3],
    fine_was: &[Vec3],
    rides: &[Ride],
) -> (Vec<Vec3>, Vec<Vec<Vec3>>) {
    let (was, now) = (place(r, split, coarse_was), place(r, split, coarse_now));
    let moved = differing(&was, &now);
    // Each rider's own base, before and after: one list where it did not change.
    let bases: Vec<(Vec<Vec3>, Option<Vec<Vec3>>)> = rides
        .iter()
        .map(|ride| {
            let unchanged = differing(ride.coarse_was, ride.coarse_now).iter().all(|changed| !changed);
            (place(r, split, ride.coarse_was), (!unchanged).then(|| place(r, split, ride.coarse_now)))
        })
        .collect();
    // One pass over the vertices, so that each turn is worked out once: the
    // position and every rider's value for a vertex side by side.
    let count = rides.len() + 1;
    let mut all = vec![Vec3::ZERO; r.total() * count];
    all.par_chunks_mut(count).enumerate().with_min_len(PER_TASK).for_each(|(i, out)| {
        let turn = reached(r, &moved, i).then(|| Turn::of(&BaseFan { r, vertex: i, base: |v| (was[v], now[v]) }));
        out[0] = match turn {
            Some(turn) => (now[i].as_dvec3() + turn.apply(fine_was[i].as_dvec3() - was[i].as_dvec3())).as_vec3(),
            None => fine_was[i],
        };
        for ((ride, (base_was, base_now)), out) in rides.iter().zip(&bases).zip(&mut out[1..]) {
            let (from, to) = (base_was[i], base_now.as_ref().map_or(base_was[i], |base| base[i]));
            *out = match turn {
                None if from.same(to) => ride.fine_was[i],
                _ => {
                    let detail = ride.fine_was[i].as_dvec3() - from.as_dvec3();
                    (to.as_dvec3() + turn.map_or(detail, |turn| turn.apply(detail))).as_vec3()
                }
            };
        }
    });
    let list = |k: usize| all.iter().skip(k).step_by(count).copied().collect();
    (list(0), (1..count).map(list).collect())
}

/// Where fine vertex `i`, at `fine_was` before the edit, is after it: its
/// detail over the base it was measured from, turned as the base turned
/// round it and put on the base as it is now. `base(v)` is fine vertex `v`'s
/// base before the edit and after it, for `i` and every vertex of its fan.
#[inline]
pub(crate) fn carried_one<R: Refiner>(r: &R, i: usize, fine_was: Vec3, base: impl Fn(usize) -> (Vec3, Vec3)) -> Vec3 {
    let (was, now) = base(i);
    let detail = fine_was.as_dvec3() - was.as_dvec3();
    let turn = Turn::of(&BaseFan { r, vertex: i, base });
    (now.as_dvec3() + turn.apply(detail)).as_vec3()
}

/// Where two lists differ, bit for bit.
fn differing(was: &[Vec3], now: &[Vec3]) -> Vec<bool> {
    was.par_iter().zip(now).with_min_len(PER_TASK).map(|(a, b)| !a.same(*b)).collect()
}

/// Whether the base moved at fine vertex `i` or anywhere on its fan.
fn reached<R: Refiner>(r: &R, moved: &[bool], i: usize) -> bool {
    let mut any = moved[i];
    if !any {
        r.fan(i, |a, b| any |= moved[a as usize] | moved[b as usize]);
    }
    any
}

/// The fan of base faces round one fine vertex, in the base as it was and
/// as it is now: `base(v)` is fine vertex `v`'s base before and after.
struct BaseFan<'a, R, B> {
    r: &'a R,
    vertex: usize,
    base: B,
}

impl<R: Refiner, B: Fn(usize) -> (Vec3, Vec3)> Fan for BaseFan<'_, R, B> {
    fn wedges(&self, mut wedge: impl FnMut([DVec3; 2], [DVec3; 2])) {
        let (centre_was, centre_now) = (self.base)(self.vertex);
        let (centre_was, centre_now) = (centre_was.as_dvec3(), centre_now.as_dvec3());
        let spoke = |v: u32| {
            let (was, now) = (self.base)(v as usize);
            (was.as_dvec3() - centre_was, now.as_dvec3() - centre_now)
        };
        self.r.fan(self.vertex, |a, b| {
            let (a, b) = (spoke(a), spoke(b));
            wedge([a.0, b.0], [a.1, b.1])
        });
    }
}
