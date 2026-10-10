//! A joint's cut faces rasterised onto a grid in the joint's own plane, with,
//! for every cell, how much material lies behind the face on each side.
//! Everything automatic placement and the tool's live "does it fit here"
//! check need, at array-lookup speed.
//!
//! DEPTH BY COLUMNS. How deep a hole may go at a point is how far a straight
//! line from the face, along the axis, travels inside the part before it
//! meets another surface. Rather than cast a ray per question, every triangle
//! of the part is rasterised ONCE into the grid and each column keeps the
//! nearest crossing behind the face. A hole that stays a minimum wall away
//! from every surface in every column under it, and whose footprint stays a
//! minimum wall inside the face, can break through nowhere: not out of the
//! far side, and not out of the sides of a part that tapers away behind its
//! cut.

use glam::{Vec2, Vec3};
use rayon::prelude::*;
use sculpt_cut::polygon;

use crate::joint::{KeyJoint, KeyPart};

/// Cells across the longer side of the face. About a fifth of a millimetre on
/// a 50 mm face; a key's fit is then judged against its true outline to
/// within a cell.
const RESOLUTION: usize = 256;

/// Most a face may lean off the joint axis and still carry a key.
pub const MAX_FACE_TILT_DEGREES: f32 = 25.0;

/// What `JointMap::fits` says when a mouth outline is too near something.
pub const TOO_CLOSE: &str = "too close to the edge of the cut, or not enough material behind it";
/// What it says when a cell under the mouth has too little behind it.
pub const NOT_ENOUGH: &str = "not enough material behind the cut face here";

/// The grid itself: where it lies in the joint's plane.
#[derive(Clone, Copy, Debug)]
struct Grid {
    min: Vec2,
    cell: f32,
    width: usize,
    height: usize,
}

impl Grid {
    #[inline]
    fn centre(&self, x: usize, y: usize) -> Vec2 {
        self.min + Vec2::new((x as f32 + 0.5) * self.cell, (y as f32 + 0.5) * self.cell)
    }

    /// The cell a plane point falls in, which may be off the grid.
    #[inline]
    fn cell_at(&self, p: Vec2) -> (i32, i32) {
        (((p.x - self.min.x) / self.cell).floor() as i32, ((p.y - self.min.y) / self.cell).floor() as i32)
    }

    /// Calls `write(cell, height)` for every cell whose centre the triangle
    /// covers, seen along the axis. Corners are (x, y) in the plane with the
    /// height along the axis in z.
    fn raster(&self, p0: Vec3, p1: Vec3, p2: Vec3, mut write: impl FnMut(usize, f32)) {
        let lo = p0.min(p1).min(p2);
        let hi = p0.max(p1).max(p2);
        let first = |v: f32, origin: f32| ((v - origin) / self.cell - 0.5).ceil().max(0.0) as i64;
        let last = |v: f32, origin: f32, count: usize| {
            (((v - origin) / self.cell - 0.5).floor() as i64).min(count as i64 - 1)
        };
        let (cx0, cx1) = (first(lo.x, self.min.x), last(hi.x, self.min.x, self.width));
        let (cy0, cy1) = (first(lo.y, self.min.y), last(hi.y, self.min.y, self.height));
        if cx0 > cx1 || cy0 > cy1 {
            return;
        }
        let (a, b, c) = (p0.truncate(), p1.truncate(), p2.truncate());
        let det = polygon::cross(a, b, c);
        // Edge-on to the axis: no column crosses it.
        if det.abs() < 1e-24 {
            return;
        }
        for y in cy0 as usize..=cy1 as usize {
            for x in cx0 as usize..=cx1 as usize {
                let p = self.centre(x, y);
                let wa = polygon::cross(b, c, p) / det;
                let wb = polygon::cross(c, a, p) / det;
                let wc = 1.0 - wa - wb;
                if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                    continue;
                }
                write(y * self.width + x, (wa * p0.z as f64 + wb * p1.z as f64 + wc * p2.z as f64) as f32);
            }
        }
    }
}

/// One side's cut faces as the grid sees them.
struct FaceRaster {
    cover: Vec<u8>,
    surface: Vec<f32>,
    face: Vec<i32>,
}

/// A joint's faces and the depth behind them, on a grid; see the module
/// remarks.
#[derive(Clone, Debug)]
pub struct JointMap {
    grid: Grid,
    /// Per cell: the face's height along the axis (on A's side), and which of
    /// each part's cut faces covers it (-1 none).
    surface: Vec<f32>,
    face_a: Vec<i32>,
    face_b: Vec<i32>,
    eligible: Vec<bool>,
    /// Free distance behind the face, into A (against the axis) and into B
    /// (along it).
    depth_a: Vec<f32>,
    depth_b: Vec<f32>,
    eligible_cells: usize,
    folded_fraction: f32,
    open_columns_a: usize,
    open_columns_b: usize,
}

impl JointMap {
    pub fn new(a: &KeyPart, b: &KeyPart, joint: &KeyJoint) -> JointMap {
        let (mut min, mut max) = (Vec2::MAX, Vec2::MIN);
        for &f in &joint.faces_a {
            for &t in &a.cut_faces[f] {
                for corner in a.corners(t) {
                    let p = joint.to_plane(corner);
                    min = min.min(p);
                    max = max.max(p);
                }
            }
        }
        if min.x > max.x {
            (min, max) = (Vec2::ZERO, Vec2::ZERO);
        }
        let extent = (max - min).max_element().max(1e-6);
        let cell = extent / RESOLUTION as f32;
        // Two empty cells round the edge, so "outside the grid" is always
        // "not the face".
        let grid = Grid {
            min: min - Vec2::splat(cell * 2.0),
            cell,
            width: ((max.x - min.x) / cell).ceil() as usize + 5,
            height: ((max.y - min.y) / cell).ceil() as usize + 5,
        };
        let n = grid.width * grid.height;

        let cos_tilt = MAX_FACE_TILT_DEGREES.to_radians().cos();
        let mut tilt_ok = vec![true; n];
        let mut on_face_a = vec![false; a.triangle_count()];
        let mut on_face_b = vec![false; b.triangle_count()];
        let side_a = raster_faces(&grid, a, joint, &joint.faces_a, 1.0, cos_tilt, &mut tilt_ok, &mut on_face_a);
        let side_b = raster_faces(&grid, b, joint, &joint.faces_b, -1.0, cos_tilt, &mut tilt_ok, &mut on_face_b);

        // The two faces are two sides of one cut; they coincide to within
        // their own triangulations. Only where both are present, once each,
        // and flat enough, is a cell somewhere a key can go.
        let coincide = cell.max(joint.edge) * 0.5;
        let eligible: Vec<bool> = (0..n)
            .map(|i| {
                side_a.cover[i] == 1
                    && side_b.cover[i] == 1
                    && tilt_ok[i]
                    && (side_a.surface[i] - side_b.surface[i]).abs() <= coincide
            })
            .collect();
        let covered = side_a.cover.iter().filter(|&&c| c > 0).count();
        let folded = side_a.cover.iter().filter(|&&c| c > 1).count();

        let (mut depth_a, mut depth_b) = rayon::join(
            || raster_depth(&grid, a, joint, &on_face_a, -1.0, &side_a.surface),
            || raster_depth(&grid, b, joint, &on_face_b, 1.0, &side_a.surface),
        );

        // A column that meets NO surface behind the face runs out through a
        // hole in the mesh. That is not infinite depth, it is no depth at all:
        // a socket there would open into the gap. (On a closed solid every
        // column comes back out somewhere.)
        let (mut open_columns_a, mut open_columns_b) = (0, 0);
        for i in (0..n).filter(|&i| eligible[i]) {
            if depth_a[i] == f32::MAX {
                depth_a[i] = 0.0;
                open_columns_a += 1;
            }
            if depth_b[i] == f32::MAX {
                depth_b[i] = 0.0;
                open_columns_b += 1;
            }
        }

        JointMap {
            grid,
            eligible_cells: eligible.iter().filter(|&&e| e).count(),
            folded_fraction: if covered > 0 { folded as f32 / covered as f32 } else { 0.0 },
            surface: side_a.surface,
            face_a: side_a.face,
            face_b: side_b.face,
            eligible,
            depth_a,
            depth_b,
            open_columns_a,
            open_columns_b,
        }
    }

    pub fn width(&self) -> usize {
        self.grid.width
    }

    pub fn height(&self) -> usize {
        self.grid.height
    }

    /// Cells in the grid.
    pub fn len(&self) -> usize {
        self.grid.width * self.grid.height
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The side of one cell, in world units.
    pub fn cell(&self) -> f32 {
        self.grid.cell
    }

    /// Cells where both faces are present, once each, and flat enough.
    pub fn eligible_cells(&self) -> usize {
        self.eligible_cells
    }

    /// Share of the face's cells that it covers more than once, seen along
    /// the axis: where the cut face folds back over itself. The halves cannot
    /// then be pulled apart along the axis at all (a peg anywhere would lock
    /// them), so a joint with a real fold gets no keys.
    pub fn folded_fraction(&self) -> f32 {
        self.folded_fraction
    }

    /// Eligible cells whose column behind the face (into A / into B) met no
    /// surface: the part is not closed there.
    pub fn open_columns(&self, into_b: bool) -> usize {
        if into_b { self.open_columns_b } else { self.open_columns_a }
    }

    /// Usable face area, in world units squared.
    pub fn area(&self) -> f32 {
        self.eligible_cells as f32 * self.grid.cell * self.grid.cell
    }

    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        y * self.grid.width + x
    }

    /// The plane position of the middle of cell `i`.
    #[inline]
    pub fn cell_centre(&self, i: usize) -> Vec2 {
        self.grid.centre(i % self.grid.width, i / self.grid.width)
    }

    /// The cell a plane point falls in, if it is on the grid.
    #[inline]
    pub fn cell_of(&self, p: Vec2) -> Option<usize> {
        let (x, y) = self.grid.cell_at(p);
        let on = x >= 0 && y >= 0 && (x as usize) < self.grid.width && (y as usize) < self.grid.height;
        on.then(|| self.index(x as usize, y as usize))
    }

    #[inline]
    pub fn eligible(&self, i: usize) -> bool {
        self.eligible[i]
    }

    /// Free depth behind the face at cell `i`, into the part that would take
    /// the hole.
    #[inline]
    pub fn depth(&self, i: usize, hole_in_b: bool) -> f32 {
        if hole_in_b { self.depth_b[i] } else { self.depth_a[i] }
    }

    /// Which of A's cut faces covers cell `i`.
    pub fn face_a(&self, i: usize) -> Option<usize> {
        usize::try_from(self.face_a[i]).ok()
    }

    pub fn face_b(&self, i: usize) -> Option<usize> {
        usize::try_from(self.face_b[i]).ok()
    }

    /// Whether a cut face of A covers plane point `p`.
    pub fn on_face(&self, p: Vec2) -> bool {
        self.cell_of(p).is_some_and(|i| self.face_a[i] >= 0)
    }

    /// The face's height along the axis above plane point `p` (nearest
    /// cell's), zero off the face.
    pub fn surface_height(&self, p: Vec2) -> f32 {
        self.cell_of(p).filter(|&i| self.face_a[i] >= 0).map_or(0.0, |i| self.surface[i])
    }

    /// The inclusive range of cells a plane box touches, clipped to the grid.
    fn cells_in(&self, lo: Vec2, hi: Vec2) -> (std::ops::RangeInclusive<usize>, std::ops::RangeInclusive<usize>) {
        let ((x0, y0), (x1, y1)) = (self.grid.cell_at(lo), self.grid.cell_at(hi));
        let clip = |v: i32, count: usize| v.clamp(0, count as i32 - 1) as usize;
        // An empty range when the box misses the grid altogether.
        let span = |a: i32, b: i32, count: usize| {
            if b < 0 || a >= count as i32 { 1..=0 } else { clip(a, count)..=clip(b, count) }
        };
        (span(x0, x1, self.grid.width), span(y0, y1, self.grid.height))
    }

    // ------------------------------------------------------------ fit questions

    /// Distance (world units) from each cell centre to the nearest cell that
    /// is NOT usable for a hole `need` deep in the given part: off the face,
    /// too steep, or with less than `need` of material behind it. Exact
    /// Euclidean (Felzenszwalb-Huttenlocher), in cells.
    pub fn clearance(&self, hole_in_b: bool, need: f32) -> Vec<f32> {
        const INFINITE: f32 = 1e20;
        let mut f: Vec<f32> = (0..self.len())
            .map(|i| if self.eligible[i] && self.depth(i, hole_in_b) >= need { INFINITE } else { 0.0 })
            .collect();
        distance_transform(&mut f, self.grid.width, self.grid.height);
        for v in &mut f {
            *v = v.sqrt() * self.grid.cell;
        }
        f
    }

    /// Whether a key's HOLE MOUTH outline (plane coordinates) fits: every
    /// cell inside it is usable, and every point of it is at least `wall`
    /// from anything that is not. `clear` comes from `clearance` for the same
    /// hole depth.
    pub fn fits(&self, mouth: &[Vec2], clear: &[f32], wall: f32) -> Result<(), &'static str> {
        // A cell's clearance is measured centre to centre; the outline point
        // can sit up to half a diagonal nearer the obstacle than its cell's
        // centre does.
        let slack = self.grid.cell * 0.71;
        let (mut lo, mut hi) = (Vec2::MAX, Vec2::MIN);
        for &p in mouth {
            lo = lo.min(p);
            hi = hi.max(p);
            if !self.cell_of(p).is_some_and(|i| clear[i] - slack >= wall) {
                return Err(TOO_CLOSE);
            }
        }
        let (xs, ys) = self.cells_in(lo, hi);
        for y in ys {
            for x in xs.clone() {
                let i = self.index(x, y);
                if clear[i] <= 0.0 && polygon::contains(mouth, self.grid.centre(x, y)) {
                    return Err(NOT_ENOUGH);
                }
            }
        }
        Ok(())
    }

    /// The smallest free depth anywhere under a mouth outline's cells, for
    /// saying how deep a hole could have gone.
    pub fn min_depth_under(&self, mouth: &[Vec2], hole_in_b: bool) -> f32 {
        let lo = mouth.iter().fold(Vec2::MAX, |m, p| m.min(*p));
        let hi = mouth.iter().fold(Vec2::MIN, |m, p| m.max(*p));
        let mut best = f32::MAX;
        let (xs, ys) = self.cells_in(lo, hi);
        for y in ys {
            for x in xs.clone() {
                if polygon::contains(mouth, self.grid.centre(x, y)) {
                    best = best.min(self.depth(self.index(x, y), hole_in_b));
                }
            }
        }
        if best == f32::MAX { 0.0 } else { best }
    }
}

/// One side's cut faces into the grid. `facing` is +1 for A, whose faces look
/// along the axis, and -1 for B, whose faces look against it.
#[allow(clippy::too_many_arguments)]
fn raster_faces(
    grid: &Grid,
    part: &KeyPart,
    joint: &KeyJoint,
    faces: &[usize],
    facing: f32,
    cos_tilt: f32,
    tilt_ok: &mut [bool],
    on_face: &mut [bool],
) -> FaceRaster {
    let n = grid.width * grid.height;
    let mut side = FaceRaster { cover: vec![0; n], surface: vec![0.0; n], face: vec![-1; n] };
    for &f in faces {
        for &t in &part.cut_faces[f] {
            on_face[t as usize] = true;
            let [p0, p1, p2] = part.corners(t).map(|p| joint.to_local(p));
            let cross = (p1 - p0).cross(p2 - p0);
            let len = cross.length();
            // In plane coordinates the axis is +z. A sliver's normal is noise
            // (see `KeyJoint`); it covers next to no cell anyway.
            let sliver = len <= joint.edge * joint.edge * 2e-3;
            let flat = sliver || (len > 0.0 && cross.z * facing / len >= cos_tilt);
            grid.raster(p0, p1, p2, |i, z| {
                side.cover[i] = side.cover[i].saturating_add(1);
                side.surface[i] = z;
                side.face[i] = f as i32;
                if !flat {
                    tilt_ok[i] = false;
                }
            });
        }
    }
    side
}

/// Every other triangle of the part, into the depth columns: the nearest
/// surface behind the face in the direction `into` (+1 along the axis, -1
/// against it). A whole model goes through here, so it runs across the cores,
/// each with its own columns, merged at the end.
fn raster_depth(grid: &Grid, part: &KeyPart, joint: &KeyJoint, on_face: &[bool], into: f32, surface: &[f32]) -> Vec<f32> {
    let n = grid.width * grid.height;
    // Plane bounds of the grid, to throw away the (usually vast) rest of the
    // model cheaply.
    let lo = grid.min;
    let hi = grid.min + Vec2::new(grid.width as f32, grid.height as f32) * grid.cell;
    let triangles = part.triangle_count();
    let chunk = (triangles / (rayon::current_num_threads() * 4).max(1)).max(4096);
    (0..triangles as u32)
        .into_par_iter()
        .with_min_len(chunk)
        .fold(
            || vec![f32::MAX; n],
            |mut depth, t| {
                if on_face[t as usize] {
                    return depth;
                }
                let [p0, p1, p2] = part.corners(t).map(|p| joint.to_local(p));
                let (tlo, thi) = (p0.min(p1).min(p2).truncate(), p0.max(p1).max(p2).truncate());
                if thi.cmplt(lo).any() || tlo.cmpgt(hi).any() {
                    return depth;
                }
                grid.raster(p0, p1, p2, |i, z| {
                    let d = (z - surface[i]) * into;
                    // Behind the face (a hair of slack for the triangles that
                    // meet it at its rim).
                    if d > -grid.cell * 0.01 && d < depth[i] {
                        depth[i] = d.max(0.0);
                    }
                });
                depth
            },
        )
        .reduce(
            || vec![f32::MAX; n],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(&b) {
                    *x = x.min(*y);
                }
                a
            },
        )
}

/// Squared distance transform in place (values are 0 at sources, "infinite"
/// elsewhere).
fn distance_transform(grid: &mut [f32], w: usize, h: usize) {
    let m = w.max(h);
    let mut f = vec![0.0f32; m];
    let mut d = vec![0.0f32; m];
    let mut v = vec![0usize; m];
    let mut z = vec![0.0f32; m + 1];
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        transform_1d(&f[..h], &mut d, &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..y * w + w]);
        transform_1d(&f[..w], &mut d, &mut v, &mut z);
        grid[y * w..y * w + w].copy_from_slice(&d[..w]);
    }
}

/// Felzenszwalb and Huttenlocher's lower envelope of parabolas. The meeting
/// points are worked out in double: "infinite" sources are 1e20 and single
/// precision would lose the q * q terms against them.
fn transform_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let meet = |q: usize, p: usize| {
        let (fq, fp, q, p) = (f[q] as f64, f[p] as f64, q as f64, p as f64);
        ((fq + q * q - (fp + p * p)) / (2.0 * q - 2.0 * p)) as f32
    };
    let mut k = 0;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let mut s = meet(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = meet(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for q in 0..n {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        let apart = q as f32 - p as f32;
        d[q] = apart * apart + f[p];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_distance_transform_is_exact() {
        // One source in a 9 x 7 grid: every cell's value is its squared
        // distance from it.
        let (w, h) = (9, 7);
        let mut grid = vec![1e20f32; w * h];
        grid[3 * w + 2] = 0.0;
        distance_transform(&mut grid, w, h);
        for y in 0..h {
            for x in 0..w {
                let want = ((x as f32 - 2.0).powi(2) + (y as f32 - 3.0).powi(2)) as f32;
                assert_eq!(grid[y * w + x], want, "cell {x}, {y}");
            }
        }
    }

    #[test]
    fn a_triangle_covers_the_cell_centres_inside_it() {
        let grid = Grid { min: Vec2::ZERO, cell: 1.0, width: 4, height: 4 };
        let mut hit = Vec::new();
        let corners = [Vec3::new(0.0, 0.0, 1.0), Vec3::new(4.0, 0.0, 1.0), Vec3::new(0.0, 4.0, 5.0)];
        grid.raster(corners[0], corners[1], corners[2], |i, z| hit.push((i, z)));
        // The centres below the diagonal x + y = 4, the diagonal included.
        let cells: Vec<usize> = hit.iter().map(|h| h.0).collect();
        assert_eq!(cells, vec![0, 1, 2, 3, 4, 5, 6, 8, 9, 12]);
        // Heights are interpolated: z = 1 + y.
        for (i, z) in hit {
            assert!((z - (1.5 + (i / 4) as f32)).abs() < 1e-5);
        }
        // Either winding, and nothing for a triangle off the grid or edge-on.
        let mut count = 0;
        grid.raster(Vec3::ZERO, Vec3::new(0.0, 4.0, 0.0), Vec3::new(4.0, 0.0, 0.0), |_, _| count += 1);
        assert_eq!(count, 10);
        grid.raster(Vec3::splat(9.0), Vec3::new(12.0, 9.0, 0.0), Vec3::new(9.0, 12.0, 0.0), |_, _| count += 1);
        grid.raster(Vec3::ZERO, Vec3::new(2.0, 2.0, 0.0), Vec3::new(4.0, 4.0, 3.0), |_, _| count += 1);
        assert_eq!(count, 10);
    }
}
