//! Turns a rig into a mesh: the "skinning" step.
//!
//! Approach: build an analytic signed distance field out of the rig, extract
//! its surface with Surface Nets (as the voxel remesher does), then fair the
//! result and snap every vertex back onto the exact field. Field-based rather
//! than tubes stitched together because a surface extracted from a field
//! cannot self-intersect however tangled the rig is, joints of any number of
//! branches need no special handling, and the result is an even mesh that is
//! sculptable straight away.
//!
//! The field is built from limbs, not from spheres and links:
//!
//!   - A limb is a run of spheres the skin should read as one continuous
//!     form: a chain through every sphere with a single child, carried on
//!     through a branching sphere into whichever child continues its
//!     direction (the neck out of the chest, not an arm), and through a root
//!     into the opposite child when two leave it roughly back to back. Every
//!     other child starts a new limb, a branch.
//!
//!   - Along a limb the centre line is a Hermite spline through the sphere
//!     centres and the radius a shape-preserving (monotone) cubic through the
//!     sphere radii, so a chain bends in a curve and tapers in a swell rather
//!     than as straight cones with a kink at every sphere. The sweep is
//!     sampled as a run of short round cones, each the exact convex hull of
//!     two spheres.
//!
//!   - Within a limb the pieces are joined with a hard minimum. A smooth
//!     minimum of two surfaces that coincide swells outward, and along a
//!     chain a link and its end spheres coincide over the whole joint, so
//!     smoothing there puts a bead at every sphere. Only the inside of a bend
//!     is filleted, by an amount that grows with how sharp the bend is.
//!
//!   - Limbs are smooth-joined into each other, parents before children,
//!     with a fillet sized off the branch's own thickness (a finger blends
//!     into a hand like a finger) and faded out behind the sphere the branch
//!     grows from, where a fillet could only swell a knob on the far side.
//!
//! Exact symmetry: with symmetry on, the lattice is laid out as its own
//! mirror image across x = 0, only the +x half is sampled and then copied
//! across, and every vertex is paired with the one in the mirrored cell and
//! re-symmetrised after each step that moves it.

use std::collections::{HashMap, VecDeque};
use std::f32::consts::FRAC_PI_2;
use std::time::Instant;

use glam::Vec3;
use rayon::prelude::*;

use super::rig::Rig;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SkinSettings {
    /// Surface detail as a multiple of "fine enough for the thinnest sphere
    /// in the rig". 1 is a sensible default at any scale: the detail a rig
    /// needs depends on how small its smallest limb is relative to the whole.
    pub density: f32,
    /// Fillet width where a branch meets its parent limb (and inside sharp
    /// bends), as a fraction of the branch's own radius. 0 gives a hard
    /// crease; about 0.45 is organic.
    pub blend: f32,
    /// Fairing passes before the surface is snapped back onto the field.
    pub smoothing: u32,
    /// How much a chain curves and swells between its spheres. 0 is straight
    /// tapered links; 1 is a full spline through the centres.
    pub curve: f32,
}

impl Default for SkinSettings {
    fn default() -> Self {
        Self { density: 1.0, blend: 0.45, smoothing: 3, curve: 0.75 }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quality {
    /// While the rig is being dragged: coarse and cheap, good enough to judge
    /// a shape by.
    Draft,
    /// The settled viewport skin: capped so a detailed creature does not
    /// hitch the viewport on every release.
    Preview,
    /// Make Skin: the full detail the rig asks for.
    Final,
}

impl Quality {
    /// (detail scale, most cells along the longest side, most lattice samples).
    /// The sample budget matters as much as the cap: resolution counts along
    /// one axis, so a long thin rig is a modest grid where a compact one at
    /// the same number is huge. The resolution is walked down to fit.
    fn limits(self) -> (f32, u32, usize) {
        match self {
            Quality::Draft => (0.55, 96, 1_500_000),
            Quality::Preview => (1.0, 224, 12_000_000),
            Quality::Final => (1.0, 640, 64_000_000),
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct SkinStats {
    pub triangles: usize,
    pub resolution: u32,
    /// Spacing of the lattice the surface was extracted on.
    pub cell: f32,
    pub ms: f32,
    /// How the field was built up; reported by the tests.
    #[allow(dead_code)]
    pub limbs: usize,
    #[allow(dead_code)]
    pub pieces: usize,
}

/// A finished skin: a closed triangle mesh with a normal per vertex.
pub struct Skin {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub indices: Vec<u32>,
    pub stats: SkinStats,
}

pub const MIN_DENSITY: f32 = 0.35;
pub const MAX_DENSITY: f32 = 2.5;

const MIN_RESOLUTION: u32 = 16;
/// Spare samples beyond the field's bounds, so the surface always closes
/// inside the grid.
const GRID_PAD: i32 = 2;
/// Voxels wanted across the smallest sphere's radius. Below about 2 a sphere
/// starts reading as a cube.
const CELLS_PER_MIN_RADIUS: f32 = 2.5;

const TAUBIN_LAMBDA: f32 = 0.5;
const TAUBIN_MU: f32 = -0.53;

/// A limb carries on through a sphere only while it turns by less than this.
/// Past it the chain is doubling back, and the far side is better treated as
/// a branch with a proper fillet in the fold.
const CORNER_ANGLE_DEG: f32 = 115.0;
/// A root carries one limb straight through it only when two of its children
/// leave it within this of dead opposite. Two legs splayed from a pelvis are
/// not that: joined, they make a bar through the hips with a ledge all round
/// the waist.
const ROOT_PAIR_ANGLE_DEG: f32 = 45.0;
/// The spline is sampled so no piece turns more than this against the next.
const SAMPLE_TURN_DEG: f32 = 5.0;
/// The centre line curves fully through turns up to the first angle and not
/// at all past the second.
const CURVE_FADE_START_DEG: f32 = 20.0;
const CURVE_FADE_END_DEG: f32 = 70.0;
const MAX_PIECES_PER_LINK: usize = 24;
/// Where a branch's fillet reaches full width, as a fraction of the radius
/// of the sphere it grows from, measured from that sphere's centre.
const FADE_LENGTH_FRACTION: f32 = 0.5;
/// Fillet inside a right-angle bend, relative to the blend setting.
const BEND_BLEND_GAIN: f32 = 2.0;
/// Fillet where a branch meets its parent, relative to blend x its radius.
const BRANCH_BLEND_GAIN: f32 = 2.0;

const PROJECTION_STEPS: usize = 2;
const GRADIENT_PROBE_CELLS: f32 = 0.1;
/// Projection lookup bins, in lattice cells per side.
const BIN_CELLS: usize = 4;
/// Outside every piece's reach only the sign is ever read.
const FAR: f32 = 1e6;
const NO_TWIN: u32 = u32::MAX;

fn direction(from: Vec3, to: Vec3) -> Vec3 {
    let d = to - from;
    let m = d.length();
    if m > 1e-9 { d / m } else { Vec3::ZERO }
}

fn angle_between(a: Vec3, b: Vec3) -> f32 {
    a.dot(b).clamp(-1.0, 1.0).acos()
}

fn hermite(p0: Vec3, m0: Vec3, p1: Vec3, m1: Vec3, t: f32) -> Vec3 {
    let (t2, t3) = (t * t, t * t * t);
    p0 * (2.0 * t3 - 3.0 * t2 + 1.0) + m0 * (t3 - 2.0 * t2 + t) + p1 * (-2.0 * t3 + 3.0 * t2) + m1 * (t3 - t2)
}

fn hermite_value(v0: f32, m0: f32, v1: f32, m1: f32, t: f32) -> f32 {
    let (t2, t3) = (t * t, t * t * t);
    v0 * (2.0 * t3 - 3.0 * t2 + 1.0) + m0 * (t3 - 2.0 * t2 + t) + v1 * (-2.0 * t3 + 3.0 * t2) + m1 * (t3 - t2)
}

/// Cubic polynomial smooth minimum. More than `k` apart this is exactly
/// min(a, b); within `k` it subtracts a bulge of at most k/6. Its curvature
/// is continuous, so the start of a fillet leaves no seam for a highlight
/// to find.
#[inline]
fn smooth_union(a: f32, b: f32, k: f32) -> f32 {
    let low = a.min(b);
    if k <= 0.0 {
        return low;
    }
    let h = k - (a - b).abs();
    if h <= 0.0 {
        return low;
    }
    let h = h / k;
    low - h * h * h * k * (1.0 / 6.0)
}

/// How much of a fillet applies at `p`: 0 behind `origin` (relative to
/// `dir`), rising smoothly to 1 at `length` in front of it. `length` 0 means
/// no fade.
///
/// The one rule behind every fade: a smooth union swells wherever its two
/// inputs coincide, and next to a shared sphere they coincide over its whole
/// far side. Only the region ahead of the joint has a crease to fill.
#[inline]
fn blend_fade(p: Vec3, origin: Vec3, dir: Vec3, length: f32) -> f32 {
    if length <= 0.0 {
        return 1.0;
    }
    let s = (p - origin).dot(dir) / length;
    if s <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        s * s * (3.0 - 2.0 * s)
    }
}

/// Three-valued sign. The region tests in [`Piece::distance`] rely on
/// sign(0) being 0; with a two-valued sign a cylindrical piece takes the
/// wrong branch along its whole length.
#[inline]
fn sign(v: f32) -> f32 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// One round cone of a limb's sweep, with the parts of its distance function
/// that do not depend on the sample point worked out once, plus how it folds
/// into the pieces before it.
#[derive(Clone, Copy)]
struct Piece {
    a: Vec3,
    b: Vec3,
    ra: f32,
    rb: f32,
    ba: Vec3,
    l2: f32,
    rr: f32,
    a2: f32,
    il2: f32,
    /// Coincident centres or one sphere swallowing the other: the hull is
    /// the bigger sphere.
    degenerate: bool,
    limb: u32,
    /// Fillet against the limb's earlier pieces; 0 = hard union. Non-zero
    /// only where the sweep bends, and faded out on the outside of the bend.
    k: f32,
    fade_origin: Vec3,
    fade_dir: Vec3,
    fade_length: f32,
    min: Vec3,
    max: Vec3,
}

impl Piece {
    fn new(a: Vec3, b: Vec3, ra: f32, rb: f32, limb: u32) -> Piece {
        let ba = b - a;
        let l2 = ba.dot(ba);
        let rr = ra - rb;
        let a2 = l2 - rr * rr;
        let degenerate = l2 < 1e-12 || a2 <= 1e-12;
        Piece {
            a,
            b,
            ra,
            rb,
            ba,
            l2,
            rr,
            a2,
            il2: if degenerate { 0.0 } else { 1.0 / l2 },
            degenerate,
            limb,
            k: 0.0,
            fade_origin: Vec3::ZERO,
            fade_dir: Vec3::ZERO,
            fade_length: 0.0,
            min: Vec3::ZERO,
            max: Vec3::ZERO,
        }
    }

    /// Exact signed distance to the round cone (after Inigo Quilez's
    /// branch-per-region formulation).
    #[inline]
    fn distance(&self, p: Vec3) -> f32 {
        if self.degenerate {
            return ((p - self.a).length() - self.ra).min((p - self.b).length() - self.rb);
        }
        let pa = p - self.a;
        let y = pa.dot(self.ba);
        let z = y - self.l2;
        let x = pa * self.l2 - self.ba * y;
        let x2 = x.dot(x);
        let y2 = y * y * self.l2;
        let z2 = z * z * self.l2;
        let k = sign(self.rr) * self.rr * self.rr * x2;
        if sign(z) * self.a2 * z2 > k {
            return (x2 + z2).sqrt() * self.il2 - self.rb;
        }
        if sign(y) * self.a2 * y2 < k {
            return (x2 + y2).sqrt() * self.il2 - self.ra;
        }
        ((x2 * self.a2 * self.il2).sqrt() + y * self.rr) * self.il2 - self.ra
    }

    #[inline]
    fn fold(&self, acc: f32, p: Vec3) -> f32 {
        let d = self.distance(p);
        if self.k > 0.0 {
            smooth_union(acc, d, self.k * blend_fade(p, self.fade_origin, self.fade_dir, self.fade_length))
        } else {
            acc.min(d)
        }
    }
}

#[derive(Clone, Copy)]
struct Limb {
    first: usize,
    count: usize,
    /// Fillet against everything folded before this limb.
    k: f32,
    /// For a branch: fades the fillet out behind the sphere it grows from.
    fade_origin: Vec3,
    fade_dir: Vec3,
    fade_length: f32,
    min: Vec3,
    max: Vec3,
}

impl Limb {
    #[inline]
    fn fold(&self, acc: f32, value: f32, p: Vec3) -> f32 {
        smooth_union(acc, value, self.k * blend_fade(p, self.fade_origin, self.fade_dir, self.fade_length))
    }
}

/// The rig as a distance field: its pieces, grouped into limbs in the order
/// they are folded together.
struct Field {
    pieces: Vec<Piece>,
    limbs: Vec<Limb>,
}

/// Scratch for decomposing the rig into limbs. One entry per sphere as it
/// appears in the world (a node, or a node's reflection).
struct LimbBuilder {
    pos: Vec<Vec3>,
    radius: Vec<f32>,
    /// A sphere on the mirror plane under symmetry: its own reflection.
    shared: Vec<bool>,
    parents: Vec<u32>,
    parent: Vec<usize>,
    children: Vec<Vec<usize>>,
    /// Which child, if any, carries each sphere's limb on.
    next: Vec<Option<usize>>,
    /// For a root: the two children one limb runs straight through it into.
    pair: Vec<Option<(usize, usize)>>,
    covered: Vec<bool>,
    processed: Vec<bool>,
    chain: Vec<usize>,
    queue: VecDeque<usize>,
    sample_pos: Vec<Vec3>,
    sample_radius: Vec<f32>,
    curve: f32,
    blend: f32,
    turn_step: f32,
    field: Field,
}

impl Field {
    /// Decomposes the rig into limbs and samples each one's sweep into
    /// pieces, in the order they will be folded: every limb after the limb
    /// its first sphere belongs to.
    fn build(rig: &Rig, symmetry: bool, settings: &SkinSettings, quality: Quality) -> Field {
        let (spheres, links) = rig.geometry(symmetry);
        let nodes = rig.nodes();
        let count = spheres.len();
        let index_of: HashMap<(usize, bool), usize> =
            spheres.iter().enumerate().map(|(i, s)| ((s.node, s.mirrored), i)).collect();

        let mut b = LimbBuilder {
            pos: spheres.iter().map(|s| s.centre).collect(),
            radius: spheres.iter().map(|s| s.radius).collect(),
            shared: spheres.iter().map(|s| symmetry && nodes[s.node].on_axis()).collect(),
            parents: vec![0; count],
            parent: vec![usize::MAX; count],
            children: vec![Vec::new(); count],
            next: vec![None; count],
            pair: vec![None; count],
            covered: vec![false; count],
            processed: vec![false; count],
            chain: Vec::new(),
            queue: VecDeque::new(),
            sample_pos: Vec::new(),
            sample_radius: Vec::new(),
            curve: settings.curve.clamp(0.0, 1.0),
            blend: settings.blend.max(0.0),
            turn_step: SAMPLE_TURN_DEG * if quality == Quality::Draft { 2.0 } else { 1.0 },
            field: Field { pieces: Vec::new(), limbs: Vec::new() },
        };

        for link in &links {
            let child = &nodes[link.child];
            let Some(parent_node) = child.parent else { continue };
            let parent_mirrored = link.mirrored && !nodes[parent_node].on_axis();
            let child_mirrored = link.mirrored && !child.on_axis();
            let (Some(&p), Some(&c)) =
                (index_of.get(&(parent_node, parent_mirrored)), index_of.get(&(link.child, child_mirrored)))
            else {
                continue;
            };
            b.children[p].push(c);
            b.parents[c] += 1;
            b.parent[c] = p;
        }

        let corner_cos = CORNER_ANGLE_DEG.to_radians().cos();
        for u in 0..count {
            if b.children[u].is_empty() {
                continue;
            }
            if b.parents[u] == 1 {
                let dir_in = direction(b.pos[b.parent[u]], b.pos[u]);
                let mut best = None;
                let mut best_dot = corner_cos;
                for &c in &b.children[u] {
                    let d = dir_in.dot(direction(b.pos[u], b.pos[c]));
                    if d >= best_dot {
                        best_dot = d;
                        best = Some(c);
                    }
                }
                // A sphere on the mirror plane cannot pick one side's child
                // to continue into: its reflection ties with it exactly, and
                // choosing either makes the skin lopsided. Both branch.
                if best.is_some_and(|c| b.shared[u] && !b.shared[c]) {
                    best = None;
                }
                b.next[u] = best;
            } else if b.parents[u] == 0 && b.children[u].len() >= 2 {
                // A root with two children leaving back to back (a bar drawn
                // out both ways from its middle) is the middle of one limb.
                let mut best_dot = -ROOT_PAIR_ANGLE_DEG.to_radians().cos();
                let kids = &b.children[u];
                for x in 0..kids.len() {
                    for y in x + 1..kids.len() {
                        let (ca, cb) = (kids[x], kids[y]);
                        let mirror_pair =
                            spheres[ca].node == spheres[cb].node && spheres[ca].mirrored != spheres[cb].mirrored;
                        if b.shared[u] && !mirror_pair && !(b.shared[ca] && b.shared[cb]) {
                            continue;
                        }
                        let d = direction(b.pos[u], b.pos[ca]).dot(direction(b.pos[u], b.pos[cb]));
                        if d <= best_dot {
                            best_dot = d;
                            b.pair[u] = Some((ca, cb));
                        }
                    }
                }
            }
        }

        // Emit limbs breadth-first from the roots, so every limb is folded
        // after the one its first sphere belongs to.
        b.queue.extend((0..count).filter(|&u| b.parents[u] == 0));
        loop {
            while let Some(u) = b.queue.pop_front() {
                if b.processed[u] {
                    continue;
                }
                b.processed[u] = true;
                if b.parents[u] == 0 && !b.covered[u] {
                    b.chain.clear();
                    if let Some((first, second)) = b.pair[u] {
                        b.append_chain(first);
                        b.chain.reverse();
                        b.chain.push(u);
                        b.append_chain(second);
                        b.emit_limb(false);
                    } else if b.children[u].is_empty() {
                        b.chain.push(u);
                        b.emit_limb(false);
                    }
                }
                for k in 0..b.children[u].len() {
                    let c = b.children[u][k];
                    if b.next[u] == Some(c) || b.pair[u].is_some_and(|(x, y)| c == x || c == y) {
                        continue;
                    }
                    b.chain.clear();
                    b.chain.push(u);
                    b.append_chain(c);
                    // The first limb out of an unpaired root owns the root
                    // sphere; every later one is a branch off it.
                    b.emit_limb(b.covered[u]);
                }
            }
            // Anything a malformed graph left unreached still gets skinned.
            let Some(stray) = (0..count).find(|&u| !b.covered[u] && !b.processed[u]) else { break };
            b.parents[stray] = 0;
            b.queue.push_back(stray);
        }
        b.field
    }

    /// Bounds of every piece, and the widest fillet.
    fn bounds(&self) -> (Vec3, Vec3, f32) {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut max_k = 0.0f32;
        for p in &self.pieces {
            min = min.min((p.a - p.ra).min(p.b - p.rb));
            max = max.max((p.a + p.ra).max(p.b + p.rb));
            max_k = max_k.max(p.k);
        }
        for l in &self.limbs {
            max_k = max_k.max(l.k);
        }
        (min, max, max_k)
    }

    /// Fills the lattice from column `from_x` up. Each limb's pieces are
    /// combined into a per-slice scratch region, and each limb then folded
    /// into the lattice.
    ///
    /// Scatters per piece rather than gathering per sample, so the work
    /// follows the volume the rig occupies. Exact where it matters: a fold
    /// can only differ from a plain minimum within its fillet width, so
    /// growing each piece's box by (fillet + 2 cells) covers every sample
    /// near the surface whose value could change.
    fn sample(&self, grid: &Grid, from_x: usize) -> Vec<f32> {
        let [sx, sy, sz] = grid.n;
        let slice = sx * sy;
        let margin = Vec3::splat(2.0 * grid.h);
        let range = |min: Vec3, max: Vec3| {
            let mut lo = [0; 3];
            let mut hi = [0; 3];
            for a in 0..3 {
                lo[a] = grid.index(a, min[a] - margin[a], false);
                hi[a] = grid.index(a, max[a] + margin[a], true);
            }
            lo[0] = lo[0].max(from_x);
            (lo, hi)
        };
        let piece_box: Vec<_> = self.pieces.iter().map(|p| range(p.min, p.max)).collect();
        let limb_box: Vec<_> = self.limbs.iter().map(|l| range(l.min, l.max)).collect();

        let mut sdf = vec![FAR; slice * sz];
        sdf.par_chunks_mut(slice).enumerate().for_each_init(
            || vec![FAR; slice],
            |scratch, (z, out)| {
                let wz = grid.coord(2, z);
                for (limb, &(lo, hi)) in self.limbs.iter().zip(&limb_box) {
                    if z < lo[2] || z > hi[2] || lo[0] > hi[0] {
                        continue;
                    }
                    for y in lo[1]..=hi[1] {
                        scratch[sx * y + lo[0]..=sx * y + hi[0]].fill(FAR);
                    }
                    let mut touched = false;
                    for pi in limb.first..limb.first + limb.count {
                        let (plo, phi) = piece_box[pi];
                        if z < plo[2] || z > phi[2] || plo[0] > phi[0] {
                            continue;
                        }
                        touched = true;
                        let piece = &self.pieces[pi];
                        for y in plo[1]..=phi[1] {
                            let wy = grid.coord(1, y);
                            let row = sx * y;
                            for x in plo[0]..=phi[0] {
                                let p = Vec3::new(grid.coord(0, x), wy, wz);
                                scratch[row + x] = piece.fold(scratch[row + x], p);
                            }
                        }
                    }
                    if !touched {
                        continue;
                    }
                    for y in lo[1]..=hi[1] {
                        let wy = grid.coord(1, y);
                        let row = sx * y;
                        for x in lo[0]..=hi[0] {
                            let d = scratch[row + x];
                            if d < FAR {
                                let p = Vec3::new(grid.coord(0, x), wy, wz);
                                out[row + x] = limb.fold(out[row + x], d, p);
                            }
                        }
                    }
                }
            },
        );
        sdf
    }

    /// The field at one point, from the pieces that can reach it (in limb
    /// order): the same limb-by-limb nesting [`Self::sample`] scatters.
    fn evaluate(&self, p: Vec3, pieces: &[u32]) -> f32 {
        let mut acc = FAR;
        let mut limb_acc = FAR;
        let mut current = u32::MAX;
        for &pi in pieces {
            let piece = &self.pieces[pi as usize];
            if piece.limb != current {
                if current != u32::MAX {
                    acc = self.limbs[current as usize].fold(acc, limb_acc, p);
                }
                current = piece.limb;
                limb_acc = FAR;
            }
            limb_acc = piece.fold(limb_acc, p);
        }
        if current != u32::MAX {
            acc = self.limbs[current as usize].fold(acc, limb_acc, p);
        }
        acc
    }

    /// Moves every vertex onto the zero set of the field with a couple of
    /// Newton steps, and writes the field gradient as its normal. Extraction
    /// and fairing only get a vertex to within a fraction of a cell of the
    /// surface; this puts it on the smooth surface the rig describes, which
    /// removes the last of the stair-stepping and lets fillets and tapers
    /// keep their true curvature.
    fn project(&self, bins: &Bins, positions: &mut [Vec3], normals: &mut [Vec3], h: f32, steps: usize) {
        let probe = h * GRADIENT_PROBE_CELLS;
        positions.par_iter_mut().zip(normals.par_iter_mut()).with_min_len(256).for_each(|(position, normal)| {
            let mut p = *position;
            let mut gradient = Vec3::ZERO;
            for _ in 0..steps {
                let near = bins.at(p);
                // Tetrahedral probe: four evaluations give both the gradient
                // and (their mean) the value.
                let f0 = self.evaluate(p + Vec3::new(probe, -probe, -probe), near);
                let f1 = self.evaluate(p + Vec3::new(-probe, -probe, probe), near);
                let f2 = self.evaluate(p + Vec3::new(-probe, probe, -probe), near);
                let f3 = self.evaluate(p + Vec3::new(probe, probe, probe), near);
                if f0.max(f1).max(f2).max(f3) >= FAR * 0.5 {
                    break;
                }
                let g = Vec3::new(f0 - f1 - f2 + f3, -f0 - f1 + f2 + f3, -f0 + f1 - f2 + f3) * (0.25 / probe);
                let g2 = g.length_squared();
                if g2 < 1e-8 {
                    break;
                }
                gradient = g;
                let mut step = g * ((f0 + f1 + f2 + f3) * 0.25 / g2);
                let length = step.length();
                if length > h {
                    step *= h / length;
                }
                p -= step;
                if length < h * 1e-3 {
                    break;
                }
            }
            *position = p;
            if let Some(n) = gradient.try_normalize() {
                *normal = n;
            }
        });
    }
}

impl LimbBuilder {
    /// Follows `next` from `start`, appending every sphere to the chain and
    /// queueing each one for its own branches.
    fn append_chain(&mut self, start: usize) {
        let mut v = Some(start);
        let mut guard = self.next.len() + 1;
        while let Some(u) = v {
            if guard == 0 {
                break;
            }
            guard -= 1;
            self.chain.push(u);
            self.queue.push_back(u);
            if self.covered[u] && u != start {
                break;
            }
            v = self.next[u];
        }
    }

    /// Samples the sweep through the spheres in the chain into pieces, and
    /// records the limb.
    fn emit_limb(&mut self, branch: bool) {
        let m = self.chain.len();
        if m == 0 {
            return;
        }
        self.sample_pos.clear();
        self.sample_radius.clear();
        if m == 1 {
            self.sample_pos.push(self.pos[self.chain[0]]);
            self.sample_radius.push(self.radius[self.chain[0]]);
        } else {
            self.sample_sweep();
        }

        let limb_index = self.field.limbs.len() as u32;
        let first = self.field.pieces.len();
        let (sp, sr) = (&self.sample_pos, &self.sample_radius);
        if sp.len() == 1 {
            self.field.pieces.push(Piece::new(sp[0], sp[0], sr[0], sr[0], limb_index));
        } else {
            for j in 0..sp.len() - 1 {
                let mut piece = Piece::new(sp[j], sp[j + 1], sr[j], sr[j + 1], limb_index);
                if j > 0 {
                    // Fillet on the inside of the bend at this joint, growing
                    // with its sharpness; none on the outside, where the
                    // joint sphere already rounds it and a blend could only
                    // swell it.
                    let dir_in = direction(sp[j - 1], sp[j]);
                    let dir_out = direction(sp[j], sp[j + 1]);
                    let r = sr[j];
                    let k = self.blend * BEND_BLEND_GAIN * r * (angle_between(dir_in, dir_out) / FRAC_PI_2).min(1.0);
                    let inside = dir_out - dir_in;
                    let inside_length = inside.length();
                    if k >= r * 1e-3 && inside_length >= 1e-6 {
                        piece.k = k;
                        piece.fade_origin = sp[j];
                        piece.fade_dir = inside / inside_length;
                        piece.fade_length = FADE_LENGTH_FRACTION * r;
                    }
                }
                self.field.pieces.push(piece);
            }
        }

        let r0 = self.radius[self.chain[0]];
        let r1 = if m > 1 { self.radius[self.chain[1]] } else { r0 };
        let mut limb = Limb {
            first,
            count: self.field.pieces.len() - first,
            // Sized off the branch, not the sphere it grows from.
            k: self.blend * BRANCH_BLEND_GAIN * r0.min(r1),
            fade_origin: Vec3::ZERO,
            fade_dir: Vec3::ZERO,
            fade_length: 0.0,
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        };
        if branch && sp.len() > 1 {
            // Faded to nothing behind the sphere the branch grows from.
            limb.fade_origin = sp[0];
            limb.fade_dir = direction(sp[0], sp[1]);
            limb.fade_length = if limb.fade_dir == Vec3::ZERO { 0.0 } else { FADE_LENGTH_FRACTION * r0 };
        }
        for piece in &mut self.field.pieces[first..] {
            let reach = piece.k.max(limb.k);
            piece.min = (piece.a - piece.ra).min(piece.b - piece.rb) - reach;
            piece.max = (piece.a + piece.ra).max(piece.b + piece.rb) + reach;
            limb.min = limb.min.min(piece.min);
            limb.max = limb.max.max(piece.max);
        }
        self.field.limbs.push(limb);
        for &c in &self.chain {
            self.covered[c] = true;
        }
    }

    /// The sweep through the two or more spheres in the chain: a Hermite
    /// spline through the centres (tangents a length-weighted average of the
    /// neighbouring chords, scaled by the curve setting and eased off at
    /// sharp turns) and a monotone cubic through the radii, sampled finely
    /// enough that no two pieces meet at more than the turn step.
    ///
    /// Monotone because an ordinary cubic overshoots: a fat sphere between
    /// thin ones would pull the radius below the thin ones on either side, a
    /// pinch no sphere asked for. At a local extreme (the fat elbow) the
    /// monotone curve arrives flat, which makes the swelling round instead of
    /// pointed.
    fn sample_sweep(&mut self) {
        let m = self.chain.len();
        let spans = m - 1;
        let p = |i: usize| self.pos[self.chain[i]];
        let r = |i: usize| self.radius[self.chain[i]];
        let span: Vec<f32> = (0..spans).map(|i| p(i + 1).distance(p(i))).collect();
        let slope: Vec<f32> =
            (0..spans).map(|i| if span[i] > 1e-9 { (r(i + 1) - r(i)) / span[i] } else { 0.0 }).collect();

        let mut pos_tangent = vec![Vec3::ZERO; m];
        let mut rad_tangent = vec![0.0f32; m];
        for i in 0..m {
            if i == 0 {
                pos_tangent[i] = direction(p(0), p(1));
                rad_tangent[i] = slope[0];
            } else if i == spans {
                pos_tangent[i] = direction(p(i - 1), p(i));
                rad_tangent[i] = slope[spans - 1];
            } else {
                let (d_prev, d_next) = (span[i - 1], span[i]);
                let u_prev = direction(p(i - 1), p(i));
                let u_next = direction(p(i), p(i + 1));
                let sum = d_prev + d_next;
                let tangent = if sum > 1e-9 { (u_prev * d_next + u_next * d_prev) / sum } else { Vec3::ZERO };
                // Curve only through gentle turns. Any smooth curve forced
                // through a sharp corner has to swing wide before it: a bent
                // arm would bow outward above the elbow like a rubber hose.
                // A sharp joint is straight bones meeting at the joint
                // sphere, filleted on the inside only.
                let turn = angle_between(u_prev, u_next).to_degrees();
                let fade =
                    ((turn - CURVE_FADE_START_DEG) / (CURVE_FADE_END_DEG - CURVE_FADE_START_DEG)).clamp(0.0, 1.0);
                pos_tangent[i] = tangent * (1.0 - fade * fade * (3.0 - 2.0 * fade));

                let (s0, s1) = (slope[i - 1], slope[i]);
                rad_tangent[i] = if s0 * s1 <= 0.0 {
                    0.0
                } else {
                    let w1 = 2.0 * d_next + d_prev;
                    let w2 = d_next + 2.0 * d_prev;
                    (w1 + w2) / (w1 / s0 + w2 / s1)
                };
            }
        }

        let mut out_pos = vec![p(0)];
        let mut out_radius = vec![r(0)];
        for i in 0..spans {
            let (p0, p1) = (p(i), p(i + 1));
            let (r0, r1) = (r(i), r(i + 1));
            let d = span[i];
            let m0 = pos_tangent[i] * (self.curve * d);
            let m1 = pos_tangent[i + 1] * (self.curve * d);
            // The radius tangent is the sphere's, shared by both spans
            // meeting there, so the profile has no slope break at a sphere.
            let mr0 = rad_tangent[i] * d;
            let mr1 = rad_tangent[i + 1] * d;

            let mut pieces = 1;
            if d > 1e-9 {
                // How far the span's curve turns in total, on a probe polyline.
                const PROBE: usize = 8;
                let mut turn = 0.0;
                let mut prev = p0;
                let mut prev_dir = Vec3::ZERO;
                for s in 1..=PROBE {
                    let q = hermite(p0, m0, p1, m1, s as f32 / PROBE as f32);
                    let dir = direction(prev, q);
                    if prev_dir != Vec3::ZERO && dir != Vec3::ZERO {
                        turn += angle_between(prev_dir, dir);
                    }
                    prev_dir = dir;
                    prev = q;
                }
                if self.curve > 0.0 && pos_tangent[i] != Vec3::ZERO {
                    turn += angle_between(pos_tangent[i].normalize(), direction(p0, p1));
                }
                pieces = (turn.to_degrees() / self.turn_step).ceil() as usize;
                // A radius profile that is not a straight taper has to be
                // sampled at about its own thickness, or the straight cones
                // between samples flatten the swell.
                let straight = slope[i] * d;
                if (mr0 - straight).abs() > 1e-4 * d || (mr1 - straight).abs() > 1e-4 * d {
                    pieces = pieces.max((d / r0.min(r1).max(1e-6)).ceil() as usize);
                }
                pieces = pieces.clamp(1, MAX_PIECES_PER_LINK);
            }
            for s in 1..=pieces {
                if s == pieces {
                    // Land exactly on the sphere, not on float noise around it.
                    out_pos.push(p1);
                    out_radius.push(r1);
                } else {
                    let t = s as f32 / pieces as f32;
                    out_pos.push(hermite(p0, m0, p1, m1, t));
                    out_radius.push(hermite_value(r0, mr0, r1, mr1, t).clamp(r0.min(r1), r0.max(r1)));
                }
            }
        }
        self.sample_pos = out_pos;
        self.sample_radius = out_radius;
    }
}

/// The sample lattice. Sample `i` on an axis sits at `(lo + i + 0.5) * h`,
/// the voxel remesher's layout: the coordinate planes run through cell
/// centres, so a symmetric lattice is its own mirror image and the cells
/// straddling the mirror plane put their vertices exactly on it.
struct Grid {
    h: f32,
    lo: [i32; 3],
    n: [usize; 3],
}

impl Grid {
    /// Covers `min..max` with spare samples all round. With `symmetric`, the
    /// lattice mirrors exactly across x = 0.
    fn new(min: Vec3, max: Vec3, h: f32, symmetric: bool) -> Grid {
        let mut lo = [0i32; 3];
        let mut n = [0usize; 3];
        for a in 0..3 {
            lo[a] = (min[a] / h - 0.5).floor() as i32 - GRID_PAD;
            let hi = (max[a] / h - 0.5).ceil() as i32 + GRID_PAD;
            n[a] = (hi - lo[a] + 1) as usize;
            if a == 0 && symmetric {
                let half = (-lo[0]).max(hi + 1).max(1);
                lo[0] = -half;
                n[0] = 2 * half as usize;
            }
        }
        Grid { h, lo, n }
    }

    #[inline]
    fn coord(&self, axis: usize, i: usize) -> f32 {
        ((self.lo[axis] + i as i32) as f32 + 0.5) * self.h
    }

    fn samples(&self) -> usize {
        self.n[0] * self.n[1] * self.n[2]
    }

    /// Index of the sample at or below (or, with `up`, at or above) a
    /// coordinate, clamped into the lattice.
    fn index(&self, axis: usize, w: f32, up: bool) -> usize {
        let i = w / self.h - 0.5;
        let i = if up { i.ceil() } else { i.floor() } as i64 - self.lo[axis] as i64;
        i.clamp(0, self.n[axis] as i64 - 1) as usize
    }
}

/// Reflects the sampled +x half of the lattice into the -x half. Cheaper
/// than sampling it, and exact where sampling could not be: the smooth
/// minimum is not associative, so two mirrored samples folding the same
/// pieces in a different order get slightly different numbers.
fn mirror_field(sdf: &mut [f32], grid: &Grid) {
    let sx = grid.n[0];
    sdf.par_chunks_mut(sx).for_each(|row| {
        for x in 0..sx / 2 {
            row[x] = row[sx - 1 - x];
        }
    });
}

struct Extracted {
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    /// With symmetry: each vertex's mirror image (itself on the plane).
    twins: Option<Vec<u32>>,
}

/// Cell corners, numbered x + 2y + 4z, and the twelve edges between them.
const CORNERS: [Vec3; 8] = [
    Vec3::new(0.0, 0.0, 0.0),
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(1.0, 1.0, 0.0),
    Vec3::new(0.0, 0.0, 1.0),
    Vec3::new(1.0, 0.0, 1.0),
    Vec3::new(0.0, 1.0, 1.0),
    Vec3::new(1.0, 1.0, 1.0),
];
const EDGES: [(usize, usize); 12] =
    [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];

/// Surface Nets over the sampled field: one vertex in each lattice cell the
/// surface passes through (the mean of that cell's edge crossings), joined
/// with a quad per crossed lattice edge. Negative is inside.
fn extract(sdf: &[f32], grid: &Grid, symmetric: bool) -> Option<Extracted> {
    let [sx, sy, sz] = grid.n;
    let slice = sx * sy;
    let h = grid.h;

    // A cell (i, j, k) spans samples i..=i+1 on each axis.
    let slabs: Vec<Vec<(u32, Vec3)>> = (0..sz - 1)
        .into_par_iter()
        .map(|k| {
            let mut out = Vec::new();
            for j in 0..sy - 1 {
                let row = |dy: usize, dz: usize| {
                    let at = sx * (j + dy) + slice * (k + dz);
                    &sdf[at..at + sx]
                };
                let rows = [row(0, 0), row(1, 0), row(0, 1), row(1, 1)];
                for i in 0..sx - 1 {
                    let v = [
                        rows[0][i],
                        rows[0][i + 1],
                        rows[1][i],
                        rows[1][i + 1],
                        rows[2][i],
                        rows[2][i + 1],
                        rows[3][i],
                        rows[3][i + 1],
                    ];
                    let inside = v.iter().filter(|&&x| x < 0.0).count();
                    if inside == 0 || inside == 8 {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    let mut count = 0.0;
                    for (a, b) in EDGES {
                        if (v[a] < 0.0) != (v[b] < 0.0) {
                            sum += CORNERS[a].lerp(CORNERS[b], v[a] / (v[a] - v[b]));
                            count += 1.0;
                        }
                    }
                    let corner = Vec3::new(grid.coord(0, i), grid.coord(1, j), grid.coord(2, k));
                    out.push(((i + sx * j + slice * k) as u32, corner + sum / count * h));
                }
            }
            out
        })
        .collect();

    let total: usize = slabs.iter().map(Vec::len).sum();
    if total == 0 {
        return None;
    }
    let mut cells: Vec<u32> = Vec::with_capacity(total);
    let mut positions: Vec<Vec3> = Vec::with_capacity(total);
    for (cell, p) in slabs.into_iter().flatten() {
        cells.push(cell);
        positions.push(p);
    }
    // Cells were produced in ascending order, so a binary search finds a
    // cell's vertex without a hash map.
    let vertex_of = |i: usize, j: usize, k: usize| -> Option<u32> {
        cells.binary_search(&((i + sx * j + slice * k) as u32)).ok().map(|v| v as u32)
    };

    // The mirrored field puts a vertex in the mirrored cell; pairing by cell
    // is exact where matching positions would need a tolerance.
    let twins = symmetric.then(|| {
        let twins: Vec<u32> = cells
            .par_iter()
            .map(|&cell| {
                let cell = cell as usize;
                let i = cell % sx;
                // Cell i spans samples i and i+1, whose mirrors are sx-1-i
                // and sx-2-i: the mirrored cell is sx-2-i.
                let twin = cell - i + (sx - 2 - i);
                cells.binary_search(&(twin as u32)).map_or(NO_TWIN, |v| v as u32)
            })
            .collect();
        symmetrize(&mut positions, &twins);
        twins
    });

    // A quad for every lattice edge the surface crosses, joining the four
    // cells around that edge. Each cell handles the three edges leaving its
    // lowest corner.
    let inside = |i: usize, j: usize, k: usize| sdf[i + sx * j + slice * k] < 0.0;
    let indices: Vec<u32> = cells
        .par_chunks(8192)
        .flat_map_iter(|chunk| {
            let mut out: Vec<u32> = Vec::new();
            for &cell in chunk {
                let cell = cell as usize;
                let (i, j, k) = (cell % sx, cell / sx % sy, cell / slice);
                let solid = inside(i, j, k);
                let mut quad = |around: [(usize, usize, usize); 4]| {
                    let mut v = [0u32; 4];
                    for (slot, c) in v.iter_mut().zip(around) {
                        match vertex_of(c.0, c.1, c.2) {
                            Some(id) => *slot = id,
                            None => return,
                        }
                    }
                    // Listed counter-clockwise seen from the higher side of
                    // the edge; flip when the solid is on that side.
                    if !solid {
                        v.swap(1, 3);
                    }
                    let p = v.map(|id| positions[id as usize]);
                    if p[0].distance_squared(p[2]) <= p[1].distance_squared(p[3]) {
                        out.extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
                    } else {
                        out.extend_from_slice(&[v[0], v[1], v[3], v[1], v[2], v[3]]);
                    }
                };
                if j > 0 && k > 0 && inside(i + 1, j, k) != solid {
                    quad([(i, j - 1, k - 1), (i, j, k - 1), (i, j, k), (i, j - 1, k)]);
                }
                if i > 0 && k > 0 && inside(i, j + 1, k) != solid {
                    quad([(i - 1, j, k - 1), (i - 1, j, k), (i, j, k), (i, j, k - 1)]);
                }
                if i > 0 && j > 0 && inside(i, j, k + 1) != solid {
                    quad([(i - 1, j - 1, k), (i, j - 1, k), (i, j, k), (i - 1, j, k)]);
                }
            }
            out
        })
        .collect();
    (!indices.is_empty()).then_some(Extracted { positions, indices, twins })
}

/// Makes every vertex and its twin exact reflections of one another (their
/// symmetric average), and pins a vertex that is its own twin onto the plane.
fn symmetrize(v: &mut [Vec3], twins: &[u32]) {
    for i in 0..v.len() {
        let j = twins[i] as usize;
        if j == i {
            v[i].x = 0.0;
        } else if twins[i] != NO_TWIN && j > i {
            let (a, b) = (v[i], v[j]);
            let mean = Vec3::new(0.5 * (a.x - b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z));
            v[i] = mean;
            v[j] = Vec3::new(-mean.x, mean.y, mean.z);
        }
    }
}

/// Neighbours of every vertex along triangle edges, as (offsets, list).
fn adjacency(n: usize, indices: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut start = vec![0u32; n + 1];
    for &v in indices {
        start[v as usize + 1] += 2;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut raw = vec![0u32; start[n] as usize];
    for t in indices.chunks_exact(3) {
        for c in 0..3 {
            let at = &mut fill[t[c] as usize];
            raw[*at as usize] = t[(c + 1) % 3];
            raw[*at as usize + 1] = t[(c + 2) % 3];
            *at += 2;
        }
    }
    // Every edge was listed from both of its triangles: sort and drop repeats.
    let mut ranges: Vec<&mut [u32]> = Vec::with_capacity(n);
    let mut rest = raw.as_mut_slice();
    for v in 0..n {
        let (head, tail) = std::mem::take(&mut rest).split_at_mut((start[v + 1] - start[v]) as usize);
        ranges.push(head);
        rest = tail;
    }
    let unique: Vec<u32> = ranges
        .into_par_iter()
        .with_min_len(2048)
        .map(|range| {
            range.sort_unstable();
            let mut kept = 0;
            for x in 0..range.len() {
                if kept == 0 || range[kept - 1] != range[x] {
                    range[kept] = range[x];
                    kept += 1;
                }
            }
            kept as u32
        })
        .collect();
    let mut offsets = vec![0u32; n + 1];
    let mut list = Vec::with_capacity(raw.len() / 2);
    for v in 0..n {
        let from = start[v] as usize;
        list.extend_from_slice(&raw[from..from + unique[v] as usize]);
        offsets[v + 1] = list.len() as u32;
    }
    (offsets, list)
}

/// Taubin fairing: evens out the voxel grid without the shrinkage plain
/// smoothing brings.
fn fair(positions: &mut [Vec3], indices: &[u32], iterations: u32, twins: Option<&[u32]>) {
    let n = positions.len();
    let (offsets, list) = adjacency(n, indices);
    let pass = |src: &[Vec3], dst: &mut [Vec3], factor: f32| {
        dst.par_iter_mut().enumerate().with_min_len(2048).for_each(|(i, out)| {
            let ring = &list[offsets[i] as usize..offsets[i + 1] as usize];
            let p = src[i];
            *out = if ring.is_empty() {
                p
            } else {
                let mean = ring.iter().map(|&o| src[o as usize]).sum::<Vec3>() / ring.len() as f32;
                p + (mean - p) * factor
            };
        });
    };
    let mut scratch = vec![Vec3::ZERO; n];
    for _ in 0..iterations {
        pass(positions, &mut scratch, TAUBIN_LAMBDA);
        if let Some(twins) = twins {
            symmetrize(&mut scratch, twins);
        }
        pass(&scratch, positions, TAUBIN_MU);
        if let Some(twins) = twins {
            symmetrize(positions, twins);
        }
    }
}

/// Area-weighted vertex normals.
fn vertex_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in indices.chunks_exact(3) {
        let p = [0, 1, 2].map(|c| positions[t[c] as usize]);
        let n = (p[1] - p[0]).cross(p[2] - p[0]);
        for &v in t {
            normals[v as usize] += n;
        }
    }
    for n in &mut normals {
        *n = n.normalize_or(Vec3::Y);
    }
    normals
}

/// Coarse buckets of pieces over the lattice, so evaluating the field at one
/// point only visits the pieces that can reach it. Each bucket's list stays
/// in piece order, which is limb order: what [`Field::evaluate`] relies on.
struct Bins {
    origin: Vec3,
    size: f32,
    dims: [usize; 3],
    start: Vec<u32>,
    pieces: Vec<u32>,
}

impl Bins {
    fn build(field: &Field, grid: &Grid) -> Bins {
        let mut bins = Bins {
            origin: Vec3::new(grid.coord(0, 0), grid.coord(1, 0), grid.coord(2, 0)),
            size: grid.h * BIN_CELLS as f32,
            dims: grid.n.map(|n| n / BIN_CELLS + 1),
            start: Vec::new(),
            pieces: Vec::new(),
        };
        let count = bins.dims[0] * bins.dims[1] * bins.dims[2];
        let margin = Vec3::splat(2.0 * grid.h);
        let boxes: Vec<_> =
            field.pieces.iter().map(|p| (bins.cell(p.min - margin), bins.cell(p.max + margin))).collect();
        let each = |f: &mut dyn FnMut(usize, u32)| {
            for (pi, (lo, hi)) in boxes.iter().enumerate() {
                for z in lo[2]..=hi[2] {
                    for y in lo[1]..=hi[1] {
                        for x in lo[0]..=hi[0] {
                            f(x + bins.dims[0] * (y + bins.dims[1] * z), pi as u32);
                        }
                    }
                }
            }
        };
        let mut start = vec![0u32; count + 1];
        each(&mut |bin, _| start[bin + 1] += 1);
        for b in 0..count {
            start[b + 1] += start[b];
        }
        let mut fill = start.clone();
        let mut pieces = vec![0u32; start[count] as usize];
        each(&mut |bin, pi| {
            pieces[fill[bin] as usize] = pi;
            fill[bin] += 1;
        });
        bins.start = start;
        bins.pieces = pieces;
        bins
    }

    fn cell(&self, p: Vec3) -> [usize; 3] {
        let local = (p - self.origin) / self.size;
        [0, 1, 2].map(|a| (local[a].floor() as i64).clamp(0, self.dims[a] as i64 - 1) as usize)
    }

    /// The pieces that can reach `p`.
    fn at(&self, p: Vec3) -> &[u32] {
        let c = self.cell(p);
        let bin = c[0] + self.dims[0] * (c[1] + self.dims[1] * c[2]);
        &self.pieces[self.start[bin] as usize..self.start[bin + 1] as usize]
    }
}

fn resolve_resolution(rig: &Rig, settings: &SkinSettings, quality: Quality, extent: f32) -> u32 {
    let (scale, max_resolution, _) = quality.limits();
    let min_radius = rig.min_radius();
    let density = settings.density.clamp(MIN_DENSITY, MAX_DENSITY);
    // Cells small enough that the thinnest sphere spans a few of them, scaled
    // by the artist's density: one default is then right for a thumb-sized
    // rig and a building-sized one alike.
    let wanted = if min_radius > 0.0 {
        extent / (min_radius / CELLS_PER_MIN_RADIUS) * density * scale
    } else {
        64.0 * density * scale
    };
    (wanted.ceil() as u32).clamp(MIN_RESOLUTION, max_resolution)
}

/// The resolution Make Skin will run at, for the interface. An estimate: it
/// does not model the sample-budget walk-down.
pub fn final_resolution(rig: &Rig, symmetry: bool, settings: &SkinSettings) -> u32 {
    if rig.is_empty() {
        return MIN_RESOLUTION;
    }
    let bounds = rig.bounds(symmetry);
    let extent = bounds.size().max_element() + settings.blend.max(0.0) * rig.mean_radius() * 0.5;
    resolve_resolution(rig, settings, Quality::Final, extent.max(1e-4))
}

/// Skins the rig. Fails (leaving the caller's last good skin in place) when
/// there is nothing to skin or the field produced no surface.
pub fn skin(rig: &Rig, symmetry: bool, settings: &SkinSettings, quality: Quality) -> Result<Skin, String> {
    if rig.is_empty() {
        return Err("No spheres placed.".into());
    }
    let started = Instant::now();
    let field = Field::build(rig, symmetry, settings, quality);
    if field.pieces.is_empty() {
        return Err("No spheres placed.".into());
    }

    // Bounds from the pieces themselves: the spline can swing a little
    // outside the hull of the sphere centres, and the fillets push the
    // surface out past the pieces by a fraction of the blend width.
    let (min, max, max_k) = field.bounds();
    let (min, max) = (min - max_k * 0.5, max + max_k * 0.5);
    let extent = (max - min).max_element().max(1e-4);
    let (_, _, budget) = quality.limits();
    let mut resolution = resolve_resolution(rig, settings, quality, extent);
    let mut grid = Grid::new(min, max, extent / resolution as f32, symmetry);
    while grid.samples() > budget && resolution > MIN_RESOLUTION {
        resolution = MIN_RESOLUTION.max((resolution as f32 * 0.8) as u32);
        grid = Grid::new(min, max, extent / resolution as f32, symmetry);
    }

    let mut sdf = field.sample(&grid, if symmetry { grid.n[0] / 2 } else { 0 });
    if symmetry {
        mirror_field(&mut sdf, &grid);
    }
    let Some(Extracted { mut positions, indices, twins }) = extract(&sdf, &grid, symmetry) else {
        return Err("Skinning produced no surface. Try a higher skin detail.".into());
    };
    drop(sdf);

    let smoothing = if quality == Quality::Draft { settings.smoothing.min(2) } else { settings.smoothing };
    if smoothing > 0 {
        fair(&mut positions, &indices, smoothing, twins.as_deref());
    }
    let mut normals = vertex_normals(&positions, &indices);
    // The draft is a drag-time preview: one projection step is enough to
    // take the voxel wobble out of it.
    {
        let steps = if quality == Quality::Draft { 1 } else { PROJECTION_STEPS };
        let bins = Bins::build(&field, &grid);
        field.project(&bins, &mut positions, &mut normals, grid.h, steps);
        if let Some(twins) = &twins {
            symmetrize(&mut positions, twins);
            symmetrize(&mut normals, twins);
            for n in &mut normals {
                *n = n.normalize_or(Vec3::Y);
            }
        }
    }

    let stats = SkinStats {
        triangles: indices.len() / 3,
        resolution,
        cell: grid.h,
        ms: started.elapsed().as_secs_f32() * 1000.0,
        limbs: field.limbs.len(),
        pieces: field.pieces.len(),
    };
    Ok(Skin { positions, normals, indices, stats })
}
