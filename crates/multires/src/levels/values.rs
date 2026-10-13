//! Carrying other data between levels: per-vertex channels (a mask, a
//! layer's offsets) and per-face values (hidden faces, face groups).
//!
//! The stack carries the mesh's own mask itself. These are for whatever else
//! the app keeps per vertex or per face of the mesh it sculpts, so that it can
//! follow a change of level too.

use super::{Levels, with_refiner};
use glam::Vec3;

use super::sound;
use crate::detail::{self, Ride};
use crate::refine::{Channel, Refiner, place, raise};

/// One list of vectors riding on the surface, for [`Levels::riders_up`].
pub struct Rider<'a> {
    /// Its values for the finest level, as they were before the edit.
    pub finest: &'a [Vec3],
    /// Its values for the current level as they are now: the first entries
    /// of `finest`, unless the edit did something to them. What an edit does
    /// to a rider on the level it is made on depends on the kind of edit (a
    /// pose turns what stands on the skin, a push leaves it pointing where it
    /// pointed), so that is the caller's to say.
    pub level: &'a [Vec3],
}

impl Levels {
    /// A per-vertex channel of level `from` as it is on the coarser level
    /// `to`: the values of the vertices that level keeps.
    pub fn values_down<T: Copy>(&self, from: usize, to: usize, values: &[T]) -> Result<Vec<T>, String> {
        self.check_span(to, from)?;
        self.check_len(from, values.len())?;
        Ok(values[..self.sizes[to].vertices].to_vec())
    }

    /// A per-vertex channel of level `from` carried up to the finer level
    /// `to`. Kept vertices keep their values; each new vertex takes the mean
    /// of the vertices it lies between (the ends of its edge, the corners of
    /// its face). Nothing is smoothed and no value leaves the range of those
    /// around it.
    pub fn values_up<T: Channel>(&self, from: usize, to: usize, values: &[T]) -> Result<Vec<T>, String> {
        self.check_span(from, to)?;
        self.check_len(from, values.len())?;
        let mut now = values.to_vec();
        for level in from..to {
            now = with_refiner!(self, level, |r| raise(&r, &now));
        }
        Ok(now)
    }

    /// A list of points of level `from` put through the split that made
    /// level `from + 1` from it: what Subdivide would have given the finer
    /// level had these been the coarser one's positions.
    ///
    /// Both splits place every vertex at a weighted mean of coarser ones,
    /// with weights that depend on the faces alone. So the split of a sum is
    /// the sum of the splits, and handed something that was added to the
    /// positions (a sculpt layer's offsets) this gives what it adds to the
    /// finer level: its share of the smoothing included, which
    /// [`Self::values_up`] would leave behind in the rest.
    pub fn points_up(&self, from: usize, points: &[Vec3]) -> Result<Vec<Vec3>, String> {
        self.check_span(from, from + 1)?;
        self.check_len(from, points.len())?;
        let split = self.steps[from].split;
        Ok(with_refiner!(self, from, |r| place(&r, split, points)))
    }

    /// Lists of vectors that ride on the surface, carried from an edit of
    /// the current level up to the finest by the rule the positions
    /// themselves are carried by. A sculpt layer is such a list: an offset
    /// for every vertex, which is part of the position the stack holds, and
    /// which has to go with the skin when the form under it is changed.
    ///
    /// Call it on the stack as it is before the step that carries the edit
    /// (or on a clone of it): below its finest level. `positions` is the
    /// current level as it now stands, what [`Self::step_to`] is about to be
    /// handed. Each of `riders` comes back as it is for the finest level
    /// after the edit: going up, a rider's values are split as the positions
    /// are, into what the level below accounts for and the detail over that,
    /// and the detail is turned by the turn the position's detail is turned
    /// by.
    ///
    /// So the positions less a rider, after the edit, are the rest of the
    /// positions carried by the same turns: switching a layer off after a
    /// change of form takes the layer away and leaves the change. (`detail`
    /// says how close that is to the positions as they would have been had
    /// the rider never been part of them.) A value the edit does not reach
    /// comes back to the bit.
    pub fn riders_up(&self, positions: &[Vec3], riders: &[Rider]) -> Result<Vec<Vec<Vec3>>, String> {
        let (from, top) = (self.current, self.top());
        let kept = self.kept.as_ref().ok_or("On the finest level there is nothing finer to carry anything into.")?;
        self.check_len(from, positions.len())?;
        for rider in riders {
            self.check_len(top, rider.finest.len())?;
            self.check_len(from, rider.level.len())?;
        }
        let mut now = sound(positions, &kept.positions, |p| p.is_finite());
        let mut riding: Vec<Vec<Vec3>> = riders.iter().map(|rider| rider.level.to_vec()).collect();
        for level in from..top {
            let (coarse, fine) = (self.sizes[level].vertices, self.sizes[level + 1].vertices);
            let split = self.steps[level].split;
            let mut rides: Vec<Ride> = Vec::with_capacity(riders.len());
            for (rider, now) in riders.iter().zip(&riding) {
                rides.push(Ride { coarse_was: &rider.finest[..coarse], coarse_now: now, fine_was: &rider.finest[..fine] });
            }
            let (was, fine_was) = (&kept.positions[..coarse], &kept.positions[..fine]);
            let (next, carried) =
                with_refiner!(self, level, |r| detail::carry_with(&r, split, was, &now, fine_was, &rides));
            drop(rides);
            (now, riding) = (next, carried);
        }
        Ok(riding)
    }

    /// As [`Self::values_up`], but onto a finer level that already has
    /// values of its own. `fine` is the channel on level `to` as it was, and
    /// `edited` its entries for level `from` as they are now. Where nothing
    /// was edited the fine values come back to the bit; where something was,
    /// the change is carried up and added to what was there.
    pub fn values_up_over<T: Channel>(
        &self,
        from: usize,
        to: usize,
        edited: &[T],
        fine: &[T],
    ) -> Result<Vec<T>, String> {
        self.check_span(from, to)?;
        self.check_len(from, edited.len())?;
        self.check_len(to, fine.len())?;
        Ok(self.raise_edit(from, to, edited.to_vec(), fine, |value| value))
    }

    /// A per-triangle value carried from level `from` to level `to`, up or
    /// down, through the parent triangles (see [`face_values_up`] and
    /// [`face_values_down`]).
    pub fn face_values<T: Copy>(&self, from: usize, to: usize, values: &[T], missing: T) -> Result<Vec<T>, String> {
        self.check_level(from)?;
        self.check_level(to)?;
        if values.len() != self.sizes[from].triangles {
            return Err(format!(
                "There are {} face values; subdivision level {from} has {} triangles.",
                values.len(),
                self.sizes[from].triangles
            ));
        }
        let mut now = values.to_vec();
        for level in from..to {
            now = face_values_up(&with_refiner!(self, level, |r| r.parent_triangles()), &now);
        }
        for level in (to..from).rev() {
            let parents = with_refiner!(self, level, |r| r.parent_triangles());
            now = face_values_down(&parents, &now, self.sizes[level].triangles, missing);
        }
        Ok(now)
    }

    fn check_span(&self, lower: usize, higher: usize) -> Result<(), String> {
        self.check_level(higher)?;
        if lower <= higher { Ok(()) } else { Err(format!("Subdivision level {lower} is not below level {higher}.")) }
    }

    fn check_len(&self, level: usize, len: usize) -> Result<(), String> {
        let vertices = self.sizes[level].vertices;
        if len == vertices {
            Ok(())
        } else {
            Err(format!("There are {len} values; subdivision level {level} has {vertices} vertices."))
        }
    }
}

/// A per-face value on the finer level: each face takes its parent's.
/// `parents` is a parent map as [`Levels::parent_triangles`] gives it.
pub fn face_values_up<T: Copy>(parents: &[u32], coarse: &[T]) -> Vec<T> {
    parents.iter().map(|&parent| coarse[parent as usize]).collect()
}

/// A per-face value on the coarser level: each face takes the value of the
/// first (lowest numbered) of its children, or `missing` if it has none.
///
/// Children that disagree can only be settled by what the value means (is a
/// face hidden when any child is, or when all are?), so where that matters
/// the app should fold over the parent map itself.
pub fn face_values_down<T: Copy>(parents: &[u32], fine: &[T], coarse_count: usize, missing: T) -> Vec<T> {
    let mut coarse = vec![missing; coarse_count];
    // Backwards, so the lowest numbered child writes last.
    for (&parent, &value) in parents.iter().zip(fine).rev() {
        coarse[parent as usize] = value;
    }
    coarse
}
