//! Scan conversion of a triangle mesh onto a lattice: for every lattice line
//! along each axis, exactly where the surface crosses it and which way.
//!
//! This is what decides inside from outside, and it is where a mesh-to-volume
//! conversion usually goes wrong. Casting a ray per lattice line and testing
//! it against triangles in floating point lets a ray slip between two
//! triangles along their shared edge (or hit both), and one miscounted
//! crossing turns a whole line of samples inside out. Here every line is
//! instead tested against each triangle with the same three numbers its
//! neighbours use:
//!
//!   - Whether a line passes through a triangle is decided by edge functions
//!     evaluated in double precision with the edge's endpoints in a fixed
//!     order, so the two triangles sharing an edge compute the identical
//!     number and can never both claim, or both miss, a line through it.
//!   - A line exactly through an edge or a vertex goes to one neighbour by a
//!     fixed rule, equivalent to nudging the line by an infinitesimal amount.
//!
//! Inside is then a winding number, the sum of signed crossings below a
//! sample, not a parity bit. Parity is only right for a single closed shell:
//! where two shells overlap (objects joined without welding, or one sculpted
//! through itself) a line crosses four surfaces and parity carves a hollow
//! out of exactly the overlap. Summing gives the union. It is tested against
//! zero rather than "more than zero", so a shell wound backwards still reads
//! as solid.
//!
//! Exact symmetry is built in rather than repaired afterwards. Each triangle
//! is reflected into the quadrant of the lattice line being tested before any
//! arithmetic is done, and its corners sorted, so a line and its mirror image
//! run bit-identical calculations on a symmetric mesh. Ties are settled as if
//! every sample had been moved an infinitesimal step away from all three
//! coordinate planes: a line exactly through an edge is nudged that way, and
//! a sample exactly on the surface is judged from that side of it. A fixed
//! direction instead would test the point a hair above each sample on both
//! sides of a mirror plane, which is not a reflection, and the two halves of
//! a model snapped to the lattice would disagree about their own shape.
//!
//! A mesh with holes has no inside to speak of, and is read as the solid it
//! would be with each hole closed over: a fan of triangles is stretched across
//! every rim before the mesh is scanned (`scan/cap.rs`), and whatever still
//! does not add up after that is settled by a vote between the three axes
//! (`scan/vote.rs`).
//!
//! Only crossings are stored, so memory follows surface area. Inside bits are
//! produced a lattice row at a time, on demand.

use glam::{DVec2, DVec3, Vec3};
use rayon::prelude::*;

use crate::extract::{CellSource, Hermite, hermite_from_edges, values_from_edges};
use crate::grid::{Grid, MAX_SAMPLES, lattice_index};
use crate::parallel;

mod cap;
mod vote;

/// One triangle seen down an axis, reflected into the quadrant of the lattice
/// lines it is about to be tested against.
pub(crate) struct Folded {
    v0: DVec2,
    v1: DVec2,
    v2: DVec2,
    e01: DVec2,
    e02: DVec2,
    e12: DVec2,
    depth: [f64; 3],
    /// +1 when the sorted corners run counter-clockwise, -1 otherwise.
    turn: f64,
    /// Per edge: whether a line exactly on it belongs to this triangle.
    owns: [bool; 3],
    /// +1 where a line travelling up the axis enters the solid here, -1 where
    /// it leaves.
    pub winding: i8,
}

impl Folded {
    /// `q` are the corners across the axis and `depth` along it; `su`, `sv`
    /// (each +1 or -1) reflect them into the quadrant being tested. `None`
    /// for a triangle seen edge-on, which no line passes through.
    pub(crate) fn new(q: [DVec2; 3], depth: [f64; 3], su: f64, sv: f64) -> Option<Folded> {
        let fold = DVec2::new(su, sv);
        let mut v = [(q[0] * fold, depth[0]), (q[1] * fold, depth[1]), (q[2] * fold, depth[2])];
        // Sorted, so that mirror twins (which list their corners in a
        // different order) add the same numbers in the same order.
        let mut parity = 1.0;
        for (a, b) in [(0, 1), (1, 2), (0, 1)] {
            if (v[b].0.x, v[b].0.y) < (v[a].0.x, v[a].0.y) {
                v.swap(a, b);
                parity = -parity;
            }
        }
        let (v0, v1, v2) = (v[0].0, v[1].0, v[2].0);
        let (e01, e02, e12) = (v1 - v0, v2 - v0, v2 - v1);
        let area = e01.perp_dot(e02);
        if area == 0.0 || !area.is_finite() {
            return None;
        }
        let turn = area.signum();
        // A line exactly on an edge is nudged a hair up and to the right (in
        // the reflected frame: away from the coordinate planes). It lands in
        // this triangle when the edge, walked counter-clockwise, runs
        // rightwards, or straight down.
        let owns = |along: DVec2| {
            let along = along * turn;
            along.x > 0.0 || (along.x == 0.0 && along.y < 0.0)
        };
        // Counter-clockwise across the axis means the normal points up it,
        // i.e. a line going up the axis leaves the solid here.
        let winding = if parity * turn * su * sv > 0.0 { -1 } else { 1 };
        Some(Folded {
            v0,
            v1,
            v2,
            e01,
            e02,
            e12,
            depth: [v[0].1, v[1].1, v[2].1],
            turn,
            owns: [owns(e12), owns(-e02), owns(e01)],
            winding,
        })
    }

    /// How far the triangle reaches, across the reflected frame, along the
    /// row of lines at height `v`: where that row cuts its edges. Roughly,
    /// for skipping the lines well clear of it; `hit` is the judge of the
    /// rest. `None` when the row misses the triangle.
    #[inline]
    fn reach(&self, v: f64) -> Option<(f64, f64)> {
        let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
        for (a, b) in [(self.v0, self.v1), (self.v1, self.v2), (self.v2, self.v0)] {
            if (a.y - v) * (b.y - v) <= 0.0 {
                let across = if a.y == b.y { [a.x, b.x] } else { [a.x + (b.x - a.x) * (v - a.y) / (b.y - a.y); 2] };
                low = low.min(across[0].min(across[1]));
                high = high.max(across[0].max(across[1]));
            }
        }
        (low <= high).then_some((low, high))
    }

    /// Where the line through `p` (in the reflected frame) meets the
    /// triangle, as a position along the axis.
    #[inline]
    pub(crate) fn hit(&self, p: DVec2) -> Option<f64> {
        let (r0, r1) = (p - self.v0, p - self.v1);
        // Each edge function is measured from the edge's lower corner, the
        // same for both triangles that share the edge.
        let e = [self.e12.perp_dot(r1), -self.e02.perp_dot(r0), self.e01.perp_dot(r0)];
        for i in 0..3 {
            let side = e[i] * self.turn;
            if !(side > 0.0 || (side == 0.0 && self.owns[i])) {
                return None;
            }
        }
        let sum = e[0] + e[1] + e[2];
        (sum != 0.0).then(|| (e[0] * self.depth[0] + e[1] * self.depth[1] + e[2] * self.depth[2]) / sum)
    }
}

/// Whether a crossing at `t` has been passed by the time a line reaches the
/// sample at `x`. A crossing exactly at the sample counts as passed above the
/// coordinate plane and as still ahead below it: the sample is judged a hair
/// further from the plane than it is, which mirrors exactly.
#[inline]
pub(crate) fn passed(t: f32, x: f32) -> bool {
    if x > 0.0 { t <= x } else { t < x }
}

/// One place the surface crosses a lattice line. Sixteen bytes, and all a
/// lookup needs in one of them.
#[derive(Clone, Copy)]
struct Crossing {
    /// Line in the high half, position along the axis (as sortable bits) in
    /// the low half.
    key: u64,
    /// The surface normal there, as the triangle's winding has it (see
    /// [`pack_direction`]).
    normal: u32,
    /// The winding number just past this crossing.
    after: i16,
    /// +1 where a line travelling up the axis enters the solid, -1 where it
    /// leaves.
    winding: i8,
}

impl Crossing {
    #[inline]
    fn t(&self) -> f32 {
        f32::from_bits(unsortable(self.key as u32))
    }

    #[inline]
    fn line(&self) -> usize {
        (self.key >> 32) as usize
    }

    /// Whether this crossing takes a line between outside and inside (rather
    /// than from one shell into an overlapping one).
    #[inline]
    fn is_boundary(&self) -> bool {
        (self.after == 0) != (self.after - self.winding as i16 == 0)
    }
}

/// Single-precision bits that sort in the same order as the numbers.
#[inline]
fn sortable(t: f32) -> u32 {
    let bits = t.to_bits();
    if bits >> 31 != 0 { !bits } else { bits | 1 << 31 }
}

#[inline]
fn unsortable(bits: u32) -> u32 {
    if bits >> 31 != 0 { bits & !(1 << 31) } else { !bits }
}

/// A direction packed into 32 bits: its octahedral projection, 16 bits a
/// side. The length is not kept. Good to a few hundred-thousandths of a
/// radian, with the axes and the zero of each component exact.
///
/// The normal is packed with its crossing because the triangle's corners are
/// at hand when the crossing is found and nowhere near when it is used; going
/// back to the source mesh for them was most of the cost of placing a vertex.
#[inline]
fn pack_direction(n: DVec3) -> u32 {
    let sum = n.x.abs() + n.y.abs() + n.z.abs();
    if !(sum > 0.0 && sum.is_finite()) {
        return 0;
    }
    let (mut x, mut y) = (n.x / sum, n.y / sum);
    if n.z < 0.0 {
        (x, y) = ((1.0 - y.abs()).copysign(x), (1.0 - x.abs()).copysign(y));
    }
    // Rounded by hand: `round` is a library call on a plain x86-64 build.
    let quantise = |v: f64| (v * 32767.0 + 0.5f64.copysign(v)) as i16 as u16 as u32;
    quantise(x) | quantise(y) << 16
}

#[inline]
pub(crate) fn unpack_direction(packed: u32) -> Vec3 {
    let (x, y) = (packed as u16 as i16 as f32 / 32767.0, (packed >> 16) as u16 as i16 as f32 / 32767.0);
    let z = 1.0 - x.abs() - y.abs();
    let folded = Vec3::new((1.0 - y.abs()).copysign(x), (1.0 - x.abs()).copysign(y), z);
    (if z < 0.0 { folded } else { Vec3::new(x, y, z) }).normalize_or_zero()
}

/// Surface crossings along one axis, grouped by lattice line and sorted along
/// each line.
struct Pass {
    /// Lines are numbered `u + nu * v`, with (u, v) the next two axes after
    /// the pass's own in x, y, z, x, y order.
    offsets: Vec<u32>,
    crossings: Vec<Crossing>,
    /// Lines the surface crosses at all, and those of them that do not close
    /// (the line is still "inside" when it leaves the lattice).
    occupied: usize,
    open: usize,
}

impl Pass {
    #[inline]
    fn line(&self, line: usize) -> std::ops::Range<usize> {
        self.offsets[line] as usize..self.offsets[line + 1] as usize
    }

    #[inline]
    fn is_open(&self, range: &std::ops::Range<usize>) -> bool {
        !range.is_empty() && self.crossings[range.end - 1].after != 0
    }

    /// How many of a line's crossings lie below the sample at `x`, as an
    /// index into the pass.
    #[inline]
    fn below(&self, range: std::ops::Range<usize>, x: f32) -> usize {
        range.start + self.crossings[range].partition_point(|c| passed(c.t(), x))
    }

    /// The winding number just below crossing `c` of a line starting at
    /// `first`: 0 before the line's first crossing.
    #[inline]
    fn winding_below(&self, first: usize, c: usize) -> i32 {
        if c > first { self.crossings[c - 1].after as i32 } else { 0 }
    }
}

/// Triangles per unit of parallel work.
const CHUNK: usize = 2048;

/// Crossings below which sorting on one thread beats handing the work out.
const PARALLEL_SORT: usize = 200_000;

/// [`Grid::span`] for the triangle walk, where it runs three times per source
/// triangle and usually finds nothing: the same answer with no division, no
/// rounding call and no branch for the processor to guess wrong. `inverse` is
/// one over the cell size.
///
/// The floor of the coordinate's lattice index is within one of the answer,
/// and the samples' coordinates rise with their index, so counting how many
/// of the three samples around it fall short settles it exactly.
#[inline]
fn span(grid: &Grid, inverse: f64, axis: usize, lo: f32, hi: f32) -> (usize, usize) {
    let base = grid.lo[axis] as i64;
    let floor = |w: f32| {
        let cells = w as f64 * inverse;
        let whole = lattice_index(cells);
        whole - (cells - 0.5 < whole as f64) as i64 - base
    };
    // The coordinate a sample would have at any index, in or out of the
    // lattice; in it, exactly `Grid::coord`.
    let coord = |k: i64| ((base + k) as f32 + 0.5) * grid.cell;
    let around = |k: i64| [coord(k - 1), coord(k), coord(k + 1)];
    let limit = grid.n[axis] as i64;
    let (at_lo, at_hi) = (floor(lo), floor(hi));
    let first = at_lo - 1 + around(at_lo).iter().filter(|&&c| c < lo).count() as i64;
    let end = at_hi - 1 + around(at_hi).iter().filter(|&&c| c <= hi).count() as i64;
    let (first, end) = (first.clamp(0, limit) as usize, end.clamp(0, limit) as usize);
    debug_assert_eq!((first, end), grid.span(axis, lo, hi));
    (first, end)
}

/// Lines in a row of a triangle's box before it is worth working out which of
/// them the triangle can reach.
const WIDE_ROW: usize = 8;

/// Lattice lines a triangle must span before its rows are shared out between
/// threads. Triangles are otherwise each one thread's work, which would leave
/// a coarse model at a fine resolution (a twelve-triangle box, say) to a
/// single core.
const SHARED_OUT: usize = 1 << 15;

/// Adds the crossings of one triangle with the lattice lines along `axis`,
/// given the samples its bounds take in on the other two axes. `normal` is
/// the triangle's packed normal, worked out the first time any axis needs it.
fn rasterize(
    p: &[Vec3; 3],
    grid: &Grid,
    inverse: f64,
    axis: usize,
    [(u0, u1), (v0, v1)]: [(usize, usize); 2],
    normal: &mut Option<u32>,
    out: &mut Vec<Crossing>,
) {
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let q = p.map(|p| DVec2::new(p[u] as f64, p[v] as f64));
    let depth = p.map(|p| p[axis] as f64);
    let across = || (p[1] - p[0]).as_dvec3().cross((p[2] - p[0]).as_dvec3());
    let normal = *normal.get_or_insert_with(|| pack_direction(across()));
    // The part of a run of samples below the coordinate plane and the part
    // above it, with the sign that reflects each into the positive quadrant.
    let halves = |from: usize, to: usize, zero: usize| [(-1.0, from, to.min(zero)), (1.0, from.max(zero), to)];
    for (sv, va, vb) in halves(v0, v1, grid.zero_index(v)) {
        for (su, ua, ub) in halves(u0, u1, grid.zero_index(u)) {
            if va >= vb || ua >= ub {
                continue;
            }
            let Some(folded) = Folded::new(q, depth, su, sv) else { continue };
            let row = |iv: usize, out: &mut Vec<Crossing>| {
                let pv = grid.coord(v, iv).abs() as f64;
                // A long row is cut down to the part the triangle reaches,
                // with a line to spare each side: a sliver lying across the
                // lattice covers very little of the box round it.
                let (mut ua, mut ub) = (ua, ub);
                if ub - ua > WIDE_ROW {
                    let Some((low, high)) = folded.reach(pv) else { return };
                    let (low, high) = if su > 0.0 { (low, high) } else { (-high, -low) };
                    let (first, end) = span(grid, inverse, u, low as f32, high as f32);
                    (ua, ub) = (ua.max(first.saturating_sub(1)), ub.min(end + 1));
                }
                for iu in ua..ub {
                    let pu = grid.coord(u, iu).abs() as f64;
                    let Some(t) = folded.hit(DVec2::new(pu, pv)) else { continue };
                    // Adding zero turns -0 into +0, which sorts differently
                    // but means the same.
                    let t = t as f32 + 0.0;
                    if t.is_finite() {
                        let key = ((iu + iv * grid.n[u]) as u64) << 32 | sortable(t) as u64;
                        out.push(Crossing { key, normal, after: 0, winding: folded.winding });
                    }
                }
            };
            if (vb - va) * (ub - ua) < SHARED_OUT {
                (va..vb).for_each(|iv| row(iv, out));
            } else {
                let rows = (va..vb).into_par_iter().fold(Vec::new, |mut part, iv| {
                    row(iv, &mut part);
                    part
                });
                rows.collect::<Vec<_>>().iter().for_each(|part| out.extend_from_slice(part));
            }
        }
    }
}

/// Sorts one axis's crossings into lines and works out the winding along each.
fn build_pass(parts: Vec<Vec<Crossing>>, lines: usize) -> Result<Pass, String> {
    let mut all = parallel::concat(parts);
    if all.len() >= u32::MAX as usize {
        return Err("That resolution is too fine for this model. Try a lower one.".into());
    }
    // Where two crossings coincide, entering goes before leaving, so one
    // shell ending exactly where another begins is never read as a gap. The
    // normal last, only so that the order never depends on how the work
    // happened to be split between threads.
    let order = |a: &Crossing, b: &Crossing| {
        a.key.cmp(&b.key).then(b.winding.cmp(&a.winding)).then(a.normal.cmp(&b.normal))
    };
    if all.len() < PARALLEL_SORT {
        all.sort_unstable_by(order);
    } else {
        all.par_sort_unstable_by(order);
    }

    let mut offsets = vec![0u32; lines + 1];
    let (mut occupied, mut open) = (0, 0);
    let mut c = 0;
    while c < all.len() {
        let line = all[c].line();
        let first = c;
        let mut winding = 0i16;
        while c < all.len() && all[c].line() == line {
            winding = winding.saturating_add(all[c].winding as i16);
            all[c].after = winding;
            c += 1;
        }
        offsets[line + 1] = (c - first) as u32;
        occupied += 1;
        open += (winding != 0) as usize;
    }
    for i in 0..lines {
        offsets[i + 1] += offsets[i];
    }
    Ok(Pass { offsets, crossings: all, occupied, open })
}

/// Unit normal of a triangle, the way its winding faces; zero only for a
/// triangle with no area at all. Worked in double precision: a cross
/// product's length is twice the triangle's area, and the triangles of a
/// dense or small model are tiny enough for single-precision squares of that
/// to vanish, which once left most of a remesh without normals.
pub fn unit_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let n = (b - a).as_dvec3().cross((c - a).as_dvec3());
    let length = n.length();
    if length > 0.0 { (n / length).as_vec3() } else { Vec3::ZERO }
}

/// `n`, turned if need be to point out of the solid across a lattice edge
/// along `axis` whose lower end is inside (or outside). Looking at which side
/// is solid rather than trusting the triangle's winding is what makes a shell
/// wound backwards, or one being subtracted, come out facing the right way.
#[inline]
pub(crate) fn facing_out(n: Vec3, axis: usize, lower_inside: bool) -> Vec3 {
    if (n[axis] > 0.0) == lower_inside || n[axis] == 0.0 { n } else { -n }
}

/// Writes the inside bits of the samples in row (j, k) as the lines along x
/// alone have them. A line that does not close gives nothing.
fn row_along_x(grid: &Grid, along_x: &Pass, j: usize, k: usize, out: &mut [u64]) -> bool {
    out.fill(0);
    let range = along_x.line(j + k * grid.n[1]);
    if range.is_empty() || along_x.is_open(&range) {
        return false;
    }
    let (mut inside, mut from, mut any) = (false, 0usize, false);
    for crossing in &along_x.crossings[range] {
        // The first sample this crossing has been passed at; see `passed`.
        let t = crossing.t();
        let to = if t > 0.0 { grid.first_at_or_above(0, t) } else { grid.first_above(0, t) };
        if inside && to > from {
            set_bits(out, from, to);
            any = true;
        }
        inside = crossing.after != 0;
        from = to;
    }
    any
}

/// Whether the three axes' lines disagree about the solid, which is to say
/// whether the mesh has holes.
///
/// A line left open is the plain sign of one. It is not the only sign: a
/// line through two holes, in and out the far side, meets nothing and looks
/// as healthy as a line that missed the model. A pipe lying exactly along x
/// has no open line on any axis, and the lines along x see no pipe at all.
/// What gives it away is that the axes then disagree about how much solid
/// there is, by the whole bore. They do not agree to the last sample even on
/// a closed mesh (a sample within rounding of the surface can fall either
/// side by axis; a handful in half a billion, measured), so a little slack is
/// allowed: less than one lattice line through anything but a sliver.
fn holed(grid: &Grid, pass: &[Pass; 3]) -> bool {
    if pass.iter().any(|p| p.open > 0) {
        return true;
    }
    let crossings: usize = pass.iter().map(|p| p.crossings.len()).sum();
    let inside = [0, 1, 2].map(|axis| vote::inside_count(grid, axis, &pass[axis]));
    let spread = inside.iter().max().unwrap() - inside.iter().min().unwrap();
    spread > crossings as u64 / 20_000 + 16
}

/// A mesh scan-converted onto a lattice. Keeps nothing of the mesh itself.
///
/// A mesh with holes is taken for the solid it would be with each hole
/// capped: a fan is stretched over every rim (`scan/cap.rs`) and that scanned
/// instead. Whatever is still open after that (a tear with no rim to speak
/// of, a surface that is not consistently wound) is settled by a vote between
/// the three axes (`scan/vote.rs`), which keeps the damage to the tear itself.
pub struct Scan {
    grid: Grid,
    pass: [Pass; 3],
    /// Inside and outside by the vote of all three axes, where the lines
    /// still disagree after capping. `None` for a mesh that is closed or was
    /// capped closed, whose lines along x say all there is to say.
    voted: Option<vote::Voted>,
    /// As the mesh was given, before any capping: whether it showed a hole,
    /// and how many of the lattice lines it meets it leaves open.
    holed: bool,
    /// Whether a cap was stretched over its holes before it was read.
    capped: bool,
    occupied: usize,
    open: usize,
}

impl Scan {
    /// Scan-converts the triangles onto `grid`. Triangles beyond the lattice
    /// sideways are clipped; beyond it along a line they still count, so a
    /// mesh larger than the lattice keeps a correct inside.
    pub fn new(positions: &[Vec3], indices: &[u32], grid: &Grid) -> Result<Scan, String> {
        Scan::build(positions, indices, grid, true)
    }

    /// [`Self::new`], with the capping of holes optional: without it, the
    /// vote between the axes is all that stands between a hole and a streak.
    pub(crate) fn build(positions: &[Vec3], indices: &[u32], grid: &Grid, cap_holes: bool) -> Result<Scan, String> {
        let mut pass = Scan::passes(positions, indices, grid)?;
        let occupied = pass.iter().map(|p| p.occupied).sum();
        let open = pass.iter().map(|p| p.open).sum();
        let was_holed = holed(grid, &pass);
        let mut capped = false;
        if was_holed && cap_holes {
            if let Some((positions, indices)) = cap::capped(positions, indices) {
                pass = Scan::passes(&positions, &indices, grid)?;
                capped = true;
            }
        }
        let voted = (was_holed && holed(grid, &pass)).then(|| vote::Voted::new(grid, &pass));
        Ok(Scan { grid: *grid, pass, voted, holed: was_holed, capped, occupied, open })
    }

    /// The crossings of the triangles with the lattice lines of each axis.
    fn passes(positions: &[Vec3], indices: &[u32], grid: &Grid) -> Result<[Pass; 3], String> {
        // Lines are numbered in 32 bits, and cells later in 16 a side.
        if grid.n.iter().any(|&n| n > MAX_SAMPLES) {
            return Err("That resolution is too fine for this model. Try a lower one.".into());
        }
        // One walk over the triangles for all three axes: the corners are
        // fetched once, and fetching them is most of the cost on a mesh too
        // large for the cache.
        let inverse = 1.0 / grid.cell as f64;
        let chunks: Vec<[Vec<Crossing>; 3]> = indices
            .par_chunks(3 * CHUNK)
            .map(|tris| {
                let mut out: [Vec<Crossing>; 3] = Default::default();
                for tri in tris.chunks_exact(3) {
                    let p = [positions[tri[0] as usize], positions[tri[1] as usize], positions[tri[2] as usize]];
                    let (low, high) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
                    let spans: [(usize, usize); 3] = std::array::from_fn(|a| span(grid, inverse, a, low[a], high[a]));
                    // Lines along an axis can only meet a triangle whose
                    // bounds take in a sample on both of the other two. For a
                    // triangle smaller than a cell that is rare.
                    let some = spans.map(|(first, end)| first < end);
                    if (some[0] as u8 + some[1] as u8 + some[2] as u8) < 2 {
                        continue;
                    }
                    let mut normal = None;
                    for (axis, out) in out.iter_mut().enumerate() {
                        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                        if some[u] && some[v] {
                            rasterize(&p, grid, inverse, axis, [spans[u], spans[v]], &mut normal, out);
                        }
                    }
                }
                out
            })
            .collect();
        let mut parts: [Vec<Vec<Crossing>>; 3] = Default::default();
        for chunk in chunks {
            for (axis, part) in chunk.into_iter().enumerate() {
                parts[axis].push(part);
            }
        }
        let [x, y, z] = parts;
        let lines = |axis: usize| grid.n[(axis + 1) % 3] * grid.n[(axis + 2) % 3];
        let (x, (y, z)) = rayon::join(
            || build_pass(x, lines(0)),
            || rayon::join(|| build_pass(y, lines(1)), || build_pass(z, lines(2))),
        );
        Ok([x?, y?, z?])
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// True when the mesh as given showed no sign of a hole: every lattice
    /// line that enters the surface also leaves it, and the three axes agree
    /// about how much solid there is.
    pub fn is_closed(&self) -> bool {
        !self.holed
    }

    /// True when the mesh showed holes and a cap was stretched over each rim
    /// before it was read (`scan/cap.rs`): the solid scanned is the mesh with
    /// its holes closed flat, which whoever asked may want to be told.
    pub fn was_capped(&self) -> bool {
        self.capped
    }

    /// True when the mesh was still open after any capping (a tear with no
    /// rim to speak of, a surface not consistently wound), so that inside and
    /// outside were settled by the vote between the axes (`scan/vote.rs`).
    /// The result is closed all the same; near the tear it is a best guess.
    pub fn was_voted(&self) -> bool {
        self.voted.is_some()
    }

    /// Lattice lines the mesh as given does not close, as a fraction of the
    /// lines it crosses: 0 for a closed mesh, small for one with a few holes,
    /// towards 1 for an open sheet. 0 when no line meets the mesh at all.
    pub fn open_fraction(&self) -> f32 {
        if self.occupied == 0 { 0.0 } else { self.open as f32 / self.occupied as f32 }
    }

    /// True when no sample of the lattice is inside: a sheet, or anything
    /// else with no volume even once its holes are capped.
    pub fn encloses_nothing(&self) -> bool {
        match &self.voted {
            Some(voted) => voted.is_empty(),
            None => vote::inside_count(&self.grid, 0, &self.pass[0]) == 0,
        }
    }

    /// True when no lattice line meets the mesh.
    pub fn is_empty(&self) -> bool {
        self.pass.iter().all(|p| p.occupied == 0)
    }

    pub fn crossing_count(&self) -> usize {
        self.pass.iter().map(|p| p.crossings.len()).sum()
    }

    #[inline]
    fn line_of(&self, axis: usize, at: [usize; 3]) -> usize {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        at[u] + at[v] * self.grid.n[u]
    }

    /// Inside or outside at one sample, judged along the lattice line through
    /// it on `axis` alone. A line that does not close says outside.
    pub fn inside_along(&self, axis: usize, at: [usize; 3]) -> bool {
        let pass = &self.pass[axis];
        let range = pass.line(self.line_of(axis, at));
        if range.is_empty() || pass.is_open(&range) {
            return false;
        }
        let below = pass.below(range.clone(), self.grid.coord(axis, at[axis]));
        pass.winding_below(range.start, below) != 0
    }

    /// The winding number at a sample along `axis`: how many shells it is
    /// inside of.
    pub(crate) fn winding_at(&self, axis: usize, at: [usize; 3]) -> i32 {
        let pass = &self.pass[axis];
        let range = pass.line(self.line_of(axis, at));
        let below = pass.below(range.clone(), self.grid.coord(axis, at[axis]));
        pass.winding_below(range.start, below)
    }

    /// Inside or outside at one sample. Slower than [`Self::row`] for many.
    ///
    /// For a closed mesh this is what the line along x through the sample
    /// says. For one with holes it is the majority of the three lines through
    /// it, a line that does not close voting outside.
    pub fn inside(&self, at: [usize; 3]) -> bool {
        match &self.voted {
            Some(voted) => voted.inside(at[1] + at[2] * self.grid.n[1], at[0]),
            None => self.inside_along(0, at),
        }
    }

    /// Writes the inside bits of the samples in row (j, k), bit `i` for
    /// sample `i` along x. Returns whether any is inside.
    pub fn row(&self, j: usize, k: usize, out: &mut [u64]) -> bool {
        match &self.voted {
            Some(voted) => voted.row(j + k * self.grid.n[1], out),
            None => row_along_x(&self.grid, &self.pass[0], j, k, out),
        }
    }

    /// The crossings on the lattice edge from sample `at` one step up `axis`,
    /// as indices into that axis's pass.
    ///
    /// Normally those passed at the upper sample and not at the lower. The
    /// three passes break a tie the same way only when the surface is not
    /// also exactly through a sample, so when that finds nothing the edge's
    /// two ends are taken in as well: a crossing the passes disagree about is
    /// exactly at one of them.
    #[inline]
    fn edge_range(&self, axis: usize, at: [usize; 3]) -> std::ops::Range<usize> {
        let pass = &self.pass[axis];
        let range = pass.line(self.line_of(axis, at));
        if range.is_empty() {
            return range;
        }
        let (lower, upper) = (self.grid.coord(axis, at[axis]), self.grid.coord(axis, at[axis] + 1));
        let from = pass.below(range.clone(), lower);
        let to = pass.below(from..range.end, upper);
        if from < to {
            return from..to;
        }
        let line = &pass.crossings[range.clone()];
        range.start + line.partition_point(|c| c.t() < lower)..range.start + line.partition_point(|c| c.t() <= upper)
    }

    /// Each crossing on the lattice edge from `at` one step up `axis`, in
    /// order: position along the axis, +1 entering or -1 leaving, and the
    /// surface normal there as [`unpack_direction`] reads it.
    pub(crate) fn edge_crossings(&self, axis: usize, at: [usize; 3]) -> impl Iterator<Item = (f32, i8, u32)> + '_ {
        self.pass[axis].crossings[self.edge_range(axis, at)].iter().map(|c| (c.t(), c.winding, c.normal))
    }

    /// Where the surface crosses the lattice edge from sample `at` one step
    /// up `axis`, as a fraction of the edge, with the outward normal there.
    /// `lower_inside` says which end is solid. `None` when no crossing was
    /// recorded on the edge.
    ///
    /// Of several crossings on one edge, the ones between outside and inside
    /// are the surface; the rest are one shell passing into another.
    pub fn edge_hit(&self, axis: usize, at: [usize; 3], lower_inside: bool) -> Option<(f32, Vec3)> {
        let (fraction, normal) = self.edge_fraction(axis, at)?;
        Some((fraction, facing_out(unpack_direction(normal), axis, lower_inside)))
    }

    /// [`Self::edge_hit`] with the normal still packed.
    fn edge_fraction(&self, axis: usize, at: [usize; 3]) -> Option<(f32, u32)> {
        let on_edge = &self.pass[axis].crossings[self.edge_range(axis, at)];
        let pick = |boundary_only: bool| {
            let (mut sum, mut count, mut normal) = (0.0f32, 0u32, 0u32);
            for c in on_edge.iter().filter(|c| !boundary_only || c.is_boundary()) {
                if count == 0 {
                    normal = c.normal;
                }
                sum += c.t();
                count += 1;
            }
            (count > 0).then(|| (sum / count as f32, normal))
        };
        let (t, normal) = pick(true).or_else(|| pick(false))?;
        let fraction = (t - self.grid.coord(axis, at[axis])) / self.grid.cell;
        Some((fraction.clamp(0.0, 1.0), normal))
    }
}

impl CellSource for Scan {
    fn row(&self, j: usize, k: usize, out: &mut [u64], _scratch: &mut Vec<u64>) -> bool {
        Scan::row(self, j, k, out)
    }

    /// The normal at each crossing is the true face normal of the triangle
    /// the lattice edge passes through, not a finite difference of a sampled
    /// field, which is what lets the vertex solve rebuild a crease instead of
    /// a smoothed approximation of one.
    fn hermite(&self, cell: [usize; 3], mask: u8, out: &mut Hermite) {
        hermite_from_edges(cell, mask, out, |axis, at, lower_inside| self.edge_hit(axis, at, lower_inside));
    }

    fn corner_values(&self, cell: [usize; 3], mask: u8) -> [f32; 8] {
        values_from_edges(&self.grid, cell, mask, |axis, at| self.edge_fraction(axis, at).map(|hit| hit.0))
    }
}

/// Sets bits `from..to` of a bit row.
#[inline]
pub(crate) fn set_bits(words: &mut [u64], from: usize, to: usize) {
    let (first, last) = (from >> 6, (to - 1) >> 6);
    let head = !0u64 << (from & 63);
    let tail = !0u64 >> (63 - ((to - 1) & 63));
    if first == last {
        words[first] |= head & tail;
    } else {
        words[first] |= head;
        words[first + 1..last].fill(!0);
        words[last] |= tail;
    }
}

/// The vote of `scan/vote.rs` asked the slow way, a sample at a time: what the
/// plane-at-a-time version has to agree with, and what it is timed against.
#[cfg(test)]
pub(crate) struct BySample<'a>(pub &'a Scan);

#[cfg(test)]
impl CellSource for BySample<'_> {
    fn row(&self, j: usize, k: usize, out: &mut [u64], _scratch: &mut Vec<u64>) -> bool {
        let scan = self.0;
        row_along_x(&scan.grid, &scan.pass[0], j, k, out);
        // Z before y: its lines for one row sit side by side in memory, and y
        // is only needed to break a tie between the other two.
        let nx = scan.grid.n[0];
        let along_z = &scan.pass[2].offsets[j * nx..=(j + 1) * nx];
        let mut any = false;
        for (w, word) in out.iter_mut().enumerate() {
            let (from, to) = (w * 64, ((w + 1) * 64).clamp(0, nx));
            if *word == 0 && along_z[from] == along_z[to] {
                continue;
            }
            for i in from..to {
                let x = *word >> (i & 63) & 1 != 0;
                let z = scan.inside_along(2, [i, j, k]);
                if x != z && scan.inside_along(1, [i, j, k]) != x {
                    *word ^= 1 << (i & 63);
                }
            }
            any |= *word != 0;
        }
        any
    }

    fn hermite(&self, cell: [usize; 3], mask: u8, out: &mut Hermite) {
        self.0.hermite(cell, mask, out);
    }

    fn corner_values(&self, cell: [usize; 3], mask: u8) -> [f32; 8] {
        self.0.corner_values(cell, mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{bowl, box_mesh, icosphere, open_tube, rotated, uv_sphere, without_near};
    use sculpt_geom::{Aabb, TriMesh};

    fn scan_grid(mesh: &TriMesh, cell: f32) -> Grid {
        Grid::covering(&mesh.bounds(), cell).unwrap()
    }

    /// Every sample of the lattice, as inside or outside.
    fn signs(scan: &Scan) -> Vec<bool> {
        let g = *scan.grid();
        let mut row = vec![0u64; g.n[0].div_ceil(64)];
        let mut out = Vec::with_capacity(g.sample_count());
        for k in 0..g.n[2] {
            for j in 0..g.n[1] {
                scan.row(j, k, &mut row);
                out.extend((0..g.n[0]).map(|i| row[i >> 6] >> (i & 63) & 1 != 0));
            }
        }
        out
    }

    #[test]
    fn bits_are_set_across_word_boundaries() {
        let mut w = [0u64; 3];
        set_bits(&mut w, 60, 131);
        assert_eq!(w, [0xF << 60, !0, 0b111]);
        let mut w = [0u64; 1];
        set_bits(&mut w, 3, 4);
        assert_eq!(w, [8]);
    }

    #[test]
    fn sortable_bits_keep_the_order() {
        let values = [-3.5f32, -1e-20, 0.0, 1e-20, 0.7, 12.0];
        for pair in values.windows(2) {
            assert!(sortable(pair[0]) < sortable(pair[1]));
        }
        for v in values {
            assert_eq!(f32::from_bits(unsortable(sortable(v))), v);
        }
    }

    #[test]
    fn packed_directions_come_back_as_they_went_in() {
        let mut rng = crate::testing::Rng::new(9);
        let mut worst = 0.0f64;
        for _ in 0..20_000 {
            let n = (rng.vec3() - 0.5).normalize();
            // Any length packs to the same direction.
            let back = unpack_direction(pack_direction(n.as_dvec3() * 1e-9));
            // The sine of the angle between them, which single-precision
            // arc-cosine cannot resolve this close to zero.
            worst = worst.max(n.as_dvec3().cross(back.as_dvec3()).length());
        }
        assert!(worst < 1e-4, "worst error {worst} radians");
        for axis in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z] {
            assert_eq!(unpack_direction(pack_direction(axis.as_dvec3())), axis);
        }
        assert_eq!(unpack_direction(pack_direction(DVec3::new(0.0, 3.0, -3.0))).x, 0.0);
    }

    #[test]
    fn a_sphere_is_inside_where_it_should_be() {
        let mesh = icosphere(4, 1.0, Vec3::new(0.11, -0.07, 0.23));
        let grid = scan_grid(&mesh, 0.05);
        let scan = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
        assert!(scan.is_closed());
        let inside = signs(&scan);
        let mut wrong = 0;
        for k in 0..grid.n[2] {
            for j in 0..grid.n[1] {
                for i in 0..grid.n[0] {
                    let r = grid.position(i, j, k).distance(Vec3::new(0.11, -0.07, 0.23));
                    // The icosphere is inscribed: allow for its flat faces.
                    if (r - 1.0).abs() > 0.02 && (r < 1.0) != inside[grid.index(i, j, k)] {
                        wrong += 1;
                    }
                    assert_eq!(scan.inside([i, j, k]), inside[grid.index(i, j, k)]);
                }
            }
        }
        assert_eq!(wrong, 0);
    }

    /// The test for a ray slipping between two triangles: on a closed mesh
    /// every lattice line must leave the surface as often as it enters it,
    /// along all three axes. One leaked or doubled crossing on any line of
    /// any pass fails this.
    #[test]
    fn no_line_through_a_dense_closed_mesh_is_left_open() {
        for mesh in [icosphere(6, 0.5, Vec3::ZERO), uv_sphere(160, 320, 0.5, Vec3::new(0.01, 0.02, 0.03))] {
            for cell in [1.0 / 96.0, 0.0173] {
                let grid = scan_grid(&mesh, cell);
                let scan = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
                assert_eq!(scan.pass.iter().map(|p| p.open).collect::<Vec<_>>(), [0, 0, 0]);
                assert!(scan.pass.iter().all(|p| p.occupied > 0));
                // And the three axes count the same solid, to a few samples:
                // nothing here is mistaken for a mesh with holes.
                let inside = [0, 1, 2].map(|axis| vote::inside_count(&grid, axis, &scan.pass[axis]));
                let spread = inside.iter().max().unwrap() - inside.iter().min().unwrap();
                assert!(spread <= 4 && scan.is_closed(), "the axes count {inside:?} samples inside");
            }
        }
    }

    /// Lattice lines running exactly along edges and through vertices: a box
    /// whose faces, edges and corners all land on samples.
    #[test]
    fn a_box_snapped_to_the_lattice_still_closes_and_is_symmetric() {
        let mesh = box_mesh(Vec3::splat(-0.75), Vec3::splat(0.75));
        let grid = Grid::covering(&mesh.bounds(), 0.1).unwrap();
        assert!((0..grid.n[0]).any(|i| grid.coord(0, i) == 0.75));
        let scan = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
        assert!(scan.is_closed());
        let inside = signs(&scan);
        let n = grid.n;
        for k in 0..n[2] {
            for j in 0..n[1] {
                for i in 0..n[0] {
                    let p = grid.position(i, j, k);
                    let here = inside[grid.index(i, j, k)];
                    // A sample on the surface is judged a hair further from
                    // the coordinate planes, which here is outside, whichever
                    // of the three passes would have been asked.
                    assert_eq!(here, p.abs().max_element() < 0.75, "at {p}");
                    for axis in 0..3 {
                        assert_eq!(scan.inside_along(axis, [i, j, k]), here, "axis {axis} at {p}");
                    }
                    assert_eq!(here, inside[grid.index(n[0] - 1 - i, j, k)]);
                    assert_eq!(here, inside[grid.index(i, n[1] - 1 - j, k)]);
                    assert_eq!(here, inside[grid.index(i, j, n[2] - 1 - k)]);
                }
            }
        }
    }

    #[test]
    fn overlapping_shells_are_their_union_not_their_difference() {
        let mut mesh = icosphere(3, 1.0, Vec3::new(-0.4, 0.0, 0.0));
        mesh.append(&icosphere(3, 1.0, Vec3::new(0.4, 0.0, 0.0)));
        let grid = scan_grid(&mesh, 0.1);
        let scan = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
        let at = |p: Vec3| {
            let i = [0, 1, 2].map(|a| grid.first_at_or_above(a, p[a]));
            (scan.inside(i), scan.winding_at(0, i))
        };
        assert_eq!(at(Vec3::ZERO), (true, 2));
        assert_eq!(at(Vec3::new(1.0, 0.0, 0.0)), (true, 1));
        assert_eq!(at(Vec3::new(0.0, 1.2, 0.0)), (false, 0));
    }

    #[test]
    fn a_shell_wound_backwards_is_still_solid() {
        let mut mesh = icosphere(3, 1.0, Vec3::ZERO);
        mesh.flip();
        let grid = scan_grid(&mesh, 0.1);
        let scan = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
        let centre = [0, 1, 2].map(|a| grid.first_at_or_above(a, 0.0));
        assert!(scan.inside(centre));
        // Its normals face inwards, and are turned to face out of the solid.
        let (_, n) = scan.edge_hit(0, [grid.first_at_or_above(0, 0.9), centre[1], centre[2]], true).unwrap();
        assert!(n.x > 0.5);
    }

    #[test]
    fn a_hole_does_not_paint_a_streak() {
        // A sphere with one triangle fan missing.
        let mut mesh = icosphere(3, 1.0, Vec3::ZERO);
        mesh.indices.truncate(mesh.indices.len() - 3 * 12);
        let grid = scan_grid(&mesh, 0.08);
        let scan = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
        assert!(!scan.is_closed() && scan.open_fraction() < 0.2);
        let inside = signs(&scan);
        let mut wrong = 0;
        for k in 0..grid.n[2] {
            for j in 0..grid.n[1] {
                for i in 0..grid.n[0] {
                    let r = grid.position(i, j, k).length();
                    if (r - 1.0).abs() > 0.3 && (r < 1.0) != inside[grid.index(i, j, k)] {
                        wrong += 1;
                    }
                }
            }
        }
        assert_eq!(wrong, 0);
    }

    /// The vote worked a plane at a time gives, bit for bit, what asking the
    /// three lines through every sample gives: on small holes, on a bowl, on
    /// shapes open at both ends, turned every which way.
    #[test]
    fn the_vote_by_planes_is_the_vote_by_samples() {
        let sphere = icosphere(4, 0.5, Vec3::new(0.02, 0.01, -0.01));
        let ends = [Vec3::new(0.52, 0.01, -0.01), Vec3::new(-0.48, 0.01, -0.01)];
        let pierced = without_near(&without_near(&sphere, ends[0], 0.2), ends[1], 0.2);
        let shapes = [
            without_near(&sphere, Vec3::new(0.3, 0.3, 0.25), 0.07),
            pierced,
            bowl(24, 48, 0.5),
            rotated(&bowl(24, 48, 0.5), Vec3::new(35.0, 20.0, 50.0)),
            open_tube(40, 12, 0.3, 1.1),
            rotated(&open_tube(40, 12, 0.3, 1.1), Vec3::new(10.0, 40.0, 25.0)),
        ];
        for (shape, mesh) in shapes.iter().enumerate() {
            // Sizes either side of a whole number of words to a row.
            for cell in [0.9 / 59.0, 0.031] {
                let grid = scan_grid(mesh, cell);
                // Uncapped, or there would be nothing left to vote about.
                let scan = Scan::build(&mesh.positions, &mesh.indices, &grid, false).unwrap();
                assert!(!scan.is_closed() && scan.voted.is_some(), "shape {shape} was meant to have a hole");
                let words = grid.n[0].div_ceil(64);
                let (mut row, mut slow, mut scratch) = (vec![0u64; words], vec![0u64; words], Vec::new());
                let mut inside = 0;
                for k in 0..grid.n[2] {
                    for j in 0..grid.n[1] {
                        let any = scan.row(j, k, &mut row);
                        assert_eq!(BySample(&scan).row(j, k, &mut slow, &mut scratch), any);
                        assert_eq!(row, slow, "shape {shape}, row ({j}, {k})");
                        for i in 0..grid.n[0] {
                            let votes = (0..3).filter(|&axis| scan.inside_along(axis, [i, j, k])).count();
                            assert_eq!(scan.inside([i, j, k]), votes >= 2, "shape {shape} at ({i}, {j}, {k})");
                            inside += (votes >= 2) as usize;
                        }
                    }
                }
                assert!(inside > 500, "shape {shape}: only {inside} samples inside");
            }
        }
    }

    #[test]
    fn an_open_sheet_is_reported_as_open() {
        let mesh = TriMesh::new(
            vec![Vec3::new(-1.0, 0.01, -1.0), Vec3::new(1.0, 0.01, -1.0), Vec3::new(0.0, 0.01, 1.0)],
            vec![0, 1, 2],
        );
        let grid = Grid::covering(&Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)), 0.1).unwrap();
        let scan = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
        assert!(scan.open_fraction() > 0.99);
        assert!(!signs(&scan).iter().any(|&s| s));
    }

    #[test]
    fn crossings_sit_on_the_surface_with_outward_normals() {
        let mesh = icosphere(5, 1.0, Vec3::ZERO);
        let grid = scan_grid(&mesh, 0.07);
        let scan = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap();
        let mut seen = 0;
        for k in 1..grid.n[2] - 1 {
            for j in 1..grid.n[1] - 1 {
                for i in 1..grid.n[0] - 1 {
                    let here = scan.inside([i, j, k]);
                    for axis in 0..3 {
                        let mut up = [i, j, k];
                        up[axis] += 1;
                        if scan.inside(up) == here {
                            continue;
                        }
                        let (f, n) = scan.edge_hit(axis, [i, j, k], here).expect("a sign change has a crossing");
                        let p = grid.position(i, j, k).lerp(grid.position(up[0], up[1], up[2]), f);
                        assert!((p.length() - 1.0).abs() < 2e-3, "crossing off the sphere: {p}");
                        assert!(n.dot(p.normalize()) > 0.98);
                        seen += 1;
                    }
                }
            }
        }
        assert!(seen > 1000);
    }
}
