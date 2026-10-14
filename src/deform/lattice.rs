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

use glam::DVec3;

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
pub fn moved_for(lattice: &Lattice, index: usize, bounds: &Bounds, axis: usize, span: (f64, f64), at: DVec3) -> [f32; 3] {
    let (min, size) = cage_box(bounds, axis, span);
    let ijk = lattice.ijk(index);
    let grid = DVec3::from_array(std::array::from_fn(|c| ijk[c] as f64 / (lattice.count[c] - 1) as f64));
    ((at - min) / size.max(DVec3::splat(1e-12)) - grid).as_vec3().to_array()
}

/// `Bᵢ(x)` for `i` in `0..n`: the Bernstein polynomials of degree `n - 1`.
#[inline]
fn bernstein(n: usize, x: f64, out: &mut [f64; MAX_POINTS]) {
    const CHOOSE: [[f64; MAX_POINTS]; MAX_POINTS] = [
        [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1.0, 2.0, 1.0, 0.0, 0.0, 0.0],
        [1.0, 3.0, 3.0, 1.0, 0.0, 0.0],
        [1.0, 4.0, 6.0, 4.0, 1.0, 0.0],
        [1.0, 5.0, 10.0, 10.0, 5.0, 1.0],
    ];
    let degree = n - 1;
    let y = 1.0 - x;
    for i in 0..n {
        out[i] = CHOOSE[degree][i] * x.powi(i as i32) * y.powi((degree - i) as i32);
    }
}

/// The cage set up for one setting: the dragged points alone.
pub struct Cage {
    min: DVec3,
    size: DVec3,
    count: [usize; 3],
    /// Each dragged point: its place in the grid and how far it went, in the frame's units.
    moved: Vec<([usize; 3], DVec3)>,
}

impl Cage {
    /// `None` if no point has been dragged.
    pub fn new(lattice: &Lattice, bounds: &Bounds, axis: usize, span: (f64, f64)) -> Option<Cage> {
        let (min, size) = cage_box(bounds, axis, span);
        let moved: Vec<([usize; 3], DVec3)> = (0..lattice.len())
            .filter(|&i| lattice.moved[i] != [0.0; 3])
            .map(|i| (lattice.ijk(i), DVec3::from_array(lattice.moved[i].map(f64::from)) * size))
            .collect();
        (!moved.is_empty()).then_some(Cage { min, size, count: lattice.count, moved })
    }

    #[inline]
    pub fn apply(&self, q: DVec3) -> Option<DVec3> {
        let at = (q - self.min) / self.size;
        if !(at.cmpge(DVec3::ZERO).all() && at.cmple(DVec3::ONE).all()) {
            return None;
        }
        let mut weights = [[0.0f64; MAX_POINTS]; 3];
        for c in 0..3 {
            bernstein(self.count[c], at[c], &mut weights[c]);
        }
        let shift = self.moved.iter().fold(DVec3::ZERO, |sum, &([i, j, k], by)| {
            sum + by * (weights[0][i] * weights[1][j] * weights[2][k])
        });
        Some(q + shift)
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
