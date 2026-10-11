//! Decimation: the same shape in fewer triangles, down to a count asked for.
//!
//! Quadric edge collapse, after Garland and Heckbert ("Surface Simplification
//! Using Quadric Error Metrics", 1997). Every vertex keeps a quadric: a small
//! matrix that measures how far a point is, squared, from the planes of the
//! triangles the vertex stood for. An edge is collapsed to one vertex, placed
//! where the sum of its two ends' quadrics is least, and the cheapest edge of
//! the whole mesh goes first. Flat ground loses its triangles long before a
//! ridge or a fold loses one, which is what a model that has to fit a
//! triangle budget (a game asset: 200k to 500k, say) wants.
//!
//! What a caller can rely on:
//!
//!   - **The surface stays the surface it was.** A collapse is only made if
//!     it leaves the mesh's connectivity what it was (the link condition): a
//!     closed mesh comes out closed, with every edge on exactly two
//!     triangles; a handle stays a handle; an open border stays one border.
//!     Nor is one made that would turn a triangle over or flatten it to
//!     nothing. So the count asked for is a target, not a promise: when no
//!     collapse is left that keeps these, the result has more triangles than
//!     were asked for, and says so ([`Decimated::reached`]).
//!   - **A triangle of the result is a triangle of the input**, with corners
//!     that may have moved ([`Decimated::triangle_source`]). Whatever the
//!     caller keeps by triangle (hidden faces) comes across by that.
//!   - **Locked vertices are not moved and not removed** (the caller's mask):
//!     a neighbour may be folded into one, at the locked vertex's own place.
//!     Surface with every corner locked is left exactly as it is.
//!   - **Borders keep their line.** Each open edge adds a plane through
//!     itself, square to its triangle, to its two ends, so sliding a border
//!     vertex along the border is cheap and pulling the border in is dear.
//!   - **A value per vertex is carried** (the mask): the new vertex takes it
//!     from the two ends, by how far along the edge it was placed.
//!   - **The same input gives the same output**, on any number of cores: the
//!     collapses are made one at a time, in order of cost and then of vertex
//!     number.
//!
//! It does not keep a mesh's mirror symmetry: the two halves are collapsed in
//! whatever order their costs come, which rounding alone makes different.
//!
//! The work is the caller's thread's. Setting up (quadrics, the first cost of
//! every edge) uses rayon; the collapses themselves are one after another,
//! about a microsecond each. `progress` is called as it goes and can stop it.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::{DVec3, Vec3};
use rayon::prelude::*;

/// A triangle may turn this far (the cosine of the angle between its normal
/// before and after) in one collapse and no further. Turning past a right
/// angle is a fold; this stops well short of it.
const MOST_TURN: f64 = 0.2;

/// The fewest triangles a closed surface can have, and the fewest asked of
/// any mesh.
const FEWEST: usize = 4;

/// How far from the middle of an edge, in edge lengths, the cheapest point
/// may be and still be used. On flat or cylindrical ground the quadric is
/// all but singular and its minimum can be anywhere along the flat
/// direction; an end or the middle of the edge is used then.
const FARTHEST_PLACE: f64 = 1.5;

/// The weight of a border edge's plane, as a multiple of the square of the
/// edge's length (a triangle's plane weighs its area).
const BORDER_WEIGHT: f64 = 10.0;

/// What is added to an edge's cost for its length, as a fraction of the mean
/// triangle area times the length squared. On flat ground every collapse
/// costs nothing, and this makes the short edges go first, so the triangles
/// left there are even ones and not long slivers. It is small enough to
/// decide nothing where the surface bends.
const LENGTH_TIE: f64 = 1e-8;

/// How many collapses are made between one call of `progress` and the next.
const PROGRESS_EVERY: usize = 4096;

/// A decimated mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct Decimated {
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
    /// The value carried for each vertex of the result, when one was given.
    pub values: Option<Vec<f32>>,
    /// For each triangle of the result, which triangle of the input it is.
    pub triangle_source: Vec<u32>,
    /// The triangle count that was asked for.
    pub target: usize,
    /// Triangles the input had, and how many of its vertices were locked.
    pub triangles_before: usize,
    pub locked: usize,
}

impl Decimated {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Whether the mesh was brought down to the count asked for. When it was
    /// not, every collapse left would have torn, pinched or folded the
    /// surface, or moved a locked vertex.
    pub fn reached(&self) -> bool {
        self.triangle_count() <= self.target
    }
}

/// The sum of squared distances to a set of planes, each weighted, as the ten
/// numbers of a symmetric 4 x 4 matrix.
#[derive(Clone, Copy, Debug, Default)]
struct Quadric {
    xx: f64,
    xy: f64,
    xz: f64,
    xd: f64,
    yy: f64,
    yz: f64,
    yd: f64,
    zz: f64,
    zd: f64,
    dd: f64,
}

impl Quadric {
    /// The plane through `point` with unit normal `n`, weighted by `weight`.
    fn plane(n: DVec3, point: DVec3, weight: f64) -> Quadric {
        let d = -n.dot(point);
        Quadric {
            xx: weight * n.x * n.x,
            xy: weight * n.x * n.y,
            xz: weight * n.x * n.z,
            xd: weight * n.x * d,
            yy: weight * n.y * n.y,
            yz: weight * n.y * n.z,
            yd: weight * n.y * d,
            zz: weight * n.z * n.z,
            zd: weight * n.z * d,
            dd: weight * d * d,
        }
    }

    fn add(&mut self, other: &Quadric) {
        self.xx += other.xx;
        self.xy += other.xy;
        self.xz += other.xz;
        self.xd += other.xd;
        self.yy += other.yy;
        self.yz += other.yz;
        self.yd += other.yd;
        self.zz += other.zz;
        self.zd += other.zd;
        self.dd += other.dd;
    }

    fn plus(mut self, other: &Quadric) -> Quadric {
        self.add(other);
        self
    }

    /// The weighted sum of squared distances from `p` to the planes.
    fn at(&self, p: DVec3) -> f64 {
        let x = self.xx * p.x + self.xy * p.y + self.xz * p.z + self.xd;
        let y = self.xy * p.x + self.yy * p.y + self.yz * p.z + self.yd;
        let z = self.xz * p.x + self.yz * p.y + self.zz * p.z + self.zd;
        (p.x * x + p.y * y + p.z * z + self.xd * p.x + self.yd * p.y + self.zd * p.z + self.dd).max(0.0)
    }

    /// The point where the sum is least, when the planes pin one down: three
    /// or more of them, not all through one line. `None` on flat or
    /// cylindrical ground, where a whole plane or line of points is as good.
    fn least(&self) -> Option<DVec3> {
        let (a, b, c) = (self.xx, self.xy, self.xz);
        let (d, e, f) = (self.yy, self.yz, self.zz);
        // Cofactors of the symmetric 3 x 3 part.
        let (c00, c01, c02) = (d * f - e * e, c * e - b * f, b * e - c * d);
        let det = a * c00 + b * c01 + c * c02;
        let size = a.abs().max(d.abs()).max(f.abs());
        if !(det.abs() > 1e-10 * size * size * size) {
            return None;
        }
        let (c11, c12, c22) = (a * f - c * c, b * c - a * e, a * d - b * b);
        let r = -DVec3::new(self.xd, self.yd, self.zd);
        let p = DVec3::new(
            c00 * r.x + c01 * r.y + c02 * r.z,
            c01 * r.x + c11 * r.y + c12 * r.z,
            c02 * r.x + c12 * r.y + c22 * r.z,
        ) / det;
        p.is_finite().then_some(p)
    }
}

/// An edge waiting to be collapsed. The versions say which state of its two
/// ends the cost was worked out for: an entry whose end has changed since is
/// stale, and is passed over when it comes up.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    cost: f64,
    a: u32,
    b: u32,
    version_a: u32,
    version_b: u32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Candidate) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Candidate) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    /// The cheapest first out of a heap that gives its greatest; then by
    /// vertex number, so that equal costs come out in one order always.
    fn cmp(&self, other: &Candidate) -> Ordering {
        other.cost.total_cmp(&self.cost).then_with(|| (other.a, other.b).cmp(&(self.a, self.b)))
    }
}

/// The mesh while it is being collapsed.
struct Work<'a> {
    /// About the middle of the mesh's box, so the quadrics are of small numbers.
    positions: Vec<DVec3>,
    triangles: Vec<[u32; 3]>,
    live_triangle: Vec<bool>,
    /// The triangles on each vertex; dead ones are cleared out as they are met.
    fans: Vec<Vec<u32>>,
    quadrics: Vec<Quadric>,
    live_vertex: Vec<bool>,
    version: Vec<u32>,
    /// On an open border.
    border: Vec<bool>,
    /// Moved from where the input had it.
    shifted: Vec<bool>,
    locked: &'a [bool],
    values: Option<Vec<f32>>,
    live_triangles: usize,
    tie: f64,
    /// Scratch: which vertices are neighbours of the one being looked at,
    /// marked with the number of the look.
    seen: Vec<u32>,
    look: u32,
}

/// Where the vertex an edge collapses to goes, what it costs, how far along
/// the edge from `a` to `b` that is (for the value carried), and which end is
/// the one that stays.
#[derive(Clone, Copy)]
struct Place {
    at: DVec3,
    cost: f64,
    along: f64,
    keep_b: bool,
}

impl Work<'_> {
    fn is_locked(&self, v: u32) -> bool {
        self.locked.get(v as usize).copied().unwrap_or(false)
    }

    /// Where edge `a`-`b` would collapse to; `None` when both ends are locked.
    fn place(&self, a: u32, b: u32) -> Option<Place> {
        let (pa, pb) = (self.positions[a as usize], self.positions[b as usize]);
        let sum = self.quadrics[a as usize].plus(&self.quadrics[b as usize]);
        let (edge, length_sq) = (pb - pa, (pb - pa).length_squared());
        let along_of = |p: DVec3| if length_sq > 0.0 { ((p - pa).dot(edge) / length_sq).clamp(0.0, 1.0) } else { 0.5 };
        // An end that is locked stays, where it is. Of two free ends, one on
        // a border and the other not, the border's stays where it is: a
        // border is only ever moved along itself. Else the lower number
        // stays, at the cheapest point.
        let (border_a, border_b) = (self.border[a as usize], self.border[b as usize]);
        let (at, keep_b) = match (self.is_locked(a), self.is_locked(b)) {
            (true, true) => return None,
            (true, false) => (pa, false),
            (false, true) => (pb, true),
            (false, false) if border_a != border_b => (if border_b { pb } else { pa }, border_b),
            (false, false) => {
                let middle = (pa + pb) * 0.5;
                let near = |p: &DVec3| (*p - middle).length_squared() <= FARTHEST_PLACE * FARTHEST_PLACE * length_sq;
                let at = match sum.least().filter(near) {
                    Some(least) => least,
                    // The cheapest of the two ends and the middle; the middle on a tie.
                    None => {
                        let cost = |p: DVec3| sum.at(p);
                        let end = if cost(pb) < cost(pa) { pb } else { pa };
                        if cost(end) < cost(middle) { end } else { middle }
                    }
                };
                (at, false)
            }
        };
        Some(Place { at, cost: sum.at(at) + self.tie * length_sq, along: along_of(at), keep_b })
    }

    fn candidate(&self, a: u32, b: u32) -> Option<Candidate> {
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        let place = self.place(a, b)?;
        Some(Candidate {
            cost: place.cost,
            a,
            b,
            version_a: self.version[a as usize],
            version_b: self.version[b as usize],
        })
    }

    /// Drops the dead triangles from a vertex's fan.
    fn tidy(&mut self, v: u32) {
        let live = &self.live_triangle;
        self.fans[v as usize].retain(|&t| live[t as usize]);
    }

    /// A fresh mark for `seen`.
    fn next_look(&mut self) -> u32 {
        self.look = self.look.wrapping_add(1);
        if self.look == 0 {
            self.seen.fill(0);
            self.look = 1;
        }
        self.look
    }

    /// The neighbours of `v` (its fan must be tidy), each once.
    fn neighbours(&mut self, v: u32, out: &mut Vec<u32>) {
        out.clear();
        let look = self.next_look();
        for &t in &self.fans[v as usize] {
            for w in self.triangles[t as usize] {
                if w != v && self.seen[w as usize] != look {
                    self.seen[w as usize] = look;
                    out.push(w);
                }
            }
        }
    }

    /// Whether collapsing edge `keep`-`gone` onto `at` leaves the surface the
    /// surface it was: same connectivity, no triangle turned over or
    /// flattened. Both fans must be tidy.
    fn sound(&mut self, keep: u32, gone: u32, at: DVec3, scratch: &mut Vec<u32>) -> bool {
        // The triangles on the edge itself, and the vertex across each.
        let mut across = [u32::MAX; 2];
        let mut shared = 0;
        for &t in &self.fans[gone as usize] {
            let corners = self.triangles[t as usize];
            if corners.contains(&keep) {
                if shared == 2 {
                    // Three sheets on one edge: not a surface here to begin with.
                    return false;
                }
                across[shared] = corners.into_iter().find(|&w| w != keep && w != gone).unwrap_or(u32::MAX);
                shared += 1;
            }
        }
        if shared == 0 {
            return false;
        }
        let (border_keep, border_gone) = (self.border[keep as usize], self.border[gone as usize]);
        // An edge across the inside of the mesh between two border vertices:
        // closing it would pinch two stretches of border together.
        if shared == 2 && border_keep && border_gone {
            return false;
        }
        // The link condition: the two ends may share no neighbour but the
        // vertices across the edge. A third one is a ring of three edges with
        // no triangle in it, a neck, which the collapse would close into a
        // pair of triangles back to back.
        self.neighbours(keep, scratch);
        let look = self.look;
        let mut common = 0;
        for &t in &self.fans[gone as usize] {
            for w in self.triangles[t as usize] {
                // Counted once: the mark is taken off as it is counted (no look is numbered 0).
                if w != gone && w != keep && self.seen[w as usize] == look {
                    self.seen[w as usize] = 0;
                    common += 1;
                }
            }
        }
        if common != shared {
            return false;
        }
        // Nothing left with too few triangles to be a surface: the vertices
        // across the edge each lose one, and the merged vertex has the two
        // fans less the edge's own, twice.
        let fewest = |on_border: bool| if on_border { 1 } else { 3 };
        for &w in &across[..shared] {
            if w == u32::MAX {
                return false;
            }
            self.tidy(w);
            if self.fans[w as usize].len() - 1 < fewest(self.border[w as usize]) {
                return false;
            }
        }
        let merged = self.fans[keep as usize].len() + self.fans[gone as usize].len() - 2 * shared;
        if merged < fewest(border_keep || border_gone) {
            return false;
        }
        // No triangle that stays may turn over, or lose its area.
        for (moved, other) in [(keep, gone), (gone, keep)] {
            for &t in &self.fans[moved as usize] {
                let corners = self.triangles[t as usize];
                if corners.contains(&other) {
                    continue;
                }
                let was = corners.map(|w| self.positions[w as usize]);
                let now = corners.map(|w| if w == moved { at } else { self.positions[w as usize] });
                let (before, after) = ((was[1] - was[0]).cross(was[2] - was[0]), (now[1] - now[0]).cross(now[2] - now[0]));
                let (size_before, size_after) = (before.length(), after.length());
                // A triangle that had no area to begin with has no way to turn.
                if size_before > 0.0 && !(before.dot(after) > MOST_TURN * size_before * size_after) {
                    return false;
                }
            }
        }
        true
    }

    /// Collapses edge `keep`-`gone` to the point `at`, which is
    /// `along_from_keep` of the way from the one to the other. Both fans
    /// must be tidy, and the collapse [`Self::sound`].
    fn collapse(&mut self, keep: u32, gone: u32, at: DVec3, along_from_keep: f64) {
        let (k, g) = (keep as usize, gone as usize);
        self.shifted[k] |= self.positions[k] != at;
        self.positions[k] = at;
        let other = self.quadrics[g];
        self.quadrics[k].add(&other);
        if let Some(values) = &mut self.values {
            let t = along_from_keep as f32;
            values[k] = values[k] + (values[g] - values[k]) * t;
        }
        self.border[k] |= self.border[g];
        let fan = std::mem::take(&mut self.fans[g]);
        for t in fan {
            let corners = &mut self.triangles[t as usize];
            if corners.contains(&keep) {
                self.live_triangle[t as usize] = false;
                self.live_triangles -= 1;
            } else {
                for corner in corners.iter_mut().filter(|c| **c == gone) {
                    *corner = keep;
                }
                self.fans[k].push(t);
            }
        }
        self.tidy(keep);
        self.live_vertex[g] = false;
        self.version[k] = self.version[k].wrapping_add(1);
    }
}

/// Each edge of a triangle list once, as `(lower vertex, higher vertex, how
/// many triangles are on it, one of those triangles)`, in order.
fn edges_of(triangles: &[[u32; 3]]) -> Vec<(u32, u32, u32, u32)> {
    let mut sides: Vec<(u64, u32)> = triangles
        .par_iter()
        .enumerate()
        .flat_map_iter(|(t, corners)| {
            (0..3).map(move |i| {
                let (a, b) = (corners[i], corners[(i + 1) % 3]);
                let (low, high) = if a < b { (a, b) } else { (b, a) };
                ((low as u64) << 32 | high as u64, t as u32)
            })
        })
        .collect();
    sides.par_sort_unstable();
    let mut edges = Vec::with_capacity(sides.len() / 2 + 1);
    let mut i = 0;
    while i < sides.len() {
        let mut j = i + 1;
        while j < sides.len() && sides[j].0 == sides[i].0 {
            j += 1;
        }
        let key = sides[i].0;
        edges.push(((key >> 32) as u32, key as u32, (j - i) as u32, sides[i].1));
        i = j;
    }
    edges
}

/// Decimates a mesh to about `target` triangles.
///
/// `locked` says which vertices may be neither moved nor removed; it may be
/// shorter than the vertex list, or empty, and a vertex it does not reach is
/// free. `values` is one number for each vertex to carry onto the result (a
/// mask). `progress` is called now and then with how far along the work is,
/// from 0 to 1, and returns whether to carry on: `None` comes back if it
/// says no, and nothing has been changed (there is nothing to change: the
/// input is only read).
///
/// Triangles that name a vertex twice, or one the mesh does not have, are
/// left out of the result, as they are when a mesh is built. A mesh that
/// already has `target` triangles or fewer comes back as it is.
pub fn decimate(
    positions: &[Vec3],
    indices: &[u32],
    values: Option<&[f32]>,
    locked: &[bool],
    target: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Option<Decimated> {
    let vertex_count = positions.len();
    let named = |corners: &[u32]| corners.iter().all(|&v| (v as usize) < vertex_count);
    let distinct = |c: &[u32]| c[0] != c[1] && c[1] != c[2] && c[0] != c[2];
    let (triangles, sources): (Vec<[u32; 3]>, Vec<u32>) = indices
        .chunks_exact(3)
        .enumerate()
        .filter(|(_, c)| named(c) && distinct(c))
        .map(|(t, c)| ([c[0], c[1], c[2]], t as u32))
        .unzip();
    let triangles_before = triangles.len();
    let target = target.clamp(FEWEST, FEWEST.max(triangles_before));
    let locked_count = locked.iter().take(vertex_count).filter(|&&l| l).count();

    // About the middle of the box, in double: the quadrics are sums of
    // products of coordinates, and a model far from its own centre would
    // spend its digits on the distance.
    let (low, high) = positions.iter().fold((DVec3::INFINITY, DVec3::NEG_INFINITY), |(low, high), p| {
        let p = p.as_dvec3();
        (low.min(p), high.max(p))
    });
    let middle = if vertex_count > 0 && (low + high).is_finite() { (low + high) * 0.5 } else { DVec3::ZERO };
    let moved: Vec<DVec3> = positions.par_iter().map(|p| p.as_dvec3() - middle).collect();

    let mut fans: Vec<Vec<u32>> = {
        let mut degree = vec![0u32; vertex_count];
        for corners in &triangles {
            for &v in corners {
                degree[v as usize] += 1;
            }
        }
        degree.iter().map(|&d| Vec::with_capacity(d as usize + 2)).collect()
    };
    for (t, corners) in triangles.iter().enumerate() {
        for &v in corners {
            fans[v as usize].push(t as u32);
        }
    }

    // Each triangle's plane, weighted by its area, on its three corners.
    let planes: Vec<Option<(DVec3, f64)>> = triangles
        .par_iter()
        .map(|corners| {
            let [a, b, c] = corners.map(|v| moved[v as usize]);
            let normal = (b - a).cross(c - a);
            let twice_area = normal.length();
            (twice_area > 0.0 && twice_area.is_finite()).then(|| (normal / twice_area, twice_area * 0.5))
        })
        .collect();
    let mut quadrics = vec![Quadric::default(); vertex_count];
    let mut area = 0.0;
    for (corners, plane) in triangles.iter().zip(&planes) {
        let Some((normal, weight)) = plane else { continue };
        area += weight;
        let quadric = Quadric::plane(*normal, moved[corners[0] as usize], *weight);
        for &v in corners {
            quadrics[v as usize].add(&quadric);
        }
    }

    // Open borders: which vertices are on one, and a plane through each open
    // edge, square to its triangle, to hold the border's line.
    let edges = edges_of(&triangles);
    let mut border = vec![false; vertex_count];
    for &(a, b, count, triangle) in &edges {
        if count != 1 {
            continue;
        }
        (border[a as usize], border[b as usize]) = (true, true);
        let Some((normal, _)) = planes[triangle as usize] else { continue };
        let (pa, pb) = (moved[a as usize], moved[b as usize]);
        let across = (pb - pa).cross(normal);
        let length_sq = (pb - pa).length_squared();
        if across.length_squared() > 0.0 {
            let quadric = Quadric::plane(across.normalize(), pa, BORDER_WEIGHT * length_sq);
            quadrics[a as usize].add(&quadric);
            quadrics[b as usize].add(&quadric);
        }
    }

    let mean_area = if triangles_before > 0 { area / triangles_before as f64 } else { 0.0 };
    let mut work = Work {
        positions: moved,
        live_triangle: vec![true; triangles_before],
        triangles,
        fans,
        quadrics,
        live_vertex: vec![true; vertex_count],
        version: vec![0; vertex_count],
        border,
        shifted: vec![false; vertex_count],
        locked,
        values: values.map(|v| {
            let mut carried = v.to_vec();
            carried.resize(vertex_count, 0.0);
            carried
        }),
        live_triangles: triangles_before,
        tie: LENGTH_TIE * mean_area,
        seen: vec![0; vertex_count],
        look: 0,
    };

    let to_remove = triangles_before.saturating_sub(target).max(1);
    let report = |work: &Work| ((triangles_before - work.live_triangles) as f32 / to_remove as f32).min(1.0);
    if !progress(0.0) {
        return None;
    }

    let mut heap: BinaryHeap<Candidate> = {
        let work = &work;
        let first: Vec<Candidate> =
            edges.par_iter().filter_map(|&(a, b, _, _)| work.candidate(a, b)).collect();
        BinaryHeap::from(first)
    };
    drop(edges);

    let (mut near, mut since_report, mut since_refill) = (Vec::new(), 0usize, 0usize);
    while work.live_triangles > target {
        let Some(next) = heap.pop() else {
            // Out of edges with the count not reached. A collapse that was
            // turned down may be sound now that its surroundings have
            // changed: look at every edge once more, and stop when a whole
            // round of that collapses nothing.
            if since_refill == 0 {
                break;
            }
            since_refill = 0;
            for v in 0..vertex_count as u32 {
                if !work.live_vertex[v as usize] {
                    continue;
                }
                work.tidy(v);
                work.neighbours(v, &mut near);
                heap.extend(near.iter().filter(|&&w| w > v).filter_map(|&w| work.candidate(v, w)));
            }
            continue;
        };
        let (a, b) = (next.a, next.b);
        let fresh = work.live_vertex[a as usize]
            && work.live_vertex[b as usize]
            && work.version[a as usize] == next.version_a
            && work.version[b as usize] == next.version_b;
        if !fresh {
            continue;
        }
        let Some(place) = work.place(a, b) else { continue };
        let (keep, gone, along) = if place.keep_b { (b, a, 1.0 - place.along) } else { (a, b, place.along) };
        work.tidy(keep);
        work.tidy(gone);
        if !work.sound(keep, gone, place.at, &mut near) {
            continue;
        }
        work.collapse(keep, gone, place.at, along);
        since_refill += 1;

        work.neighbours(keep, &mut near);
        heap.extend(near.iter().filter_map(|&w| work.candidate(keep, w)));
        since_report += 1;
        if since_report == PROGRESS_EVERY {
            since_report = 0;
            if !progress(report(&work)) {
                return None;
            }
        }
    }

    // What is left, its vertices numbered as its triangles first name them.
    let mut number = vec![u32::MAX; vertex_count];
    let mut out_positions = Vec::new();
    let mut out_values = work.values.as_ref().map(|_| Vec::new());
    let mut out_indices = Vec::with_capacity(work.live_triangles * 3);
    let mut triangle_source = Vec::with_capacity(work.live_triangles);
    for (t, corners) in work.triangles.iter().enumerate().filter(|(t, _)| work.live_triangle[*t]) {
        for &v in corners {
            let v = v as usize;
            if number[v] == u32::MAX {
                number[v] = out_positions.len() as u32;
                // A vertex that was never moved is the very point it was.
                out_positions.push(if work.shifted[v] { (work.positions[v] + middle).as_vec3() } else { positions[v] });
                if let (Some(out), Some(values)) = (&mut out_values, &work.values) {
                    out.push(values[v]);
                }
            }
            out_indices.push(number[v]);
        }
        triangle_source.push(sources[t]);
    }
    progress(1.0);
    Some(Decimated {
        positions: out_positions,
        indices: out_indices,
        values: out_values,
        triangle_source,
        target,
        triangles_before,
        locked: locked_count,
    })
}

#[cfg(test)]
mod tests;
