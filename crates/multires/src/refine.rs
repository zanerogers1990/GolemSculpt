//! What the two schemes share: how one level is made from the one below it.
//!
//! A scheme ([`crate::loop_scheme`], [`crate::catmull_clark`]) answers a few
//! questions about a single fine vertex, each from flat tables of the coarser
//! level, and everything that walks a whole level is written once here against
//! those answers ([`Refiner`]):
//!
//! * Where does it sit under the smooth rule? (`smooth`)
//! * Which kept vertices is a new one between? (`between`: the ends of its
//!   edge, or the corners of its face. Flat splitting, a mask and every other
//!   per-vertex channel are the plain mean of these.)
//! * Which fine vertices surround it, wound the way the faces are? (`fan`,
//!   which is all the detail frames need: see [`crate::detail`].)
//!
//! Every answer is a gather: a fine vertex reads the coarse vertices it
//! depends on and writes only itself. That is what lets a level be filled on
//! every thread with a result that does not depend on which thread did what,
//! and what lets a step back up recompute only the vertices an edit reached.
//!
//! Sums are taken in double precision and in a fixed order, then rounded once.
//! A vertex and its mirror twin list their neighbours in different orders, and
//! in single precision that alone would make the two halves of a symmetric
//! model drift apart a little more with every level.

use glam::{DVec3, Vec3};
use rayon::prelude::*;

use crate::mesh::Faces;

/// "No index": the missing second face of a boundary edge, and the like.
pub(crate) const NONE: u32 = u32::MAX;

/// In a vertex's rule: it moves by the scheme's interior rule.
pub(crate) const SMOOTH: u32 = u32::MAX;

/// In a vertex's rule: it stays where it is.
pub(crate) const FIXED: u32 = u32::MAX - 1;

/// Most vertices a level may have, so that no vertex is ever numbered like
/// one of the marks above.
pub(crate) const MOST_VERTICES: usize = (u32::MAX - 2) as usize;

/// Vertices handed to one rayon task.
pub(crate) const PER_TASK: usize = 2048;

/// How the new vertices of a level were placed, and so what surface the
/// level's detail is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    /// The scheme's own rules: every vertex moves toward the smooth surface.
    Smooth,
    /// Faces are only cut up. Kept vertices stay exactly where they are and
    /// new ones go to the middle of their edge or face.
    Flat,
}

impl Split {
    /// A byte to write to a file.
    pub fn to_byte(self) -> u8 {
        match self {
            Split::Smooth => 0,
            Split::Flat => 1,
        }
    }

    pub fn from_byte(byte: u8) -> Option<Split> {
        match byte {
            0 => Some(Split::Smooth),
            1 => Some(Split::Flat),
            _ => None,
        }
    }
}

/// A value kept per vertex that can follow a mesh from level to level: a
/// mask, a layer's offsets.
pub trait Channel: Copy + Send + Sync {
    /// The mean of `values` at the vertices `of`, summed in double precision.
    fn mean(values: &[Self], of: &[u32]) -> Self;

    /// This value moved by the step from `from` to `to`.
    fn shifted(self, from: Self, to: Self) -> Self;

    /// Bit for bit the same. (`==` would call a not-a-number different from
    /// itself and so "edited" on every step.)
    fn same(self, other: Self) -> bool;
}

impl Channel for f32 {
    fn mean(values: &[f32], of: &[u32]) -> f32 {
        let sum: f64 = of.iter().map(|&v| values[v as usize] as f64).sum();
        (sum / of.len() as f64) as f32
    }

    fn shifted(self, from: f32, to: f32) -> f32 {
        (self as f64 + (to as f64 - from as f64)) as f32
    }

    fn same(self, other: f32) -> bool {
        self.to_bits() == other.to_bits()
    }
}

impl Channel for Vec3 {
    fn mean(values: &[Vec3], of: &[u32]) -> Vec3 {
        let sum: DVec3 = of.iter().map(|&v| values[v as usize].as_dvec3()).sum();
        (sum / of.len() as f64).as_vec3()
    }

    fn shifted(self, from: Vec3, to: Vec3) -> Vec3 {
        (self.as_dvec3() + (to.as_dvec3() - from.as_dvec3())).as_vec3()
    }

    fn same(self, other: Vec3) -> bool {
        self.x.same(other.x) && self.y.same(other.y) && self.z.same(other.z)
    }
}

/// One level made from the one below, a vertex at a time. The first `kept()`
/// vertices of the finer level are the coarser level's, under the same
/// numbers; the rest are new.
pub(crate) trait Refiner: Sync {
    /// Vertices of the coarser level.
    fn kept(&self) -> usize;

    /// Vertices of the finer level.
    fn total(&self) -> usize;

    /// The kept vertices a new vertex lies between.
    fn between(&self, new: usize) -> &[u32];

    /// Where fine vertex `vertex` sits under the smooth rule, given the
    /// coarser level's positions.
    fn smooth(&self, coarse: &[Vec3], vertex: usize) -> Vec3;

    /// Calls `wedge(a, b)` for every corner of a fine face at fine vertex
    /// `vertex`, `a` and `b` being its neighbours there with `(vertex, a, b)`
    /// wound as the face is. Whenever `u` is named in `v`'s fan, `v` is named
    /// in `u`'s: a step up that starts from what moved relies on it.
    fn fan(&self, vertex: usize, wedge: impl FnMut(u32, u32));

    /// Calls `touched(v)` for fine vertices whose `smooth` or `between`
    /// reads coarse vertex `coarse`: every one that does, perhaps some that do
    /// not, perhaps some twice. Not necessarily `coarse` itself. This is how
    /// a step up that starts from the coarse vertices an edit changed finds
    /// the fine vertices it has to look at (`crate::dirty`).
    fn touched_by(&self, coarse: u32, touched: impl FnMut(u32));

    /// The finer level's faces.
    fn fine_faces(&self) -> Faces;

    /// For every triangle of the finer level, the coarser triangle it lies in.
    fn parent_triangles(&self) -> Vec<u32>;
}

/// The finer level's positions.
pub(crate) fn place<R: Refiner>(r: &R, split: Split, coarse: &[Vec3]) -> Vec<Vec3> {
    (0..r.total()).into_par_iter().with_min_len(PER_TASK).map(|i| place_one(r, split, coarse, i)).collect()
}

/// Fine vertex `i` of [`place`].
#[inline]
pub(crate) fn place_one<R: Refiner>(r: &R, split: Split, coarse: &[Vec3], i: usize) -> Vec3 {
    match split {
        Split::Smooth => r.smooth(coarse, i),
        Split::Flat if i < r.kept() => coarse[i],
        Split::Flat => Vec3::mean(coarse, r.between(i)),
    }
}

/// A channel carried up one level: kept vertices keep their values and new
/// ones take the mean of the vertices they lie between. Nothing is smoothed,
/// so a mask's edge stays where it was painted and a value never leaves the
/// range of the values around it.
pub(crate) fn raise<R: Refiner, T: Channel>(r: &R, coarse: &[T]) -> Vec<T> {
    let kept = r.kept();
    (0..r.total())
        .into_par_iter()
        .with_min_len(PER_TASK)
        .map(|i| if i < kept { coarse[i] } else { T::mean(coarse, r.between(i)) })
        .collect()
}

/// A channel carried up one level over the values the finer level already
/// has. `was` and `now` are the coarser level before and after an edit and
/// `fine` the finer level as it was. A new vertex none of whose neighbours
/// changed keeps its value to the bit; one that was reached moves by as much
/// as [`raise`] would have moved it, so what was painted at the finer level is
/// still there on top. `settle` puts a moved value back in range.
pub(crate) fn raise_over<R: Refiner, T: Channel>(
    r: &R,
    was: &[T],
    now: &[T],
    fine: &[T],
    settle: impl Fn(T) -> T + Sync,
) -> Vec<T> {
    (0..r.total()).into_par_iter().with_min_len(PER_TASK).map(|i| raise_over_one(r, was, now, fine, &settle, i)).collect()
}

/// Fine vertex `i` of [`raise_over`].
#[inline]
pub(crate) fn raise_over_one<R: Refiner, T: Channel>(
    r: &R,
    was: &[T],
    now: &[T],
    fine: &[T],
    settle: &impl Fn(T) -> T,
    i: usize,
) -> T {
    if i < r.kept() {
        return now[i];
    }
    let between = r.between(i);
    if between.iter().all(|&v| was[v as usize].same(now[v as usize])) {
        return fine[i];
    }
    settle(fine[i].shifted(T::mean(was, between), T::mean(now, between)))
}

/// The crease edges met at one vertex, counted as they are found.
#[derive(Clone, Copy)]
pub(crate) struct Creases {
    count: u8,
    ends: [u32; 2],
}

impl Creases {
    pub(crate) const NONE: Creases = Creases { count: 0, ends: [SMOOTH; 2] };

    /// A crease edge leaves this vertex for `other`.
    pub(crate) fn note(&mut self, other: u32) {
        if self.count < 2 {
            self.ends[self.count as usize] = other;
        }
        self.count = self.count.saturating_add(1);
    }

    /// How the vertex moves under the smooth rule, given how many edges it
    /// has in all.
    ///
    /// * No edges: a stray vertex, which has nothing to be smoothed against.
    /// * Two crease edges: it is on a rim (or on the line where sheets meet)
    ///   and moves along that line only, as a point of the curve through its
    ///   two crease neighbours. This is what keeps a hole's rim from being
    ///   pulled into the surface.
    /// * Three or more: several rims or sheets meet here. There is no one
    ///   curve to slide along, so it stays put.
    /// * None, or one (a crease that ends here): the interior rule.
    pub(crate) fn rule(self, edges: usize) -> [u32; 2] {
        match self.count {
            _ if edges == 0 => [FIXED; 2],
            2 => self.ends,
            0 | 1 => [SMOOTH; 2],
            _ => [FIXED; 2],
        }
    }
}

/// A vertex on a crease between `a` and `b`: a point of the cubic curve
/// through the crease, which is what both schemes use along a boundary.
#[inline]
pub(crate) fn crease_point(p: &[Vec3], vertex: usize, a: u32, b: u32) -> Vec3 {
    let ends = p[a as usize].as_dvec3() + p[b as usize].as_dvec3();
    (p[vertex].as_dvec3() * 0.75 + ends * 0.125).as_vec3()
}

/// Rows of `u32`s in one flat array, built on one thread in the order the
/// pairs are given, so a row's order is the caller's.
#[derive(Clone, Default)]
pub(crate) struct Rows {
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl Rows {
    /// `pairs` is run twice over the same `(row, item)` pairs: once to size
    /// the rows and once to fill them.
    pub(crate) fn build(rows: usize, pairs: impl Fn(&mut dyn FnMut(u32, u32))) -> Rows {
        let mut offsets = vec![0u32; rows + 1];
        pairs(&mut |row, _| offsets[row as usize + 1] += 1);
        for i in 0..rows {
            offsets[i + 1] += offsets[i];
        }
        let mut cursor = offsets.clone();
        let mut items = vec![0u32; offsets[rows] as usize];
        pairs(&mut |row, item| {
            let at = &mut cursor[row as usize];
            items[*at as usize] = item;
            *at += 1;
        });
        Rows { offsets, items }
    }

    #[inline]
    pub(crate) fn row(&self, i: usize) -> &[u32] {
        &self.items[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    pub(crate) fn bytes(&self) -> usize {
        (self.offsets.len() + self.items.len()) * 4
    }
}
