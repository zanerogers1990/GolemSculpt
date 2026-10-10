//! Builds pegs and holes into a part's cut face, as real geometry welded into
//! the surface: no voxels, no boolean, nothing remeshed.
//!
//! WHY NOT THE VOXEL BOOLEAN. `sculpt_volume::boolean` could union a peg on
//! and subtract a hole out, but it re-meshes the whole object onto a lattice,
//! rounds every edge to the voxel size and cannot hold a 0.15 mm clearance on
//! any lattice a sculpt could afford. A key is only useful if it is exactly
//! the size it says. So the key is CONSTRUCTED: its outline is sampled from
//! `KeyProfile` at exactly the offsets the fit needs, and sewn into the cut
//! face.
//!
//! HOW IT IS SEWN IN. The cut face is already a triangulated patch (the
//! trimmer's cap). Around each key, the patch triangles within a small margin
//! of the key's footprint are taken out; what remains of the patch is
//! untouched. The hole that leaves is a disk whose rim is made of EXISTING
//! vertices, and it is refilled as a flat ring (its outer loop that rim, its
//! inner loop the key's footprint) at the density of the surface around it
//! (`sculpt_cut::planar`). The key's wall then rises (peg) or sinks (hole)
//! from the footprint as a stack of rings, and an end face closes it. Every
//! seam shares vertices with the geometry on both sides of it, and every new
//! triangle is wound to agree with its neighbours by construction: the wall
//! takes its direction from the ring it was built against, never from a guess
//! about which way is out. So the result is exactly as closed and as manifold
//! as the part was.
//!
//! The ring is refilled in the key's own plane. For a flat cut (a
//! straight-line cut, or a box wall) that plane IS the face. For a gently
//! curved one (a lasso) the footprint and every new point are lifted back
//! onto the old face by barycentric lookup in the triangles that were taken
//! out, so the face keeps its shape to within its own triangulation.
//!
//! Hard edges get a SUPPORT RING a hair away from them, the trick subdivision
//! modellers use. Vertex normals in the app are area-weighted and shared
//! across the edge (the key is welded, not split, for the same reason the
//! trim cap is), so without it the shading of a 90-degree edge bleeds across
//! the whole first ring of the wall. With it, the blend happens across a
//! strip a few hundredths of a millimetre wide and the edge reads crisp. It
//! changes nothing a printer can resolve.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{Vec2, Vec3};
use sculpt_cut::{planar, polygon};
use sculpt_geom::edge_key::{EdgeMap, EdgeSet, directed_edge_key};
use sculpt_geom::{TriMesh, units};

use crate::joint::KeyPart;
use crate::profile::{KeyProfile, KeyShape};
use crate::types::{KeyDimensions, KeyFeature, KeyFrame, KeyRole};

/// Steepest a cut face may lean away from the key's axis under a key. Past
/// this, the flattened ring would fold over itself, and the key would be
/// sticking out sideways.
const MAX_TILT_DEGREES: f32 = 70.0;

/// Rounds of "grow the taken-out region" allowed while untangling pinched
/// rims.
const MAX_REGION_PASSES: usize = 8;

/// `Built::source` for a vertex the keys added.
pub const NEW_VERTEX: u32 = u32::MAX;

const TOO_IRREGULAR: &str = "cut face is too irregular under this key";

/// A part with its keys built in.
#[derive(Clone, Debug, Default)]
pub struct Built {
    /// In the part's own space.
    pub mesh: TriMesh,
    /// For each vertex of `mesh`, the vertex of the part it is, or
    /// `NEW_VERTEX`: what carries a mask or any other per-vertex value over.
    pub source: Vec<u32>,
    /// Per feature: `None` when it was built, otherwise why not. A feature
    /// that fails leaves its part of the cut face exactly as it was.
    pub errors: Vec<Option<String>>,
    pub built: usize,
}

/// Builds `features` into `part`.
pub fn build(part: &KeyPart, features: &[KeyFeature]) -> Built {
    let mut job = Job {
        part,
        features,
        original: part.world().len(),
        removed: vec![false; part.triangle_count()],
        new_world: Vec::new(),
        new_triangles: Vec::new(),
        errors: vec![None; features.len()],
    };
    job.run();
    job.assemble()
}

/// One ring of a feature's wall: how far along the axis it is, and the
/// outline offset it is drawn at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub height: f32,
    pub offset: f32,
}

/// The rings a feature's wall is made of, from just above the cut face to its
/// end face, and the offset of the support ring inside the end face. Shared
/// by the builder and the preview, so what is previewed is what is built.
pub fn rings(feature: &KeyFeature, edge: f32) -> (Vec<Ring>, f32) {
    let d = feature.dimensions;
    let slope = d.taper_slope();
    let support = support_distance(feature.profile.size(), d.height, edge);
    let spacing = edge.max(support * 4.0);
    let mut heights = Vec::new();

    let rings: Vec<Ring> = match feature.role {
        KeyRole::Peg => {
            let h = d.height.max(support * 6.0);
            let c = d.tip_chamfer.clamp(0.0, h * 0.45);
            // Up to the chamfer's start (or, with no chamfer, to a support
            // ring just under the tip's edge), then the tip itself.
            let wall_top = if c > 0.0 { h - c } else { h - support };
            add_run(&mut heights, support, wall_top, spacing);
            heights.push(h);
            heights.iter().map(|&z| Ring { height: z, offset: -z * slope - (z - (h - c)).max(0.0) }).collect()
        }
        KeyRole::Hole => {
            let depth = d.hole_depth().max(support * 6.0);
            let e = d.entry_chamfer.clamp(0.0, depth * 0.45);
            if e > 0.0 {
                heights.push(e);
            }
            add_run(&mut heights, e + support, depth - support, spacing);
            heights.push(depth);
            heights.iter().map(|&z| Ring { height: z, offset: d.clearance - z * slope + (e - z).max(0.0) }).collect()
        }
    };
    let end_inset = rings[rings.len() - 1].offset - support;
    (rings, end_inset)
}

/// From `from` to `to` inclusive, in steps no longer than `spacing`.
fn add_run(heights: &mut Vec<f32>, from: f32, to: f32, spacing: f32) {
    if to <= from {
        heights.push(from);
        return;
    }
    let steps = (((to - from) / spacing).ceil() as usize).max(1);
    heights.extend((0..=steps).map(|i| from + (to - from) * (i as f32 / steps as f32)));
}

/// A profile for a key of these dimensions: its corners rounded enough for
/// every offset the peg and the hole will ask of it (see `KeyProfile`),
/// sampled finely enough that no arc strays a hundredth of a millimetre from
/// its circle, and with straight runs split at `edge` so the wall matches the
/// density of the face it stands on.
pub fn create_profile(
    shape: KeyShape,
    size: f32,
    aspect: f32,
    corner_radius: f32,
    d: &KeyDimensions,
    edge: f32,
) -> KeyProfile {
    let support = support_distance(size, d.height, edge);
    let peg_tip = d.tip_offset() - support;
    let hole_floor = d.clearance - d.hole_depth() * d.taper_slope() - support;
    let margin = support * 2.0;
    let min_offset = peg_tip.min(hole_floor).min(0.0) - margin;
    let max_offset = d.mouth_offset() + margin;
    let chord = units::from_millimetres(0.01).max(size * 0.0015);
    // Written as max-then-min: a size near zero must not panic a `clamp`.
    let max_edge = edge.max(size / 48.0).min(size / 3.0);
    KeyProfile::create(shape, size, aspect, corner_radius, min_offset, max_offset, chord, max_edge)
}

/// `create_profile`, but when the shape cannot take the dimensions at this
/// size (a small star asked for a tip chamfer wider than its points) the tip
/// chamfer and then the taper are eased off until it can, and the returned
/// flag says so. Both halves are then built from the eased dimensions, so the
/// fit is unaffected; only the lead-in is smaller. Returns an invalid profile
/// only when even a plain straight key will not fit the shape.
pub fn fit_profile(
    shape: KeyShape,
    size: f32,
    aspect: f32,
    corner_radius: f32,
    d: &mut KeyDimensions,
    edge: f32,
) -> (KeyProfile, bool) {
    let mut adjusted = false;
    let mut profile = create_profile(shape, size, aspect, corner_radius, d, edge);
    for attempt in 0..6 {
        if profile.is_valid() {
            break;
        }
        adjusted = true;
        if d.tip_chamfer > 1e-6 {
            d.tip_chamfer = if attempt < 4 { d.tip_chamfer * 0.5 } else { 0.0 };
        } else {
            d.taper_degrees = if attempt < 5 { d.taper_degrees * 0.5 } else { 0.0 };
        }
        profile = create_profile(shape, size, aspect, corner_radius, d, edge);
    }
    (profile, adjusted)
}

/// A stand-alone solid of a feature's wall and end face (the same rings
/// `build` would stack on the face, standing on the key's plane), for
/// previewing a key before it is built. World space (the frame's). Appended
/// to `into`; outward-facing for a peg, inward (facing into the socket) for a
/// hole. Open at the cut face.
pub fn preview(feature: &KeyFeature, edge: f32, into: &mut TriMesh) {
    let profile = &feature.profile;
    if !profile.is_valid() {
        return;
    }
    let (rings, end_inset) = rings(feature, edge);
    let count = profile.point_count();
    let mut outline = vec![Vec2::ZERO; count];
    let outward = feature.role == KeyRole::Peg;
    let mut add_ring = |into: &mut TriMesh, offset: f32, height: f32| -> u32 {
        let first = into.positions.len() as u32;
        profile.outline_into(offset, &mut outline);
        into.positions.extend(outline.iter().map(|&p| feature.frame.point(p, height)));
        first
    };
    // Counter-clockwise outline seen from the tip of the axis: (lower i,
    // lower j, upper j) faces out.
    let strip = |into: &mut TriMesh, lower: u32, upper: u32| {
        for i in 0..count as u32 {
            let j = (i + 1) % count as u32;
            let (a, b, c, d) = (lower + i, lower + j, upper + j, upper + i);
            into.indices.extend_from_slice(&if outward { [a, b, c, a, c, d] } else { [a, c, b, a, d, c] });
        }
    };

    let mut lower = add_ring(into, footprint_offset(feature), 0.0);
    for ring in &rings {
        let upper = add_ring(into, ring.offset, ring.height);
        strip(into, lower, upper);
        lower = upper;
    }
    let top = rings[rings.len() - 1].height;
    let inset = add_ring(into, end_inset, top);
    strip(into, lower, inset);

    let face: Vec<u32> = (0..count as u32).collect();
    let Ok(triangles) = planar::triangulate(&outline, &face, &[]) else { return };
    // Counter-clockwise in the profile plane faces along the axis: out of a
    // peg's tip, into a hole's floor.
    for t in triangles.chunks_exact(3) {
        let [a, b, c] = [inset + t[0], inset + t[1], inset + t[2]];
        into.indices.extend_from_slice(&if outward { [a, b, c] } else { [a, c, b] });
    }
}

/// The outline offset of a feature's footprint on the cut face.
pub fn footprint_offset(feature: &KeyFeature) -> f32 {
    match feature.role {
        KeyRole::Peg => 0.0,
        KeyRole::Hole => feature.dimensions.mouth_offset(),
    }
}

/// How far a support ring sits from the edge it sharpens. A small fraction of
/// the local edge length, and never more than a sliver of the key itself.
pub fn support_distance(size: f32, height: f32, edge: f32) -> f32 {
    let h = height.max(1e-4);
    (0.15 * edge).min(0.04 * h).min(0.02 * size).max(1e-5)
}

// ===========================================================================

struct Job<'a> {
    part: &'a KeyPart,
    features: &'a [KeyFeature],
    /// Vertices the part came with; new ones are numbered after them.
    original: usize,
    removed: Vec<bool>,
    new_world: Vec<Vec3>,
    new_triangles: Vec<u32>,
    errors: Vec<Option<String>>,
}

impl Job<'_> {
    fn run(&mut self) {
        // Features by the cut face they are on, faces in the order first met.
        let mut by_patch: Vec<(usize, Vec<usize>)> = Vec::new();
        for (id, feature) in self.features.iter().enumerate() {
            if let Some(problem) = feature.profile.problem() {
                self.errors[id] = Some(problem.to_string());
                continue;
            }
            if feature.patch >= self.part.cut_faces.len() {
                self.errors[id] = Some("not on a cut face".to_string());
                continue;
            }
            match by_patch.iter_mut().find(|(patch, _)| *patch == feature.patch) {
                Some((_, ids)) => ids.push(id),
                None => by_patch.push((feature.patch, vec![id])),
            }
        }
        let part = self.part;
        for (patch, ids) in by_patch {
            PatchJob::new(self, &part.cut_faces[patch], ids).run();
        }
    }

    fn assemble(self) -> Built {
        let indices = self.part.indices();
        let mut triangles: Vec<u32> = Vec::with_capacity(indices.len() + self.new_triangles.len());
        for (t, corners) in indices.chunks_exact(3).enumerate() {
            if !self.removed[t] {
                triangles.extend_from_slice(corners);
            }
        }
        triangles.extend_from_slice(&self.new_triangles);

        // Drop the vertices the taken-out regions left behind: the old
        // interior points of the cut face under each key. Unreferenced
        // vertices are harmless to render, but a brush would find and move
        // them, and every exporter and checker would count them.
        let total = self.original + self.new_world.len();
        let mut used = vec![false; total];
        for &i in &triangles {
            used[i as usize] = true;
        }
        let mut remap = vec![u32::MAX; total];
        let mut count = 0u32;
        for i in (0..total).filter(|&i| used[i]) {
            remap[i] = count;
            count += 1;
        }

        let to_local = self.part.local_to_world().inverse();
        let local = &self.part.mesh().positions;
        let mut positions = Vec::with_capacity(count as usize);
        let mut source = Vec::with_capacity(count as usize);
        for i in (0..total).filter(|&i| used[i]) {
            if i < self.original {
                positions.push(local[i]);
                source.push(i as u32);
            } else {
                positions.push(to_local.transform_point3(self.new_world[i - self.original]));
                source.push(NEW_VERTEX);
            }
        }
        for i in &mut triangles {
            *i = remap[*i as usize];
        }
        let built = self.errors.iter().filter(|e| e.is_none()).count();
        Built { mesh: TriMesh::new(positions, triangles), source, errors: self.errors, built }
    }

    fn add_vertex(&mut self, world: Vec3) -> u32 {
        self.new_world.push(world);
        (self.original + self.new_world.len() - 1) as u32
    }

    fn world_of(&self, vertex: u32) -> Vec3 {
        match (vertex as usize).checked_sub(self.original) {
            Some(new) => self.new_world[new],
            None => self.part.world()[vertex as usize],
        }
    }

    fn emit(&mut self, a: u32, b: u32, c: u32) {
        if a != b && b != c && c != a {
            self.new_triangles.extend_from_slice(&[a, b, c]);
        }
    }

    /// One band of wall between two corresponding rings. When the ring below
    /// is used i -> i+1 by what is under it (`forward`), this band uses it
    /// i+1 -> i and leaves the ring above used i -> i+1: the same situation
    /// one band up, which is why one flag serves the whole stack.
    fn emit_strip(&mut self, lower: &[u32], upper: &[u32], forward: bool) {
        let n = lower.len();
        for i in 0..n {
            let j = (i + 1) % n;
            if forward {
                self.emit(lower[j], lower[i], upper[i]);
                self.emit(lower[j], upper[i], upper[j]);
            } else {
                self.emit(lower[i], lower[j], upper[j]);
                self.emit(lower[i], upper[j], upper[i]);
            }
        }
    }

    fn add_ring(&mut self, feature: &KeyFeature, outline: &[Vec2], height: f32) -> Vec<u32> {
        outline.iter().map(|&p| self.add_vertex(feature.frame.point(p, height))).collect()
    }

    /// Stacks the feature's rings on its footprint and closes the end.
    /// `forward` says which way round the refilled face uses the footprint
    /// ring; the wall uses it the other way, and every ring after that
    /// follows (see `emit_strip`).
    fn build_wall(&mut self, feature: &KeyFeature, footprint: Vec<u32>, forward: bool, lift: f32, edge: f32) {
        let (rings, end_inset) = rings(feature, edge);
        let count = footprint.len();
        let mut outline = vec![Vec2::ZERO; count];
        let mut lower = footprint;
        for ring in &rings {
            feature.profile.outline_into(ring.offset, &mut outline);
            let upper = self.add_ring(feature, &outline, ring.height + lift);
            self.emit_strip(&lower, &upper, forward);
            lower = upper;
        }

        // The end face: a thin strip in to a support ring, then the flat
        // polygon.
        let end_height = rings[rings.len() - 1].height + lift;
        feature.profile.outline_into(end_inset, &mut outline);
        let inset = self.add_ring(feature, &outline, end_height);
        self.emit_strip(&lower, &inset, forward);

        let face: Vec<u32> = (0..count as u32).collect();
        // A profile outline is simple by construction, so the fan is a last
        // resort: still closed, which is the property that matters.
        let mut triangles = planar::triangulate(&outline, &face, &[])
            .unwrap_or_else(|_| (1..count as u32 - 1).flat_map(|i| [0, i, i + 1]).collect());
        planar::improve(&mut outline, &mut triangles, &planar::loop_edges(&face, &[]), 0.0, 0);

        // Counter-clockwise in the profile's plane walks the ring i -> i+1;
        // the strip before left that direction used already when `forward`,
        // so the face takes it reversed.
        for t in triangles.chunks_exact(3) {
            let [a, b, c] = [inset[t[0] as usize], inset[t[1] as usize], inset[t[2] as usize]];
            if forward { self.emit(a, c, b) } else { self.emit(a, b, c) }
        }
    }
}

// ============================================================== one cut face

/// A feature on this cut face, with its outlines in the face's chart.
struct Slot {
    /// Index into the job's features.
    id: usize,
    /// The feature's outline on the face.
    footprint: Vec<Vec2>,
    /// The footprint grown by the key's support distance.
    support: Vec<Vec2>,
    /// The margin of face taken out around the footprint.
    gap: f32,
}

struct PatchJob<'j, 'a> {
    job: &'j mut Job<'a>,
    /// Mesh triangle indices.
    patch: &'a [u32],
    ids: Vec<usize>,
    slots: Vec<Slot>,

    origin: Vec3,
    e1: Vec3,
    e2: Vec3,
    edge: f32,
    /// Which way round the patch runs in the chart.
    sign: f32,
    /// Per patch triangle (local index k): its corners, in the chart.
    corners: Vec<[Vec2; 3]>,
    in_region: Vec<bool>,
    /// Directed edge (u, v) -> local triangle.
    edge_owner: EdgeMap<u32>,
}

impl<'j, 'a> PatchJob<'j, 'a> {
    fn new(job: &'j mut Job<'a>, patch: &'a [u32], ids: Vec<usize>) -> PatchJob<'j, 'a> {
        PatchJob {
            job,
            patch,
            ids,
            slots: Vec::new(),
            origin: Vec3::ZERO,
            e1: Vec3::X,
            e2: Vec3::Y,
            edge: 0.0,
            sign: 0.0,
            corners: Vec::new(),
            in_region: Vec::new(),
            edge_owner: EdgeMap::default(),
        }
    }

    fn feature(&self, slot: usize) -> &'a KeyFeature {
        &self.job.features[self.slots[slot].id]
    }

    fn failed(&self, slot: usize) -> bool {
        self.job.errors[self.slots[slot].id].is_some()
    }

    fn fail(&mut self, slot: usize, why: &str) {
        let error = &mut self.job.errors[self.slots[slot].id];
        if error.is_none() {
            *error = Some(why.to_string());
        }
    }

    fn chart(&self, world: Vec3) -> Vec2 {
        let d = world - self.origin;
        Vec2::new(d.dot(self.e1), d.dot(self.e2))
    }

    /// The mesh vertices of patch triangle `k`.
    fn vertices(&self, k: usize) -> [u32; 3] {
        let t = self.patch[k] as usize * 3;
        let i = &self.job.part.indices()[t..t + 3];
        [i[0], i[1], i[2]]
    }

    fn world_corners(&self, k: usize) -> [Vec3; 3] {
        self.vertices(k).map(|v| self.job.world_of(v))
    }

    fn run(mut self) {
        if self.patch.is_empty() {
            for &id in &self.ids {
                self.job.errors[id].get_or_insert_with(|| "empty cut face".to_string());
            }
            return;
        }

        let frame = self.job.features[self.ids[0]].frame;
        let axis = frame.axis.normalize();
        self.origin = frame.origin;
        self.e1 = (frame.u - axis * frame.u.dot(axis)).normalize();
        self.e2 = axis.cross(self.e1).normalize();

        self.build_chart();
        self.slots = self.ids.iter().map(|&id| self.slot(id)).collect();
        if self.sign == 0.0 {
            for slot in 0..self.slots.len() {
                self.fail(slot, "cut face is edge-on to the key axis");
            }
            return;
        }

        self.drop_overlapping();
        let live: Vec<usize> = (0..self.slots.len()).filter(|&s| !self.failed(s)).collect();
        if live.is_empty() {
            return;
        }

        self.select_region(&live);
        if !self.settle_region() {
            for &slot in &live {
                self.fail(slot, TOO_IRREGULAR);
            }
            return;
        }

        let mut built = vec![false; self.slots.len()];
        for component in self.components() {
            self.build_component(&component, &live, &mut built);
        }
        // A footprint that no taken-out piece of the face closed round is
        // hanging off the face's edge.
        for &slot in &live {
            if !built[slot] {
                self.fail(slot, "off the cut face");
            }
        }
    }

    // -------------------------------------------------------------- the chart

    fn build_chart(&mut self) {
        let n = self.patch.len();
        self.corners = Vec::with_capacity(n);
        self.edge_owner = EdgeMap::with_capacity_and_hasher(n * 3, Default::default());
        let (mut signed, mut edge_sum) = (0.0f64, 0.0f64);
        for k in 0..n {
            let [i0, i1, i2] = self.vertices(k);
            let [w0, w1, w2] = self.world_corners(k);
            let c = [self.chart(w0), self.chart(w1), self.chart(w2)];
            signed += polygon::cross(c[0], c[1], c[2]);
            edge_sum += (w0.distance(w1) + w1.distance(w2) + w2.distance(w0)) as f64;
            self.corners.push(c);
            for (from, to) in [(i0, i1), (i1, i2), (i2, i0)] {
                self.edge_owner.insert(directed_edge_key(from, to), k as u32);
            }
        }
        self.sign = if signed > 0.0 {
            1.0
        } else if signed < 0.0 {
            -1.0
        } else {
            0.0
        };
        self.edge = (edge_sum / (3.0 * n as f64)) as f32;
    }

    fn slot(&self, id: usize) -> Slot {
        let feature = &self.job.features[id];
        let outline = |offset: f32| -> Vec<Vec2> {
            feature.profile.outline(offset).iter().map(|&p| self.chart(feature.frame.point(p, 0.0))).collect()
        };
        let footprint = footprint_offset(feature);
        let support = support_distance(feature.profile.size(), feature.dimensions.height, self.edge);
        Slot {
            id,
            footprint: outline(footprint),
            support: outline(footprint + support),
            gap: self.edge.max(0.12 * feature.profile.size()),
        }
    }

    /// Two keys whose footprints (plus their margins) overlap would need one
    /// wall to run through the other. The later one is dropped, with a
    /// reason.
    fn drop_overlapping(&mut self) {
        for i in 0..self.slots.len() {
            if self.failed(i) {
                continue;
            }
            let crowded = (0..i).any(|j| {
                !self.failed(j)
                    && polygon_distance(&self.slots[i].footprint, &self.slots[j].footprint)
                        < self.slots[i].gap.max(self.slots[j].gap)
            });
            if crowded {
                self.fail(i, "overlaps another key");
            }
        }
    }

    // ------------------------------------------------------ the taken-out region

    fn select_region(&mut self, live: &[usize]) {
        let boxes: Vec<(Vec2, Vec2, usize)> = live
            .iter()
            .map(|&s| {
                let slot = &self.slots[s];
                let lo = slot.footprint.iter().fold(Vec2::MAX, |m, p| m.min(*p));
                let hi = slot.footprint.iter().fold(Vec2::MIN, |m, p| m.max(*p));
                (lo - Vec2::splat(slot.gap), hi + Vec2::splat(slot.gap), s)
            })
            .collect();
        self.in_region = self
            .corners
            .iter()
            .map(|&[a, b, c]| {
                let (tlo, thi) = (a.min(b).min(c), a.max(b).max(c));
                boxes.iter().any(|&(lo, hi, s)| {
                    let slot = &self.slots[s];
                    !(thi.cmplt(lo).any() || tlo.cmpgt(hi).any())
                        && polygon::triangle_to_polygon_distance(a, b, c, &slot.footprint) <= slot.gap
                })
            })
            .collect();
    }

    /// The patch triangle across edge `corner` of triangle `k`.
    fn neighbour(&self, k: usize, corner: usize) -> Option<usize> {
        let v = self.vertices(k);
        self.edge_owner.get(&directed_edge_key(v[(corner + 1) % 3], v[corner])).map(|&o| o as usize)
    }

    /// Whether the edge `corner` of region triangle `k` is on the region's
    /// rim.
    fn on_rim(&self, k: usize, corner: usize) -> bool {
        !self.neighbour(k, corner).is_some_and(|o| self.in_region[o])
    }

    /// Makes the taken-out region a set of clean disks: anything it encloses
    /// is taken too (else the refill would have to go round an island), and
    /// wherever its rim touches itself at a single vertex the triangles round
    /// that vertex are added (else the rim is not a simple loop). False if it
    /// will not settle.
    fn settle_region(&mut self) -> bool {
        let n = self.patch.len();
        for _ in 0..MAX_REGION_PASSES {
            // Everything outside the region that cannot reach the cut face's
            // own rim without crossing the region is enclosed by it.
            let mut reached = vec![false; n];
            let mut stack: Vec<usize> = Vec::new();
            for k in (0..n).filter(|&k| !self.in_region[k]) {
                if (0..3).any(|e| self.neighbour(k, e).is_none()) {
                    reached[k] = true;
                    stack.push(k);
                }
            }
            while let Some(k) = stack.pop() {
                for e in 0..3 {
                    if let Some(o) = self.neighbour(k, e).filter(|&o| !reached[o] && !self.in_region[o]) {
                        reached[o] = true;
                        stack.push(o);
                    }
                }
            }
            for k in 0..n {
                self.in_region[k] |= !reached[k];
            }

            // Pinches: a vertex with more than one rim edge leaving it.
            let mut outgoing: HashMap<u32, u32> = HashMap::new();
            for k in (0..n).filter(|&k| self.in_region[k]) {
                let v = self.vertices(k);
                for e in (0..3).filter(|&e| self.on_rim(k, e)) {
                    *outgoing.entry(v[e]).or_insert(0) += 1;
                }
            }
            outgoing.retain(|_, count| *count > 1);
            if outgoing.is_empty() {
                return true;
            }
            for k in 0..n {
                if !self.in_region[k] && self.vertices(k).iter().any(|v| outgoing.contains_key(v)) {
                    self.in_region[k] = true;
                }
            }
        }
        false
    }

    /// Connected pieces of the region, as lists of local triangle indices.
    fn components(&self) -> Vec<Vec<usize>> {
        let n = self.patch.len();
        let mut seen = vec![false; n];
        let mut result = Vec::new();
        let mut stack = Vec::new();
        for start in 0..n {
            if !self.in_region[start] || seen[start] {
                continue;
            }
            let mut component = Vec::new();
            seen[start] = true;
            stack.push(start);
            while let Some(k) = stack.pop() {
                component.push(k);
                for e in 0..3 {
                    if let Some(o) = self.neighbour(k, e).filter(|&o| !seen[o] && self.in_region[o]) {
                        seen[o] = true;
                        stack.push(o);
                    }
                }
            }
            result.push(component);
        }
        result
    }

    // ------------------------------------------------------ one region, rebuilt

    /// The rim of a piece of the region, as directed loops of existing
    /// vertices.
    fn rim_loops(&self, component: &[usize]) -> Vec<Vec<u32>> {
        let mut next: HashMap<u32, u32> = HashMap::new();
        let mut starts: Vec<u32> = Vec::new();
        for &k in component {
            let v = self.vertices(k);
            for e in (0..3).filter(|&e| self.on_rim(k, e)) {
                if next.insert(v[e], v[(e + 1) % 3]).is_none() {
                    starts.push(v[e]);
                }
            }
        }
        let mut loops = Vec::new();
        let mut visited: HashSet<u32> = HashSet::new();
        for start in starts {
            let mut path = Vec::new();
            let mut v = start;
            while visited.insert(v) {
                path.push(v);
                match next.get(&v) {
                    Some(&to) => v = to,
                    None => break,
                }
            }
            if !path.is_empty() {
                loops.push(path);
            }
        }
        loops
    }

    /// Whether a key's support loop stands wholly on this piece of the face,
    /// and how far up the key's axis the face reaches under it: the height
    /// the key's wall starts from.
    fn footing(&self, slot: usize, rim: &[Vec2], locator: &SurfaceLocator) -> Result<f32, &'static str> {
        let feature = self.feature(slot);
        let mut top = f32::MIN;
        for &p in &self.slots[slot].support {
            if !polygon::contains(rim, p) || polygon::distance_to_outline(rim, p) < self.edge * 0.05 {
                return Err("too close to the edge of the cut face");
            }
            // The wall's first ring has to clear the highest point of the
            // footprint, which on a curved face is not the key's origin.
            top = top.max((locator.lift(p) - feature.frame.origin).dot(feature.frame.axis));
        }
        if top > 0.5 * feature.dimensions.height.max(1e-6) {
            return Err("cut face is too curved under this key");
        }
        Ok(top.max(0.0))
    }

    fn build_component(&mut self, component: &[usize], live: &[usize], built: &mut [bool]) {
        let loops = self.rim_loops(component);
        let locator = SurfaceLocator::new(self, component);

        if loops.len() != 1 {
            // No single rim to refill inside. Whichever keys stand on this
            // piece cannot be built; the face is left as it was.
            for &slot in live {
                if !self.failed(slot) && locator.covers(self.slots[slot].footprint[0]) {
                    self.fail(slot, TOO_IRREGULAR);
                    built[slot] = true;
                }
            }
            return;
        }
        let outer = &loops[0];
        let rim: Vec<Vec2> = outer.iter().map(|&v| self.chart(self.job.world_of(v))).collect();

        // The keys whose footprint is inside this piece.
        let mut mine: Vec<usize> = live
            .iter()
            .copied()
            .filter(|&s| !self.failed(s) && rim.len() >= 3 && polygon::contains(&rim, self.slots[s].footprint[0]))
            .collect();
        // A grown region with no key in it: leave the face alone.
        if mine.is_empty() {
            return;
        }
        for &slot in &mine {
            built[slot] = true;
        }

        // Every triangle coming out must face the same way as the face does,
        // and not lean too far off the key's axis: the refill is flat in the
        // key's plane. Slivers (see `KeyJoint`'s remarks) are skipped: their
        // normals are noise, and they are about to be taken out and refilled
        // anyway.
        let cos_tilt = MAX_TILT_DEGREES.to_radians().cos();
        let sliver = self.edge * self.edge * 1e-3;
        let (mut removed_area, mut removed_count) = (0.0f64, 0usize);
        for &k in component {
            let [w0, w1, w2] = self.world_corners(k);
            let area3 = (w1 - w0).cross(w2 - w0).length();
            let [a, b, c] = self.corners[k];
            let area2 = polygon::cross(a, b, c) as f32 * self.sign;
            if area3 > sliver && area2 < cos_tilt * area3 {
                for &slot in &mine {
                    self.fail(slot, "cut face is too steep or curved under this key");
                }
                return;
            }
            if area3 * 0.5 > sliver * 0.5 {
                removed_area += area3 as f64 * 0.5;
                removed_count += 1;
            }
        }

        // Per-key checks. A key that fails one is dropped on its own: the
        // region is still refilled (without its hole), which leaves a flat
        // face there, and the keys that passed are built as normal.
        let mut base_height: HashMap<usize, f32> = HashMap::new();
        for slot in std::mem::take(&mut mine) {
            match self.footing(slot, &rim, &locator) {
                Ok(height) => {
                    base_height.insert(slot, height);
                    mine.push(slot);
                }
                Err(why) => self.fail(slot, why),
            }
        }
        if mine.is_empty() {
            return;
        }

        // -------------------------------------------------- refill the ring, flat

        // The face is refilled up to each key's SUPPORT loop, a hair outside
        // its footprint: see the module remarks, and the strip built below.
        let mut points = rim;
        let outer_local: Vec<u32> = (0..outer.len() as u32).collect();
        let mut holes: Vec<Vec<u32>> = Vec::with_capacity(mine.len());
        for &slot in &mine {
            let first = points.len() as u32;
            points.extend_from_slice(&self.slots[slot].support);
            holes.push((first..points.len() as u32).collect());
        }
        let Ok(mut triangles) = planar::triangulate(&points, &outer_local, &holes) else {
            for &slot in &mine {
                self.fail(slot, "could not fill the cut face round this key");
            }
            return;
        };
        // Refilled at the density of what came out, measured by AREA: the cut
        // face was itself refined to an area target (`sculpt_cut::cap`), and
        // its mean edge length, skewed by the rim's short chords, undershoots
        // it, which left a visibly coarser ring round every key. The
        // refinement splits at 1.6x an equilateral triangle of the target
        // edge and settles about 0.6 of that on average, hence the 0.42.
        let refill_edge =
            if removed_count > 0 { ((removed_area / removed_count as f64) as f32 / 0.42).sqrt() } else { self.edge };
        let budget = (points.len() * 60).clamp(512, 120_000);
        planar::improve(&mut points, &mut triangles, &planar::loop_edges(&outer_local, &holes), refill_edge, budget);

        // ------------------------------------------------------------- commit

        for &k in component {
            self.job.removed[self.patch[k] as usize] = true;
        }
        // The rim keeps its vertices; every other point (the support loops,
        // and the interior points the refinement added past them) is new, and
        // is lifted onto the old face.
        let mut mesh_id: Vec<u32> = outer.clone();
        for &p in &points[outer.len()..] {
            mesh_id.push(self.job.add_vertex(locator.lift(p)));
        }

        let mut emitted = EdgeSet::default();
        for t in triangles.chunks_exact(3) {
            // The triangulator hands back counter-clockwise triangles in the
            // chart; the face runs whichever way its own triangles did.
            let (a, mut b, mut c) = (mesh_id[t[0] as usize], mesh_id[t[1] as usize], mesh_id[t[2] as usize]);
            if self.sign < 0.0 {
                std::mem::swap(&mut b, &mut c);
            }
            self.job.emit(a, b, c);
            for (from, to) in [(a, b), (b, c), (c, a)] {
                emitted.insert(directed_edge_key(from, to));
            }
        }

        for (&slot, hole) in mine.iter().zip(&holes) {
            let support: Vec<u32> = hole.iter().map(|&i| mesh_id[i as usize]).collect();
            let forward = emitted.contains(&directed_edge_key(support[0], support[1]));

            // A thin flat strip in from the support loop to the footprint
            // proper, on the face. The footprint ring's normals lean toward
            // the wall (or the hole's chamfer); with the strip, that lean is
            // blended across a few hundredths of a millimetre instead of
            // across the refilled triangles, whose long thin shapes it would
            // otherwise streak.
            let ring: Vec<u32> =
                self.slots[slot].footprint.iter().map(|&p| self.job.add_vertex(locator.lift(p))).collect();
            self.job.emit_strip(&support, &ring, forward);
            let (feature, edge) = (self.feature(slot), self.edge);
            self.job.build_wall(feature, ring, forward, base_height[&slot], edge);
        }
    }
}

fn polygon_distance(p: &[Vec2], q: &[Vec2]) -> f32 {
    if polygon::contains(p, q[0]) || polygon::contains(q, p[0]) {
        return 0.0;
    }
    let mut best = f32::MAX;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        for j in 0..q.len() {
            let (c, d) = (q[j], q[(j + 1) % q.len()]);
            if polygon::segments_cross(a, b, c, d) {
                return 0.0;
            }
            best = best.min(polygon::sqr_distance_to_segment(a, c, d)).min(polygon::sqr_distance_to_segment(c, a, b));
        }
    }
    best.sqrt()
}

// ------------------------------------------------------------ surface lookup

/// Puts a chart point back on the cut face as it was before the region was
/// taken out: find the old triangle it falls in and interpolate its corners.
/// On a flat face that is the plane itself; on a curved one it is the face's
/// own triangulation, which is the best description of it there is.
///
/// It keeps its own copy of the triangles, so it goes on answering while the
/// mesh under it is being rebuilt.
struct SurfaceLocator {
    chart: Vec<[Vec2; 3]>,
    world: Vec<[Vec3; 3]>,
    min: Vec2,
    cell: f32,
    width: usize,
    height: usize,
    /// Per grid cell, the triangles (indices into `chart`) that reach it.
    cells: Vec<Vec<u32>>,
}

impl SurfaceLocator {
    fn new(owner: &PatchJob, component: &[usize]) -> SurfaceLocator {
        let chart: Vec<[Vec2; 3]> = component.iter().map(|&k| owner.corners[k]).collect();
        let world: Vec<[Vec3; 3]> = component.iter().map(|&k| owner.world_corners(k)).collect();
        let lo = chart.iter().flatten().fold(Vec2::MAX, |m, p| m.min(*p));
        let hi = chart.iter().flatten().fold(Vec2::MIN, |m, p| m.max(*p));
        let side = ((chart.len() as f32).sqrt().ceil() as usize).clamp(1, 256);
        let cell = (hi - lo).max_element().max(1e-9) / side as f32;
        let width = (((hi.x - lo.x) / cell).ceil() as usize + 1).max(1);
        let height = (((hi.y - lo.y) / cell).ceil() as usize + 1).max(1);
        let cells = vec![Vec::new(); width * height];
        let mut locator = SurfaceLocator { chart, world, min: lo, cell, width, height, cells };
        for i in 0..locator.chart.len() {
            let [a, b, c] = locator.chart[i];
            let (x0, y0) = locator.cell_of(a.min(b).min(c));
            let (x1, y1) = locator.cell_of(a.max(b).max(c));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    locator.cells[y * width + x].push(i as u32);
                }
            }
        }
        locator
    }

    fn cell_of(&self, p: Vec2) -> (usize, usize) {
        let at = |v: f32, origin: f32, count: usize| {
            (((v - origin) / self.cell).floor().max(0.0) as usize).min(count - 1)
        };
        (at(p.x, self.min.x, self.width), at(p.y, self.min.y, self.height))
    }

    /// The triangle `p` falls in.
    fn find(&self, p: Vec2) -> Option<usize> {
        let (x, y) = self.cell_of(p);
        self.cells[y * self.width + x].iter().map(|&i| i as usize).find(|&i| {
            let [a, b, c] = self.chart[i];
            polygon::in_triangle(p, a, b, c)
        })
    }

    fn covers(&self, p: Vec2) -> bool {
        self.find(p).is_some()
    }

    fn lift(&self, p: Vec2) -> Vec3 {
        if let Some(i) = self.find(p) {
            return self.interpolate(i, p);
        }
        // Off every triangle by a rounding error (a point on the old rim):
        // the nearest triangle, with its weights clamped onto it.
        let distance = |&[a, b, c]: &[Vec2; 3]| {
            polygon::sqr_distance_to_segment(p, a, b)
                .min(polygon::sqr_distance_to_segment(p, b, c))
                .min(polygon::sqr_distance_to_segment(p, c, a))
        };
        let nearest = (0..self.chart.len())
            .min_by(|&i, &j| distance(&self.chart[i]).total_cmp(&distance(&self.chart[j])))
            .unwrap_or(0);
        self.interpolate(nearest, p)
    }

    fn interpolate(&self, i: usize, p: Vec2) -> Vec3 {
        let [a, b, c] = self.chart[i];
        let [w0, w1, w2] = self.world[i];
        let weights = polygon::barycentric(p, a, b, c).map(|w| w.map(|x| x.max(0.0)));
        let [wa, wb, wc] = weights.filter(|w| w.iter().sum::<f32>() > 0.0).unwrap_or([1.0; 3]);
        (w0 * wa + w1 * wb + w2 * wc) / (wa + wb + wc)
    }
}

/// A key's dimensions and profile are the same for its peg and its hole;
/// this makes the pair of features from one copy.
pub fn feature_pair(
    profile: &Arc<KeyProfile>,
    dimensions: KeyDimensions,
    frame: KeyFrame,
    peg_patch: usize,
    hole_patch: usize,
) -> (KeyFeature, KeyFeature) {
    let feature = |role, patch| KeyFeature { role, profile: profile.clone(), dimensions, frame, patch };
    (feature(KeyRole::Peg, peg_patch), feature(KeyRole::Hole, hole_patch))
}
