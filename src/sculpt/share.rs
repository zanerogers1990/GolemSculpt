//! What the copies of a dab do to a vertex they all reach, made into one move.
//!
//! Under symmetry a dab is laid with its copies: its images in the mirror
//! planes, its turns round the radial axis. Far apart they are simply
//! applied in turn. Where their footprints meet, the copies that can meet
//! are each applied to the surface as it stood before any of them, and what
//! they did to a vertex is banked here ([`Shared`]) and settled once all
//! have had their turn ([`Settling`]). Nothing depends on the order they
//! came in, so the result is symmetric by construction.
//!
//! What it is settled as is one rule for every such set, a pair across one
//! mirror plane, four or eight copies where planes cross, or the ring round
//! an axis: **a dab is as strong beside a mirror plane, on it, or at the
//! axis, as it is anywhere else.** Copies that land on one spot act as one
//! dab, profile and all. Before, each kind of set had its own way: a pair
//! was applied in both orders and averaged, which for most brushes is
//! adding the two up, so a stroke down the centre line of a mirrored model
//! was twice as strong as the same stroke beside it; four copies and rings
//! were added up and divided by how much their discs overlap, which made a
//! dab half a radius from two planes half as strong as it should be.
//!
//! There are two ways of settling it, because brushes are of two kinds
//! ([`Sharing`]).
//!
//! A brush that **adds** a step each dab (Clay, Snake Hook, and Standard,
//! Inflate and Crease with Accumulate on) has its copies' moves added up and
//! divided by how many dabs the copies count as between them
//! ([`Tally`]): all of them when they land on one spot, one when they do not
//! touch, and in between by how much of what they did overlaps. One number
//! for the whole set, so it cannot favour a copy, and a sum divided by a
//! number is as smooth as the dabs are: there is no line down the middle
//! where a stroke meets its image. What the number is counted from is what
//! the copies did, so it follows the brush's own falloff.
//!
//! One number cannot be right everywhere under the dab: where the copies
//! partly overlap, more of them cover the middle than the set counts as, and
//! the sum comes out taller there than a dab is anywhere by itself (a
//! quarter taller a third of a radius from a plane, three quarters under two
//! flat-topped tips, twice as tall where four of those meet). So what a
//! vertex is moved is eased off as it passes the furthest any one copy moved
//! any vertex, and never reaches a twentieth more ([`OVER`]). Up to that
//! distance nothing is changed, so copies on one spot are still one dab to
//! the bit.
//!
//! A brush that **seeks** a place (Layer, Flatten, Smooth, and Standard,
//! Inflate and Crease without Accumulate) closes a share of the gap to where
//! it wants the surface each dab, and stops there. Adding up what its copies
//! ask for would go past that place, and such a brush does not come back:
//! Layer stood a third too tall where two planes crossed. So each vertex
//! goes the way the copies ask weighted by how far each asked for it to go,
//! heavily: all but as far as the one that asked most, and exactly as far
//! where they agree. Never further than any one of them asked, so never
//! past the place, and no faster on a mirror plane than off it. (Where two
//! such dabs meet, the surface has a crease between them; so it has however
//! they are laid, since each stops at its own height.)
//!
//! Dividing each vertex of an adding brush by the number of copies that
//! cover it was tried first in the Unity app, for the ring round an axis. It
//! turned N coincident dabs into a flat-topped mesa with a cliff where the
//! cover fell below one, and Inflate amplified float noise on that cliff to
//! 4% of the stroke. And taking the furthest copy's move for an adding brush
//! was tried here: it leaves a hard line down the centre of the model
//! wherever a stroke runs beside its image.

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use super::kernels;
use crate::mesh::Mesh;
use crate::util::SharedMut;

/// How the copies of a dab that overlap share a vertex between them: see
/// the top of this file. Which a brush is, is `Params::sharing`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Sharing {
    /// Each dab adds a step: the copies' moves are added up and divided by
    /// how many dabs they count as.
    Adds,
    /// Each dab closes a share of the gap to a place: a vertex goes all but
    /// as far as the copy that asked most of it.
    Seeks,
}

/// How much further than the furthest any one copy moved any vertex the
/// copies of an adding brush may move one between them, as a share of that
/// distance: the most an overlap stands taller than the dab does alone.
const OVER: f64 = 0.05;

/// What the copies of one set have done to a vertex between them.
///
/// As small as it can be: a dab's footprint on a dense mesh is tens of
/// thousands of these, written once for every copy.
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Shared {
    /// The copies' moves added up; for a brush that seeks, each weighted
    /// first by its share of `weight`.
    moved: DVec3,
    /// For a brush that adds, the squares of how far the copies moved the
    /// vertex, added up whatever way each went. For one that seeks, their
    /// weights added up: the fourth power of how far each went.
    weight: f64,
}

/// What the copies of a set of an adding brush did over the whole of their
/// footprint, kept as they are banked: how many dabs they count as between
/// them comes from it ([`Tally::count`]).
///
/// Each copy counts for itself, and for every other copy by how much of what
/// that one did lies where this one did the most: over the whole footprint,
/// the sum of the square of (the squares of how far the copies moved a
/// vertex, added up), against the sum of the fourth powers of how far they
/// moved it. That is the number of copies when they all did the same thing
/// in the same place (a dab on a mirror plane, or on the axis), 1 when no
/// two of them moved the same vertex, and it passes smoothly from one to the
/// other as the dab travels away, by the brush's own falloff and not by its
/// outline. How far each moved a vertex is what counts, not which way: a
/// pull across a mirror plane and its image, which cancel, are still two
/// dabs in one place, and what is left of them is their average.
///
/// The squares weight the count toward where a copy is strongest, under the
/// pen. Counted plainly (each copy by the overlap of everything it did) a
/// dab a third of a radius from a plane came out a sixth weaker under the
/// pen than the same dab alone; like this it is a twentieth, and what that
/// makes the overlap taller by is eased off ([`OVER`]).
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(super) struct Tally {
    /// Vertex by vertex, the square of the copies' squares added up.
    together: f64,
    /// The copies' fourth powers, every one by itself.
    alone: f64,
    /// The square of the furthest any one copy moved any vertex.
    furthest: f64,
}

impl Tally {
    /// Adds what another part of the footprint, or another copy, came to.
    pub(super) fn add(&mut self, other: Tally) {
        self.together += other.together;
        self.alone += other.alone;
        self.furthest = self.furthest.max(other.furthest);
    }

    /// How many dabs the copies count as between them.
    fn count(&self) -> f64 {
        if self.alone > 0.0 { (self.together / self.alone).max(1.0) } else { 1.0 }
    }
}

impl Shared {
    /// Banks what one copy did to the vertex, and for a brush that adds,
    /// what that comes to in `tally`. A copy that did not move the vertex
    /// has no say in where it goes.
    ///
    /// Added up in double. A vertex and its twin across a plane, or in
    /// another sector, get the same moves in a different order, and float
    /// addition is not associative: with four terms or more the twins come
    /// out an ulp apart, a seed every brush that follows the normals then
    /// amplifies. A handful of floats adds up exactly in double, so for a
    /// brush that adds the order stops mattering; for one that seeks it
    /// matters to the last bit of a double, which a float does not show.
    #[inline]
    pub(super) fn add(&mut self, sharing: Sharing, step: Vec3, tally: &mut Tally) {
        if step == Vec3::ZERO {
            return;
        }
        let step = step.as_dvec3();
        let far2 = step.length_squared();
        match sharing {
            Sharing::Adds => {
                // The square of the sum, a term at a time: this copy's own
                // square, and twice its product with those before it.
                let own = far2 * far2;
                tally.together += own + 2.0 * self.weight * far2;
                tally.alone += own;
                tally.furthest = tally.furthest.max(far2);
                self.moved += step;
                self.weight += far2;
            }
            Sharing::Seeks => {
                let weight = far2 * far2;
                self.moved += step * weight;
                self.weight += weight;
            }
        }
    }
}

/// How what the copies of a set did is made into one move a vertex: the
/// kind of brush, and for one that adds, the two numbers that are the same
/// for every vertex under the set.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Settling {
    sharing: Sharing,
    /// One over how many dabs the copies count as between them.
    share: f64,
    /// The furthest any one copy moved any vertex.
    furthest: f64,
}

impl Settling {
    /// For the copies of a brush of this kind that came to `tally` between
    /// them (which only a brush that adds has anything in).
    pub(super) fn of(sharing: Sharing, tally: &Tally) -> Settling {
        match sharing {
            Sharing::Seeks => Settling { sharing, share: 1.0, furthest: 0.0 },
            Sharing::Adds => Settling { sharing, share: 1.0 / tally.count(), furthest: tally.furthest.sqrt() },
        }
    }

    /// The one move the copies' moves on a vertex come to, or `None` for a
    /// vertex no copy moved, which is then not written at all and keeps its
    /// position to the bit.
    #[inline]
    pub(super) fn step(&self, shared: &Shared) -> Option<Vec3> {
        if shared.weight <= 0.0 {
            return None;
        }
        Some(match self.sharing {
            Sharing::Seeks => (shared.moved / shared.weight).as_vec3(),
            Sharing::Adds => held_to(shared.moved * self.share, self.furthest),
        })
    }
}

/// `step`, eased off where it goes further than `furthest` ([`eased`]).
#[inline]
fn held_to(step: DVec3, furthest: f64) -> Vec3 {
    let far2 = step.length_squared();
    if far2 <= furthest * furthest {
        step.as_vec3()
    } else {
        let far = far2.sqrt();
        (step * (eased(far / furthest) * furthest / far)).as_vec3()
    }
}

/// The same rule for a Move grab, whose copies do not dab. Each takes hold
/// of the surface once, at the press, and from then on carries every vertex
/// it holds by its weight there times the pointer's travel as that copy
/// makes it. Those travels are all one length, so how many grabs the copies
/// count as between them ([`Tally::count`]) is a matter of the weights alone,
/// and is settled at the press for the whole drag.
///
/// Before, a vertex had its copies' pulls added up and divided by its own
/// total weight once that passed 1. A grab on a mirror plane then held
/// everything inside half its radius as firmly as its centre and let go
/// twice as steeply from there: a mesa where the falloff should be, along
/// the centre line of every mirrored model, whatever the Focal Shift.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Hold {
    /// One over how many grabs the copies count as between them.
    share: f64,
    /// The firmest any one copy holds any vertex.
    firmest: f64,
}

impl Hold {
    /// From the weights of every grabbed vertex, one for each copy that
    /// holds it.
    pub(super) fn of<'a>(vertices: impl Iterator<Item = &'a [f32]>) -> Hold {
        let mut tally = Tally::default();
        for weights in vertices {
            let (mut squares, mut fourths) = (0.0, 0.0);
            for &weight in weights {
                let square = weight as f64 * weight as f64;
                squares += square;
                fourths += square * square;
                tally.furthest = tally.furthest.max(square);
            }
            tally.together += squares * squares;
            tally.alone += fourths;
        }
        Hold { share: 1.0 / tally.count(), firmest: tally.furthest.sqrt() }
    }

    /// How far a vertex goes. `pulled` is the pulls of the copies that hold
    /// it, each times its weight, added up; `travel` is how far the grabbed
    /// point has been carried.
    #[inline]
    pub(super) fn settle(&self, pulled: DVec3, travel: f64) -> Vec3 {
        // No two copies hold the same vertex: each has one copy's pull, whole.
        if self.share == 1.0 {
            return pulled.as_vec3();
        }
        held_to(pulled * self.share, travel * self.firmest)
    }
}

/// How far a vertex goes that the copies between them would move `over`
/// times the furthest any one of them moved a vertex, as a multiple of that
/// distance too: `over` itself up to 1, and from there it eases off, with no
/// corner where it starts to, and never passes `1 + OVER`.
#[inline]
fn eased(over: f64) -> f64 {
    if over <= 1.0 { over } else { 1.0 + OVER * (1.0 - (-(over - 1.0) / OVER).exp()) }
}

/// Banks what the copy just applied did to every vertex of `own`, measured
/// from `saved`, where those vertices stood before it, and puts them back
/// there for the next copy. `own` is some or all of `leaves`, whose vertices
/// `saved` and `shared` hold leaf after leaf, the leaf at `leaves[k]` from
/// `starts[k]` (`kernels::layout`). Gives back what the copy came to, for a
/// brush that adds.
///
/// That is totalled leaf by leaf and the leaves added up in their order, so
/// the answer does not depend on how the work was shared between threads.
pub(super) fn bank(
    mesh: &mut Mesh,
    own: &[u32],
    leaves: &[u32],
    starts: &[usize],
    saved: &[Vec3],
    shared: &mut [Shared],
    sharing: Sharing,
) -> Tally {
    let ranges = &mesh.bvh.leaves;
    let owned = |leaf: u32| kernels::owned(&ranges[leaf as usize]);
    let place = |leaf: u32| leaves.binary_search(&leaf).map_or(0, |k| starts[k]);
    let total: usize = own.iter().map(|&leaf| owned(leaf).len()).sum();
    let (live, sums) = (SharedMut::new(&mut mesh.positions), SharedMut::new(shared));
    let settle = |&leaf: &u32| {
        let (verts, at) = (owned(leaf), place(leaf));
        // SAFETY: `own` lists each leaf once, every leaf owns its own run of
        // vertices, and `leaves` gives each its own stretch of `shared`.
        let (live, sums) = unsafe { (live.slice(verts.clone()), sums.slice(at..at + verts.len())) };
        let mut tally = Tally::default();
        for ((pos, shared), &before) in live.iter_mut().zip(sums.iter_mut()).zip(&saved[at..]) {
            shared.add(sharing, *pos - before, &mut tally);
            *pos = before;
        }
        tally
    };
    debug_assert!(own.windows(2).all(|w| w[0] < w[1]));
    let by_leaf: Vec<Tally> = if total < kernels::PAR_MIN {
        own.iter().map(settle).collect()
    } else {
        own.par_iter().with_min_len(64).map(settle).collect()
    };
    let mut tally = Tally::default();
    by_leaf.into_iter().for_each(|leaf| tally.add(leaf));
    tally
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A footprint as the copies of a set leave it: for each vertex, what
    /// each copy did to it. The copies are banked one after another, as the
    /// walks do it.
    fn footprint(sharing: Sharing, vertices: &[Vec<Vec3>]) -> (Vec<Shared>, Settling) {
        let mut shared = vec![Shared::default(); vertices.len()];
        let mut tally = Tally::default();
        let copies = vertices.iter().map(Vec::len).max().unwrap_or(0);
        for copy in 0..copies {
            for (shared, moves) in shared.iter_mut().zip(vertices) {
                shared.add(sharing, moves.get(copy).copied().unwrap_or(Vec3::ZERO), &mut tally);
            }
        }
        (shared, Settling::of(sharing, &tally))
    }

    /// What a set of copies comes to at a vertex that is all they reached,
    /// and how many dabs they count as.
    fn settled(sharing: Sharing, moves: &[Vec3]) -> (Option<Vec3>, f64) {
        let (shared, settling) = footprint(sharing, &[moves.to_vec()]);
        (settling.step(&shared[0]), 1.0 / settling.share)
    }

    const UP: Vec3 = Vec3::new(0.0, 0.25, 0.0);

    /// Copies that land on one spot are one dab, however many there are and
    /// whichever kind the brush is: the centre line of a mirrored model, the
    /// corner where planes cross, the pole of a ring of thirty-two.
    #[test]
    fn copies_on_one_spot_act_as_one_dab() {
        for sharing in [Sharing::Adds, Sharing::Seeks] {
            for copies in [1usize, 2, 4, 8, 32] {
                let (step, count) = settled(sharing, &vec![UP; copies]);
                assert_eq!(step, Some(UP), "{sharing:?}, {copies} copies");
                assert_eq!(count, if sharing == Sharing::Adds { copies as f64 } else { 1.0 });
            }
        }
    }

    /// A vertex no copy moved is not written, and a copy that did not move
    /// it has no say: the one that did moves it as it would alone.
    #[test]
    fn a_copy_that_did_not_reach_a_vertex_has_no_say_in_it() {
        for sharing in [Sharing::Adds, Sharing::Seeks] {
            assert_eq!(settled(sharing, &[]).0, None);
            assert_eq!(settled(sharing, &[Vec3::ZERO; 4]).0, None);
            assert_eq!(settled(sharing, &[Vec3::ZERO, UP, Vec3::ZERO]), (Some(UP), 1.0), "{sharing:?}");
        }
    }

    /// Two copies of a bump along a line, `apart` samples from each other,
    /// as an adding brush lays them: what they did to every sample.
    fn two_bumps(apart: i32) -> (Vec<Shared>, Settling) {
        let bump = |at: i32| {
            let t = 1.0 - (at as f32 / 40.0).abs().min(1.0);
            UP * (t * t * (3.0 - 2.0 * t))
        };
        let vertices: Vec<Vec<Vec3>> = (-100..=100).map(|at| vec![bump(at - apart / 2), bump(at + apart / 2)]).collect();
        footprint(Sharing::Adds, &vertices)
    }

    /// Copies that do not touch count as one dab each: every vertex has one
    /// copy's move and gets all of it. Between that and all on one spot the
    /// count follows how much of what the copies did overlaps, and never
    /// falls under one dab.
    #[test]
    fn the_count_passes_from_all_the_copies_to_one_as_they_draw_apart() {
        let counted = |apart: i32| 1.0 / two_bumps(apart).1.share;
        assert_eq!(counted(0), 2.0);
        assert_eq!(counted(80), 1.0, "a whole footprint apart they do not touch");
        let mut last = 2.0;
        for apart in [4, 10, 20, 30, 40, 60, 78] {
            let count = counted(apart);
            assert!(count < last && count >= 1.0, "{apart} apart: {count} after {last}");
            last = count;
        }
        assert_eq!(Tally::default().count(), 1.0, "nothing moved is one dab, not none");
        // Each vertex with a move of its own size, none shared.
        let alone: Vec<Vec<Vec3>> = (0..500).map(|at| vec![UP * (1.0 + (at % 17) as f32)]).collect();
        let (shared, settling) = footprint(Sharing::Adds, &alone);
        assert_eq!((1.0 / settling.share, settling.step(&shared[5])), (1.0, Some(UP * 6.0)));
    }

    /// The tally is the same whatever order the copies are banked in, and
    /// however the footprint is cut up to be totalled.
    #[test]
    fn the_tally_does_not_depend_on_the_order_of_the_copies() {
        let moves = [UP, UP * 0.5, UP * 2.0];
        let tally_of = |order: [usize; 3]| {
            let (mut shared, mut tally) = (Shared::default(), Tally::default());
            order.iter().for_each(|&copy| shared.add(Sharing::Adds, moves[copy], &mut tally));
            (tally, shared.weight)
        };
        let (first, weight) = tally_of([0, 1, 2]);
        // (1 + 1/4 + 4)^2 and 1 + 1/16 + 16, in sixteenths of a sixteenth.
        assert_eq!((first.together, first.alone, first.furthest), (27.5625 / 256.0, 17.0625 / 256.0, 0.25));
        for order in [[2, 1, 0], [1, 0, 2], [2, 0, 1]] {
            assert_eq!(tally_of(order), (first, weight));
        }
        let mut whole = Tally::default();
        whole.add(Tally { together: 1.0, alone: 0.5, furthest: 0.25 });
        whole.add(Tally { together: 2.0, alone: 1.5, furthest: 0.125 });
        assert_eq!(whole, Tally { together: 3.0, alone: 2.0, furthest: 0.25 });
    }

    /// However far apart the two are, the pair is as strong under the pen as
    /// one dab alone, give or take a tenth, nowhere more than a twentieth
    /// taller, and as smooth as the bump itself: no step from one sample to
    /// the next is bigger than the bump's own steepest.
    #[test]
    fn a_dab_and_its_image_are_as_strong_as_one_dab_wherever_they_stand() {
        let (alone, _) = two_bumps(200);
        let own_steepest = alone.windows(2).map(|w| (w[1].moved.y - w[0].moved.y).abs()).fold(0.0, f64::max);
        for apart in 0..=84 {
            let (footprint, settling) = two_bumps(apart);
            let height = |at: i32| settling.step(&footprint[(at + 100) as usize]).map_or(0.0, |step| step.y as f64);
            let (pen, tallest) = (height(apart / 2), (-100..=100).map(height).fold(0.0, f64::max));
            let what = format!("{apart} apart: {pen:.3} under the pen, {tallest:.3} at the tallest, of 0.25");
            assert!((0.9..=1.0 + OVER + 1e-6).contains(&(pen / 0.25)), "{what}");
            assert!(tallest / 0.25 <= 1.0 + OVER + 1e-6, "{what}");
            let steepest = (-100..100).map(|at| (height(at + 1) - height(at)).abs()).fold(0.0, f64::max);
            assert!(steepest <= own_steepest * 1.05, "{what}; steepest {steepest:.5} against {own_steepest:.5}");
        }
    }

    /// The two copies of a Move grab along a line, `apart` samples from each
    /// other: how firmly each holds every sample.
    fn two_holds(apart: i32) -> (Vec<[f32; 2]>, Hold) {
        let bump = |at: i32| {
            let t = 1.0 - (at as f32 / 40.0).abs().min(1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let weights: Vec<[f32; 2]> = (-100..=100).map(|at| [bump(at - apart / 2), bump(at + apart / 2)]).collect();
        let hold = Hold::of(weights.iter().map(|held| &held[..]));
        (weights, hold)
    }

    /// A grab and its image, both pulling the same way, carry the surface as
    /// one grab does wherever they stand: on one spot they are one grab,
    /// falloff and all; clear of each other each is itself; and between, the
    /// grabbed point goes with the pen give or take a tenth, nothing goes a
    /// twentieth further than the pen, and no step is steeper than the
    /// falloff's own.
    #[test]
    fn a_grab_and_its_image_hold_as_one_grab_wherever_they_stand() {
        let pull = DVec3::new(0.0, 0.25, 0.0);
        let carried = |weights: &[[f32; 2]], hold: &Hold, at: i32| {
            let held = weights[(at + 100) as usize];
            hold.settle(pull * held[0] as f64 + pull * held[1] as f64, 0.25).y as f64
        };
        let (alone, apart_hold) = two_holds(200);
        assert_eq!((apart_hold.share, apart_hold.firmest), (1.0, 1.0));
        let (on_one_spot, one_hold) = two_holds(0);
        assert_eq!(one_hold.share, 0.5);
        for at in -100..=100 {
            let one = 0.25 * on_one_spot[(at + 100) as usize][0] as f64;
            assert!((carried(&on_one_spot, &one_hold, at) - one).abs() < 1e-7, "{at}");
        }
        let own_steepest = (-100..100)
            .map(|at| (carried(&alone, &apart_hold, at + 1) - carried(&alone, &apart_hold, at)).abs())
            .fold(0.0, f64::max);
        for apart in 0..=84 {
            let (weights, hold) = two_holds(apart);
            let height = |at: i32| carried(&weights, &hold, at);
            let (pen, tallest) = (height(apart / 2), (-100..=100).map(height).fold(0.0, f64::max));
            let what = format!("{apart} apart: {pen:.3} under the pen, {tallest:.3} at the furthest, of 0.25");
            assert!((0.9..=1.0 + OVER + 1e-6).contains(&(pen / 0.25)), "{what}");
            assert!(tallest / 0.25 <= 1.0 + OVER + 1e-6, "{what}");
            let steepest = (-100..100).map(|at| (height(at + 1) - height(at)).abs()).fold(0.0, f64::max);
            assert!(steepest <= own_steepest * 1.05, "{what}; steepest {steepest:.5} against {own_steepest:.5}");
        }
        // Nothing grabbed is one grab, not none.
        assert_eq!(Hold::of(std::iter::empty()).share, 1.0);
    }

    /// A brush that adds: the moves are added up and divided by the count,
    /// so a pull across a mirror plane and its image leave what they agree
    /// on, one dab's worth of it.
    #[test]
    fn what_adds_is_added_up_and_shared_out_by_the_count() {
        let (pull, image) = (Vec3::new(0.3, 0.4, 0.0), Vec3::new(-0.3, 0.4, 0.0));
        let (step, count) = settled(Sharing::Adds, &[pull, image]);
        assert_eq!(count, 2.0, "two dabs in one place, whichever way they pull");
        assert_eq!(step, Some(Vec3::new(0.0, 0.4, 0.0)));
    }

    /// What a vertex is moved past the furthest any one copy moved any
    /// vertex is eased off: not at all up to that distance, without a corner
    /// from there, and never as much as a twentieth more.
    #[test]
    fn an_overlap_is_never_much_taller_than_the_dab_alone() {
        assert_eq!((eased(0.0), eased(0.6), eased(1.0)), (0.0, 0.6, 1.0));
        let mut last = 1.0;
        for over in [1.001, 1.01, 1.05, 1.2, 1.56, 2.0, 8.0] {
            let eased = eased(over);
            assert!(eased > last && eased < over && eased <= 1.0 + OVER, "{over} is eased to {eased}");
            last = eased;
        }
        assert!((eased(1.001) - 1.001).abs() < 1e-4, "no corner where it starts");
        assert!(eased(8.0) > 1.0 + OVER - 1e-9);
        // Two flat-topped tips half over each other: the middle, which both
        // cover, is not twice as tall as the rest, and leans the way they do.
        let lean = Vec3::new(0.15, 0.2, 0.0);
        let (shared, settling) = footprint(Sharing::Adds, &[vec![lean], vec![lean, lean], vec![lean, lean], vec![Vec3::ZERO, lean]]);
        let (count, furthest) = (1.0 / settling.share, settling.furthest);
        assert!(count > 1.0 && count < 2.0 && (furthest - 0.25).abs() < 1e-7, "{count} dabs, {furthest} at the furthest");
        let middle = settling.step(&shared[1]).unwrap();
        assert!(middle.length() > 0.25 && middle.length() <= 0.25 * (1.0 + OVER as f32), "{middle}");
        assert!(middle.normalize().distance(lean.normalize()) < 1e-6);
        assert!(settling.step(&shared[0]).unwrap().distance(lean / count as f32) < 1e-7);
    }

    /// A brush that seeks: a vertex goes all but as far as the copy that
    /// asked most, never further than any copy asked, and the way the copies
    /// agree on where they pull against each other.
    #[test]
    fn what_seeks_goes_no_further_than_the_copy_that_asked_most() {
        let far = |step: Option<Vec3>| step.unwrap().length();
        for (strong, weak) in [(1.0f32, 0.9f32), (1.0, 0.5), (1.0, 0.1), (0.2, 0.19), (3.0, 0.0)] {
            let (step, _) = settled(Sharing::Seeks, &[UP * strong, UP * weak]);
            let (most, went) = (0.25 * strong, far(step));
            assert!(went <= most * 1.000_001, "{strong} and {weak}: went {went}, past {most}");
            assert!(went >= most * 0.92, "{strong} and {weak}: went only {went} of {most}");
            assert_eq!(step, settled(Sharing::Seeks, &[UP * weak, UP * strong]).0, "the order they came in");
        }
        // Four that cover the middle alike, where Layer piled up a third too tall.
        assert_eq!(settled(Sharing::Seeks, &[UP; 4]).0, Some(UP));
        // Across a mirror plane: what the two agree on.
        let (step, _) = settled(Sharing::Seeks, &[Vec3::new(0.3, 0.4, 0.0), Vec3::new(-0.3, 0.4, 0.0)]);
        assert_eq!(step, Some(Vec3::new(0.0, 0.4, 0.0)));
    }
}
