//! Lattice: free-form deformation (Sederberg & Parry 1986). A cage of
//! control points, 2 to 6 along each of the gizmo's axes, is laid evenly over
//! the range: the range along the deformer's axis and the whole box across
//! it. A point inside the cage has coordinates `(a, b, c)` from 0 to 1 along
//! the cage's three sides, and goes to the Bernstein-weighted sum of the
//! control points:
//!
//! `X(a, b, c) = Σᵢ Σⱼ Σₖ Bᵢ(a) Bⱼ(b) Bₖ(c) Pᵢⱼₖ`
//!
//! With the points where the even grid puts them that sum is the point
//! itself (Bernstein polynomials reproduce straight lines), so the sum is
//! worked out as the point plus the same weights times how far each control
//! point has been dragged. Only the dragged points take part: an untouched
//! cage moves nothing, exactly, and a cage with one point dragged costs one
//! weight a vertex, not the whole cage's 216.
//!
//! Outside the cage nothing moves. The cage takes in the whole box across
//! the axis; along it, a range narrower than the box leaves the rest of the
//! shape alone, and the two parts part company where they meet if the end
//! points of the cage are dragged (as in every free-form deformation tool).
//!
//! **Under radial symmetry** (`Cage::repeated`) the cage stays the box it
//! is, and what its dragged points do is repeated instead: each copy of the
//! symmetry carries the whole of it, the clay and the way it moves, round
//! the axis (and across the planes). A turn of the box is not the box, so a
//! copy of a dragged point is seldom a point of the cage, and moving the
//! nearest points instead would leave the copies a little unlike each other.
//! Carried whole, every copy is the drag itself, seen from another place: a
//! model that was symmetric before is symmetric after, to rounding. The
//! deformation is then the point plus the sum, over the cage and its
//! copies, of each one's move, worked out where that copy sees the point.

use glam::{DAffine3, DMat3, DVec3};

use super::Bounds;

/// The fewest and most control points along a side.
pub const MIN_POINTS: usize = 2;
pub const MAX_POINTS: usize = 6;

/// The cage's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Lattice {
    /// Control points along the gizmo's X, Y and Z.
    pub count: [usize; 3],
    /// How far each point has been dragged from its place in the even grid,
    /// along the gizmo's axes, as fractions of the cage's size along each.
    /// X counts fastest, then Y, then Z (`index`).
    pub moved: Vec<[f32; 3]>,
    /// The point the card's number boxes edit.
    pub picked: usize,
}

impl Default for Lattice {
    fn default() -> Self {
        let count = [3; 3];
        Lattice { count, moved: vec![[0.0; 3]; 27], picked: 0 }
    }
}

impl Lattice {
    /// The number of control points.
    pub fn len(&self) -> usize {
        self.count.iter().product()
    }

    /// The point `[i, j, k]` along X, Y and Z as a place in `moved`.
    pub fn index(&self, ijk: [usize; 3]) -> usize {
        ijk[0] + self.count[0] * (ijk[1] + self.count[1] * ijk[2])
    }

    /// The other way about.
    pub fn ijk(&self, index: usize) -> [usize; 3] {
        let [nx, ny, _] = self.count;
        [index % nx, (index / nx) % ny, index / (nx * ny)]
    }

    /// Whether every point is where the even grid puts it.
    pub fn is_rest(&self) -> bool {
        self.moved.iter().all(|m| *m == [0.0; 3])
    }

    /// Puts every point back in the even grid.
    pub fn rest(&mut self) {
        self.moved.iter_mut().for_each(|m| *m = [0.0; 3]);
    }

    /// The same cage with this many points along each side, every point
    /// back in the grid: a dragged point has no place in a grid of another size.
    pub fn with_count(&self, count: [usize; 3]) -> Lattice {
        let count = count.map(|n| n.clamp(MIN_POINTS, MAX_POINTS));
        if count == self.count {
            return self.clone();
        }
        Lattice { count, moved: vec![[0.0; 3]; count.iter().product()], picked: 0 }
    }

    pub fn sanitized(&self) -> Lattice {
        let mut l = self.with_count(self.count);
        if l.moved.len() != l.len() {
            l.moved = vec![[0.0; 3]; l.len()];
        }
        for m in &mut l.moved {
            *m = m.map(|v| if v.is_finite() { v.clamp(-10.0, 10.0) } else { 0.0 });
        }
        l.picked = l.picked.min(l.len() - 1);
        l
    }
}

/// The cage's box in the gizmo's frame: the range along `axis`, the whole of
/// `bounds` across it (widened a hair, so a flat model still has a cage
/// with some thickness).
pub fn cage_box(bounds: &Bounds, axis: usize, (lo, hi): (f64, f64)) -> (DVec3, DVec3) {
    let pad = bounds.longest().max(1e-9) * 1e-6;
    let (mut min, mut max) = (bounds.min - DVec3::splat(pad), bounds.max + DVec3::splat(pad));
    min[axis] = lo;
    max[axis] = hi;
    (min, max - min)
}

/// Where point `index` of the cage is, in the gizmo's frame.
pub fn point(lattice: &Lattice, index: usize, bounds: &Bounds, axis: usize, span: (f64, f64)) -> DVec3 {
    let (min, size) = cage_box(bounds, axis, span);
    let ijk = lattice.ijk(index);
    let grid = DVec3::from_array(std::array::from_fn(|c| ijk[c] as f64 / (lattice.count[c] - 1) as f64));
    let moved = DVec3::from_array(lattice.moved.get(index).copied().unwrap_or_default().map(f64::from));
    min + (grid + moved) * size
}

/// The fraction of the cage's size along each axis that puts point `index`
/// at `at` (in the gizmo's frame): what a drag of it sets.
pub fn moved_for(
    lattice: &Lattice,
    index: usize,
    bounds: &Bounds,
    axis: usize,
    span: (f64, f64),
    at: DVec3,
) -> [f32; 3] {
    let (min, size) = cage_box(bounds, axis, span);
    let ijk = lattice.ijk(index);
    let grid = DVec3::from_array(std::array::from_fn(|c| ijk[c] as f64 / (lattice.count[c] - 1) as f64));
    ((at - min) / size.max(DVec3::splat(1e-12)) - grid).as_vec3().to_array()
}

/// The binomial coefficients the Bernstein polynomials are made of.
const CHOOSE: [[f64; MAX_POINTS]; MAX_POINTS] = [
    [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
    [1.0, 2.0, 1.0, 0.0, 0.0, 0.0],
    [1.0, 3.0, 3.0, 1.0, 0.0, 0.0],
    [1.0, 4.0, 6.0, 4.0, 1.0, 0.0],
    [1.0, 5.0, 10.0, 10.0, 5.0, 1.0],
];

/// `Bᵢ(x)` for `i` in `0..n`: the Bernstein polynomials of degree `n - 1`.
#[inline]
fn bernstein(n: usize, x: f64, out: &mut [f64; MAX_POINTS]) {
    let degree = n - 1;
    let y = 1.0 - x;
    for i in 0..n {
        out[i] = CHOOSE[degree][i] * x.powi(i as i32) * y.powi((degree - i) as i32);
    }
}

/// `Bᵢ(x)` alone, worked out as `bernstein` works it, to the bit.
#[inline]
fn bernstein_one(n: usize, i: usize, x: f64) -> f64 {
    let degree = n - 1;
    CHOOSE[degree][i] * x.powi(i as i32) * (1.0 - x).powi((degree - i) as i32)
}

/// The Bernstein weights along each side at `at` (fractions of the cage).
#[inline]
fn weights_at(count: [usize; 3], at: DVec3) -> [[f64; MAX_POINTS]; 3] {
    let mut weights = [[0.0f64; MAX_POINTS]; 3];
    for c in 0..3 {
        bernstein(count[c], at[c], &mut weights[c]);
    }
    weights
}

/// How far outside its sides, as a share of its size, a cage with copies
/// still takes a point in, as on the side. A copy carries a point by a turn
/// worked out in floats, and a mesh is kept in floats: the image of a vertex
/// on the side of the box can come out a hair outside it, and would then
/// miss that copy's move, which its twin got. A millionth is a hundred times
/// what a float rounds off, and the side of a cage is outside the model
/// across the axis by as much (`cage_box`).
const SLACK: f64 = 1e-6;

/// One copy of the cage's moves under symmetry (`Cage::repeated`).
struct Image {
    /// From the frame to where the cage sees a point this copy reaches.
    back: DAffine3,
    /// What the copy does to a way of moving.
    linear: DMat3,
}

/// The cage set up for one setting: the dragged points alone.
pub struct Cage {
    min: DVec3,
    size: DVec3,
    count: [usize; 3],
    /// Each dragged point: its place in the grid and how far it went, in the frame's units.
    moved: Vec<([usize; 3], DVec3)>,
    /// Every copy of the symmetry but the cage itself; none without one.
    images: Vec<Image>,
    /// With copies, the stretch of the deformer's axis that the cage and
    /// its copies reach between them.
    reach: Option<(f64, f64)>,
}

impl Cage {
    /// `None` if no point has been dragged.
    pub fn new(lattice: &Lattice, bounds: &Bounds, axis: usize, span: (f64, f64)) -> Option<Cage> {
        let (min, size) = cage_box(bounds, axis, span);
        let moved: Vec<([usize; 3], DVec3)> = (0..lattice.len())
            .filter(|&i| lattice.moved[i] != [0.0; 3])
            .map(|i| (lattice.ijk(i), DVec3::from_array(lattice.moved[i].map(f64::from)) * size))
            .collect();
        (!moved.is_empty()).then_some(Cage { min, size, count: lattice.count, moved, images: Vec::new(), reach: None })
    }

    /// The same cage with what its dragged points do repeated by every one
    /// of `copies`: the symmetry's copies other than the cage itself, as
    /// maps of the gizmo's frame. `axis` is the deformer's. See the top of
    /// this file.
    ///
    /// **As strong as one drag.** A point's copies can crowd in on it: round
    /// the axis, a point's neighbours in a ring of thirty-two are close
    /// enough for their moves to add up to six times its own, and a point
    /// on the axis is its own copy every time. Copies on one spot are one
    /// drag, and copies near each other are not a stronger one (the rule a
    /// brush's dabs keep, sculpt/share.rs): if the copies between them would
    /// move the clay at a point's own place further along its way than the
    /// point moves it alone, every copy of that point is scaled down by the
    /// same share, until they do not. One share for all the copies of a
    /// point, so they stay copies of each other. A point clear of its copies
    /// keeps its whole move, so the corners of the cage go where they are
    /// dragged as they always have.
    pub fn repeated(mut self, copies: &[DAffine3], axis: usize) -> Cage {
        if copies.is_empty() {
            return self;
        }
        let images: Vec<Image> = copies.iter().map(|c| Image { back: c.inverse(), linear: c.matrix3 }).collect();
        for k in 0..self.moved.len() {
            let (ijk, by) = self.moved[k];
            let grid = DVec3::from_array(std::array::from_fn(|c| ijk[c] as f64 / (self.count[c] - 1) as f64));
            let rest = self.min + grid * self.size;
            // Its own weight at its own place, from the grid itself: a point
            // on the side of the box must not be found a hair outside it.
            let w = weights_at(self.count, grid);
            let own = w[0][ijk[0]] * w[1][ijk[1]] * w[2][ijk[2]];
            let together = images.iter().fold(by * own, |sum, image| {
                let weight = self.near(image.back.transform_point3(rest)).map_or(0.0, |at| {
                    (0..3).map(|c| bernstein_one(self.count[c], ijk[c], at[c])).product::<f64>()
                });
                sum + image.linear * by * weight
            });
            let (alone, along) = (own * by.length_squared(), by.dot(together));
            if along > alone {
                self.moved[k].1 = by * (alone / along);
            }
        }
        // Where the copies of the box reach along the axis: its corners carried.
        let corners = (0..8).map(|c| self.min + DVec3::new((c & 1) as f64, (c >> 1 & 1) as f64, (c >> 2) as f64) * self.size);
        let (mut lo, mut hi) = (self.min[axis], self.min[axis] + self.size[axis]);
        for copy in copies {
            for corner in corners.clone() {
                let x = copy.transform_point3(corner)[axis];
                (lo, hi) = (lo.min(x), hi.max(x));
            }
        }
        // What `near` takes in past the sides, and a hair for the rounding.
        let pad = self.size.max_element() * (2.0 * SLACK);
        self.reach = Some((lo - pad, hi + pad));
        self.images = images;
        self
    }

    /// The stretch of the axis the cage's copies reach, with copies.
    pub fn reach(&self) -> Option<(f64, f64)> {
        self.reach
    }

    #[inline]
    pub fn apply(&self, q: DVec3) -> Option<DVec3> {
        if self.images.is_empty() {
            let at = (q - self.min) / self.size;
            if !(at.cmpge(DVec3::ZERO).all() && at.cmple(DVec3::ONE).all()) {
                return None;
            }
            return Some(q + self.shift(&weights_at(self.count, at)));
        }
        // The cage's own move, then each copy's, as that copy carries it.
        let mut total = self.near(q).map(|at| self.shift_near(at));
        for image in &self.images {
            if let Some(at) = self.near(image.back.transform_point3(q)) {
                let shift = image.linear * self.shift_near(at);
                total = Some(total.map_or(shift, |sum| sum + shift));
            }
        }
        total.map(|shift| q + shift)
    }

    /// How far the dragged points move a point whose weights are `weights`.
    #[inline]
    fn shift(&self, weights: &[[f64; MAX_POINTS]; 3]) -> DVec3 {
        self.moved
            .iter()
            .fold(DVec3::ZERO, |sum, &([i, j, k], by)| sum + by * (weights[0][i] * weights[1][j] * weights[2][k]))
    }

    /// `shift` at `at` (fractions of the cage), for a cage with copies,
    /// which works it out once a copy. With a few points dragged, only
    /// their own weights are worked out, not every point's along each side:
    /// the same numbers, a third of the work for one point of a cage of
    /// three a side.
    #[inline]
    fn shift_near(&self, at: DVec3) -> DVec3 {
        if self.moved.len() * 3 >= self.count.iter().sum::<usize>() {
            return self.shift(&weights_at(self.count, at));
        }
        let [nx, ny, nz] = self.count;
        self.moved.iter().fold(DVec3::ZERO, |sum, &([i, j, k], by)| {
            sum + by * (bernstein_one(nx, i, at.x) * bernstein_one(ny, j, at.y) * bernstein_one(nz, k, at.z))
        })
    }

    /// Where `q` is in the cage, as fractions of its sides, for a cage with
    /// copies, which takes in a point a hair outside its sides as on them
    /// (`SLACK`); `None` further out.
    #[inline]
    fn near(&self, q: DVec3) -> Option<DVec3> {
        let at = (q - self.min) / self.size;
        if !(at.cmpge(DVec3::splat(-SLACK)).all() && at.cmple(DVec3::splat(1.0 + SLACK)).all()) {
            return None;
        }
        Some(at.clamp(DVec3::ZERO, DVec3::ONE))
    }

    /// The textbook form, every control point in the sum: for the test that
    /// the even grid is the identity.
    #[cfg(test)]
    pub fn full_sum(lattice: &Lattice, bounds: &Bounds, axis: usize, span: (f64, f64), q: DVec3) -> DVec3 {
        let (min, size) = cage_box(bounds, axis, span);
        let at = (q - min) / size;
        let mut weights = [[0.0f64; MAX_POINTS]; 3];
        for c in 0..3 {
            bernstein(lattice.count[c], at[c], &mut weights[c]);
        }
        (0..lattice.len()).fold(DVec3::ZERO, |sum, index| {
            let [i, j, k] = lattice.ijk(index);
            sum + point(lattice, index, bounds, axis, span) * (weights[0][i] * weights[1][j] * weights[2][k])
        })
    }
}
