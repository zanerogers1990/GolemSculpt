//! Cuts a mesh against a screen-space shape swept along the view direction,
//! deletes what falls on one side, and closes the opening it leaves.
//!
//! Why not the voxel boolean. A boolean could express this: build a prism from
//! the dragged shape and subtract it. But it works on a lattice
//! (`sculpt_volume::boolean`): both solids are scan-converted onto one grid
//! and the result is extracted from it, which rebuilds the whole object as an
//! evenly spaced lattice and loses anything finer than one cell. (Its seams are
//! crisp, since they come from exact crossings and not from a blended distance
//! field; the cost is the resampling, not the look of the cut.) Trim is
//! supposed to leave the surface being sculpted exactly as it was and only
//! touch the boundary. So this cuts the triangles directly: every triangle
//! away from the cut survives untouched, and the ones that straddle it are
//! split exactly on the boundary.
//!
//! Where the cut is. The classification is the same screen-space test the hide
//! and mask gestures use (`ScreenRegion::contains` after
//! `ScreenProjection::project`), so what a trim removes is exactly what a hide
//! would have hidden. The exact crossing along a straddling edge is found by
//! bisecting that same test rather than by intersecting against plane
//! equations: the swept surface of a lasso is a bundle of hundreds of planes
//! (and, under a perspective camera, planes through the eye), and hunting for
//! the flip point in the one predicate that defines the region cannot disagree
//! with the classification the way a separately derived plane set can.
//!
//! Closing the hole. The edges of the surviving surface that the cut left
//! open, chained together, form the cut loops. Each loop is triangulated in
//! the swept surface's own 2D coordinates (distance along the dragged outline
//! against view depth), not flattened onto a best-fit plane. For a straight
//! cut the two are identical, but for a box the loop bends around the corner
//! between two walls, and a planar cap there would slice that corner off.
//!
//! The cap shares the loop's vertices with the shell rather than duplicating
//! them for a hard crease. Duplicates would render a touch crisper, but they
//! tear apart the moment a brush is dragged across the rim, and in a sculpting
//! app the cut is rarely the last thing that happens to that surface.
//!
//! Winding. Nothing here depends on which way round the input is wound: the
//! shell keeps the input's winding, and every cap, ring and wall takes its
//! own from the direction the shell walks the rim it is sewn to. A closed
//! mesh wound counter-clockwise from outside comes back the same.
//!
//! What is parallel. Classifying the vertices and carrying the untouched
//! vertices and triangles across are the only steps that visit the whole mesh,
//! and both run on every core. The cutting and capping only ever see the
//! triangles the boundary passes through, a few thousand on a million-triangle
//! model.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use glam::{DMat4, Vec2, Vec3};
use rayon::prelude::*;
use sculpt_geom::{SymmetryOp, TriMesh};

use crate::polygon::edge_key;
use crate::region::{CutView, ScreenRegion};

mod caps;
mod tunnel;

#[cfg(test)]
pub(crate) use caps::cap_vertex_budget;
#[cfg(test)]
pub(crate) use tunnel::{tunnel_ring_count, tunnel_ring_vertices};

/// Bisection steps for locating a crossing along a straddling edge. Each
/// halves the interval, so 20 places the cut within a millionth of one edge
/// length: far below the sub-pixel scale anything here is judged at, and only
/// ever paid on the handful of edges that straddle the boundary.
const BISECTION_STEPS: u32 = 20;

/// A vertex this close to the shape's outline on screen, in pixels, is taken
/// to be on it (see `Pass::classify`). A few thousandths of a pixel is far
/// below anything visible and a little above the rounding in projecting a
/// point, which near the right-hand edge of a wide screen is about a
/// thousandth of a pixel.
pub(crate) const ON_BOUNDARY_PIXELS: f32 = 0.004;

/// Vertices and triangles handed to one worker at a time.
const VERTEX_CHUNK: usize = 1 << 14;
const TRIANGLE_CHUNK: usize = 1 << 14;

/// "No vertex" in the internal tables.
const NONE: u32 = u32::MAX;

/// In `Trimmed::source_triangles`: this triangle is part of a new cut face.
pub const CUT_FACE: u32 = u32::MAX;

/// What one cut produced.
#[derive(Clone, Debug)]
pub struct Trimmed {
    /// The surviving geometry, closed over the cut.
    pub mesh: TriMesh,
    /// One mask value per vertex of `mesh` when a mask was passed in, empty
    /// otherwise. Surviving vertices keep their value; a vertex made by the
    /// cut takes the value interpolated along the edge it lies on, and the
    /// vertices filling a cut face blend the rim's values across it.
    pub mask: Vec<f32>,
    /// For each output triangle, the input triangle it was cut from, or
    /// `CUT_FACE`. What lets a caller that cuts in several passes follow an
    /// earlier pass's cut face through a later one.
    pub source_triangles: Vec<u32>,
    /// The cut faces that closed an ordinary opening, one per cut loop (a loop
    /// and the holes inside it count as one), as ranges of output triangles.
    /// Tunnel walls and fan fallbacks are not listed: a wall is not a face
    /// anything could be keyed to, and a fan is not a face at all.
    pub cut_faces: Vec<Range<u32>>,
    /// Cut loops that could not be closed cleanly and fell back to a fan, or
    /// were left open. Zero on any ordinary cut of a closed mesh; anything
    /// else is worth telling the user about, since a hole is invisible until
    /// the object is remeshed, booleaned or exported.
    pub open_loops: usize,
    /// Triangles the cut faces themselves contributed. They are filled at the
    /// density of the surface around them, so on a big cut they can easily
    /// outnumber what the trim removed.
    pub cap_triangles: usize,
    /// Cut faces that had to fall back to averaging their corners because the
    /// swept surface could not be evaluated for them. Diagnostic only: such a
    /// face is still closed, just filled the less accurate way.
    pub approximated_cap_faces: usize,
}

/// Cuts a mesh, in its own local space, against `region` swept along the view
/// direction, returning the surviving geometry.
///
/// `symmetry` is one symmetry operation: a point is tested through its
/// inverse, which is how one pass cuts against that operation's mirrored or
/// rotated copy of the shape. Pass `SymmetryOp::IDENTITY` for the shape as
/// drawn. Symmetry is done as one pass per operation rather than by widening
/// the test (see `trim_passes`), so each pass has a single well-defined swept
/// surface to close the hole against.
///
/// `remove_covered` false inverts the sense: everything the shape does not
/// cover is removed, a crop down to the shape.
///
/// `mask`, when given, is one value per vertex and comes back per output
/// vertex in `Trimmed::mask`.
///
/// Fails, with the reason as a phrase ("the shape covered nothing"), when the
/// cut would do nothing or would leave nothing behind.
pub fn trim(
    positions: &[Vec3],
    indices: &[u32],
    mask: Option<&[f32]>,
    view: &CutView,
    symmetry: &SymmetryOp,
    region: &ScreenRegion,
    remove_covered: bool,
) -> Result<Trimmed, String> {
    if positions.is_empty() || indices.len() < 3 {
        return Err("no geometry".to_string());
    }
    if let Some(mask) = mask {
        assert_eq!(mask.len(), positions.len(), "a mask needs one value per vertex");
    }
    // Vertices the outline passes through are treated as on it, which keeps
    // the result clean (see `Pass::classify`). Now and then that cannot be
    // done: the outline pinches the surface at such a vertex, so the cut rim
    // would have to pass through it twice. Then the cut is made again taking
    // every vertex to be on one side or the other.
    if let Some(trimmed) =
        trim_once(positions, indices, mask, view, symmetry, region, remove_covered, ON_BOUNDARY_PIXELS)?
    {
        return Ok(trimmed);
    }
    match trim_once(positions, indices, mask, view, symmetry, region, remove_covered, 0.0)? {
        Some(trimmed) => Ok(trimmed),
        None => Err("the cut could not be made".to_string()),
    }
}

/// One attempt at a cut, with vertices within `band` pixels of the outline
/// taken to be on it. `None` means the attempt has to be made again with no
/// band; with none there is always an answer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn trim_once(
    positions: &[Vec3],
    indices: &[u32],
    mask: Option<&[f32]>,
    view: &CutView,
    symmetry: &SymmetryOp,
    region: &ScreenRegion,
    remove_covered: bool,
    band: f32,
) -> Result<Option<Trimmed>, String> {
    Pass::new(positions, indices, mask, view, *symmetry, region, remove_covered, band).run()
}

/// The result of cutting an object against a shape everywhere the shape
/// applies: each place the object is drawn, under each symmetry operation.
#[derive(Clone, Debug)]
pub struct TrimSummary {
    pub mesh: TriMesh,
    /// As `Trimmed::mask`.
    pub mask: Vec<f32>,
    pub triangles_before: usize,
    /// Triangles of the new cut faces, over every pass.
    pub cap_triangles: usize,
    /// Cut loops that could not be closed cleanly, over every pass.
    pub open_loops: usize,
    /// How many passes actually cut something.
    pub cuts: usize,
}

impl TrimSummary {
    /// One line for the user. Reported as before and after plus the cut
    /// face's own share, rather than as "removed N": the face is filled at the
    /// density of the surface around it, so a big cut can easily add more
    /// triangles than it took away, and a bare "removed -5,522" reads as a bug
    /// rather than as the tool working.
    pub fn message(&self) -> String {
        let mut message = format!(
            "Trimmed: {} -> {} triangles, {} of them the new cut face.",
            grouped(self.triangles_before),
            grouped(self.mesh.triangle_count()),
            grouped(self.cap_triangles)
        );
        if self.open_loops > 0 {
            message.push_str(&open_loops_note(self.open_loops));
        }
        message
    }
}

/// The sentence appended to a result when some cut loops would not close.
pub(crate) fn open_loops_note(open_loops: usize) -> String {
    format!(" {open_loops} cut edge{} could not be closed cleanly.", if open_loops == 1 { "" } else { "s" })
}

/// A count with thousands separators.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Cuts an object against `region` once per place it is drawn and per symmetry
/// operation, each pass cutting what the one before left.
///
/// `views` has one entry per place the mesh is drawn: the object, then its
/// live mirror copies. A cut drawn over a copy cuts the vertices the copy
/// shows, which are the original's own. `symmetry` is the object's symmetry
/// (`SymmetryGroup::ops`), or empty for none.
///
/// Symmetry is applied by running one whole cut per operation rather than by
/// widening the coverage test to "covered under any copy". Each pass then has
/// a single swept surface, which is what its cut face has to be built on; a
/// test that answered for two prisms at once would leave the face with no
/// coherent surface to follow where they meet.
///
/// That holds for cutting away only. Passes in sequence remove the union of
/// the mirrored shapes, which is exactly right there; for a crop
/// (`remove_covered` false) they would keep only the overlap, the opposite of
/// what mirroring should mean, so a crop runs unmirrored and stops at the
/// first place it lands.
///
/// A pass that lands on empty space is ordinary (a shape drawn over one arm
/// has no counterpart when the other arm is turned away), so this fails only
/// if every pass misses, with a sentence saying why.
pub fn trim_passes(
    positions: &[Vec3],
    indices: &[u32],
    mask: Option<&[f32]>,
    views: &[CutView],
    symmetry: &[SymmetryOp],
    region: &ScreenRegion,
    remove_covered: bool,
) -> Result<TrimSummary, String> {
    if positions.is_empty() || indices.len() < 3 {
        return Err("No geometry.".to_string());
    }
    let identity = [SymmetryOp::IDENTITY];
    let symmetry = if symmetry.is_empty() || !remove_covered { &identity[..] } else { symmetry };

    let mut current: Option<Trimmed> = None;
    let (mut cap_triangles, mut open_loops, mut cuts) = (0, 0, 0);
    let mut last_error: Option<String> = None;
    for view in views {
        if !remove_covered && cuts > 0 {
            break;
        }
        for op in symmetry {
            let result = match &current {
                Some(cut) => {
                    let mask = mask.is_some().then_some(&cut.mask[..]);
                    trim(&cut.mesh.positions, &cut.mesh.indices, mask, view, op, region, remove_covered)
                }
                None => trim(positions, indices, mask, view, op, region, remove_covered),
            };
            match result {
                Ok(cut) => {
                    cap_triangles += cut.cap_triangles;
                    open_loops += cut.open_loops;
                    cuts += 1;
                    current = Some(cut);
                }
                Err(error) => last_error = Some(error),
            }
        }
    }
    let Some(cut) = current else {
        let reason = last_error.unwrap_or_else(|| "the shape covered nothing".to_string());
        return Err(format!("Nothing trimmed - {reason}."));
    };
    Ok(TrimSummary {
        mesh: cut.mesh,
        mask: cut.mask,
        triangles_before: indices.len() / 3,
        cap_triangles,
        open_loops,
        cuts,
    })
}

// -------------------------------------------------------------------------- one cut

/// Where a vertex stands with respect to one cut.
const KEEP: u8 = 0;
const GONE: u8 = 1;
/// On the outline, to within `ON_BOUNDARY_PIXELS`: part of whichever side
/// needs it.
const ON: u8 = 2;

/// The region test as one pass applies it.
#[derive(Clone, Copy)]
struct Probe<'a> {
    view: &'a CutView,
    symmetry: SymmetryOp,
    region: &'a ScreenRegion,
}

impl Probe<'_> {
    /// Where a local-space point lands on screen, as this pass sees the shape.
    #[inline]
    fn screen(&self, local: Vec3) -> Option<Vec2> {
        self.view.projection.project(self.symmetry.apply_inverse_point(local))
    }

    /// Whether the shape covers a local-space point.
    #[inline]
    fn covers(&self, local: Vec3) -> bool {
        self.screen(local).is_some_and(|screen| self.region.contains(screen))
    }
}

/// One cut loop expressed in the swept surface's own coordinates: `u` is
/// distance along the dragged outline, unwrapped so a loop that walks off one
/// end of the outline continues past it rather than jumping back to zero, and
/// `d` is depth from the camera. Triangulating in these coordinates is what
/// makes the cap follow the surface that did the cutting.
struct LoopChart {
    vertices: Vec<u32>,
    u: Vec<f32>,
    d: Vec<f32>,
    /// The raw, still-wrapped arc position; the tunnel stitch wants it.
    arc: Vec<f32>,
    /// Net travel along the outline over the whole loop.
    wind: f32,
    /// The loop goes the whole way round the outline: one end of a tunnel.
    wraps: bool,
    /// How far the loop travels along the outline against its own net
    /// direction, in pixels. Next to nothing for the opening of a plain
    /// tunnel, which goes round once and steadily; a good deal for one that
    /// breaks into another tunnel and has to follow its wall there and back.
    backtrack: f32,
    /// Multipliers putting `u` and `d` into the same units as the mesh itself,
    /// so that a shape judged well-proportioned in the chart really is so in
    /// 3D. Raw `u` is in screen pixels and raw `d` in view units: a chart
    /// stretched by two orders of magnitude, in which a perfectly reasonable
    /// triangulation maps to a fan of needles.
    scale_u: f32,
    scale_d: f32,
}

/// One cut. A struct rather than a pile of parameters because every step reads
/// the same dozen pieces of state, and because a trim under symmetry runs
/// several of these back to back with nothing shared between them.
struct Pass<'a> {
    verts: &'a [Vec3],
    tris: &'a [u32],
    mask: Option<&'a [f32]>,
    probe: Probe<'a>,
    remove_covered: bool,
    /// How near the outline, in pixels, a vertex counts as on it. Zero for
    /// never.
    band: f32,

    // The two inverses `surface_point` needs to turn a point on the cutting
    // surface back into geometry: view to clip is what the projection does on
    // its own, view to model undoes the rest. In double, so a rim vertex put
    // through the chart and back lands on itself.
    view_to_clip: DMat4,
    view_to_model: DMat4,

    /// `KEEP`, `GONE` or `ON` for each input vertex.
    state: Vec<u8>,
    /// Where each `KEEP` vertex went.
    remap: Vec<u32>,
    /// How many output vertices are input vertices carried across. Every
    /// vertex after them was made by the cut.
    kept_count: u32,

    out_verts: Vec<Vec3>,
    out_mask: Vec<f32>,
    out_tris: Vec<u32>,
    /// Per output triangle, the input triangle it came from.
    out_source: Vec<u32>,
    current_source: u32,
    cut_faces: Vec<Range<u32>>,

    // Crossing vertices, keyed by the source edge that produced them so the two
    // triangles sharing an edge land on the same new vertex; without that the
    // cut boundary comes apart into unconnected fragments and no loop can be
    // chained through it.
    crossings: HashMap<u64, u32>,
    // The output vertex standing for each `ON` input vertex that some
    // surviving triangle uses.
    on_vertices: HashMap<u32, u32>,
    // Edges of the surviving surface that so far have a triangle on one side
    // only, each in the direction that triangle walks it.
    open_edges: HashSet<(u32, u32)>,
    // Edges between two vertices the cut made that turned out to have a
    // surviving triangle on both sides, and how many cut-face triangles then
    // used one of them (see `emit_triangle`).
    sealed_edges: HashSet<u64>,
    doubled_edges: usize,

    // The vertices on the cut boundary, each with a slot in the lists below.
    rim_slot: HashMap<u32, u32>,
    rim_vertex: Vec<u32>,
    // Swept-surface coordinates of each rim vertex.
    cut_arc: Vec<f32>,
    cut_depth: Vec<f32>,
    // The cut boundary as directed edges: the vertex that follows each rim
    // vertex when walking the hole's rim the way the surviving surface's
    // winding implies.
    next: Vec<u32>,
    ambiguous_edges: usize,
    unclosed_loops: usize,
    chart_mapping_rejected: usize,

    // Mean edge length of the triangles the cut passed through: the density
    // the cut faces are filled at.
    shell_edge_sum: f64,
    shell_edge_count: u64,
}

impl<'a> Pass<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        verts: &'a [Vec3],
        tris: &'a [u32],
        mask: Option<&'a [f32]>,
        view: &'a CutView,
        symmetry: SymmetryOp,
        region: &'a ScreenRegion,
        remove_covered: bool,
        band: f32,
    ) -> Pass<'a> {
        let view_to_model = view.model_to_view.as_dmat4().inverse();
        Pass {
            verts,
            tris,
            mask,
            probe: Probe { view, symmetry, region },
            remove_covered,
            band,
            view_to_clip: view.projection.view_proj.as_dmat4() * view_to_model,
            view_to_model,
            state: Vec::new(),
            remap: Vec::new(),
            kept_count: 0,
            out_verts: Vec::new(),
            out_mask: Vec::new(),
            out_tris: Vec::new(),
            out_source: Vec::new(),
            current_source: CUT_FACE,
            cut_faces: Vec::new(),
            crossings: HashMap::new(),
            on_vertices: HashMap::new(),
            open_edges: HashSet::new(),
            sealed_edges: HashSet::new(),
            doubled_edges: 0,
            rim_slot: HashMap::new(),
            rim_vertex: Vec::new(),
            cut_arc: Vec::new(),
            cut_depth: Vec::new(),
            next: Vec::new(),
            ambiguous_edges: 0,
            unclosed_loops: 0,
            chart_mapping_rejected: 0,
            shell_edge_sum: 0.0,
            shell_edge_count: 0,
        }
    }

    /// Makes the cut. `None` only when taking vertices to be on the outline
    /// tied the rim in a knot (it leaves one vertex twice, or a cut face came
    /// out sharing an edge with the surface it closes) and the cut should be
    /// made again without.
    fn run(mut self) -> Result<Option<Trimmed>, String> {
        let (kept, gone) = self.classify();
        if gone == 0 {
            return Err("the shape covered nothing".to_string());
        }
        if kept == 0 {
            return Err("that would delete the whole object".to_string());
        }
        self.keep_vertices(kept);
        let cut = self.keep_whole_triangles();
        self.clip_triangles(&cut);
        if self.out_tris.is_empty() {
            return Err("that would delete the whole object".to_string());
        }
        let loops = self.chain_loops();
        if self.band > 0.0 && self.ambiguous_edges > 0 {
            return Ok(None);
        }

        let before_cap = self.out_tris.len();
        self.current_source = CUT_FACE;
        self.cap_cut_loops(loops);
        if self.band > 0.0 && self.doubled_edges > 0 {
            return Ok(None);
        }

        // Loops that could not be closed cleanly: the cut boundary ran through
        // one vertex twice (a shape whose edge grazes the surface), or a loop
        // dead-ended (the mesh had a hole of its own). Surfaced rather than
        // swallowed.
        let open_loops = self.unclosed_loops + (self.ambiguous_edges > 0) as usize;
        Ok(Some(Trimmed {
            cap_triangles: (self.out_tris.len() - before_cap) / 3,
            mesh: TriMesh::new(self.out_verts, self.out_tris),
            mask: self.out_mask,
            source_triangles: self.out_source,
            cut_faces: self.cut_faces,
            open_loops,
            approximated_cap_faces: self.chart_mapping_rejected,
        }))
    }

    fn region(&self) -> &'a ScreenRegion {
        self.probe.region
    }

    fn shell_edge_length(&self) -> f32 {
        if self.shell_edge_count > 0 { (self.shell_edge_sum / self.shell_edge_count as f64) as f32 } else { 0.0 }
    }

    // ---------------------------------------------------------------- classification

    /// Decides for every vertex whether the cut keeps it, removes it, or runs
    /// through it; returns how many it keeps and how many it removes.
    ///
    /// A vertex within `band` pixels of the outline is `ON` it. Such a vertex
    /// is not rare: starting shapes have rows of vertices on their mirror
    /// planes, "cut it down the middle" in a front view lands right on them,
    /// and a mirrored cut always meets its own first cut at vertices on the
    /// mirror plane. Forced onto one side or the other by rounding, each
    /// would be shadowed by a new vertex per edge leaving it, all in the same
    /// place, joined by triangles of no area that shade as noise and give the
    /// cut face a rim it cannot chart. Taken as on the outline, the rim simply
    /// runs through it.
    fn classify(&mut self) -> (usize, usize) {
        let (probe, remove_covered, band) = (self.probe, self.remove_covered, self.band);
        self.state = self
            .verts
            .par_iter()
            .with_min_len(VERTEX_CHUNK / 4)
            .map(|&p| match probe.screen(p) {
                Some(screen) if band > 0.0 && probe.region.near_boundary(screen, band) => ON,
                Some(screen) if probe.region.contains(screen) == remove_covered => GONE,
                Some(_) => KEEP,
                // Behind the eye: covered by nothing.
                None if remove_covered => KEEP,
                None => GONE,
            })
            .collect();
        self.state
            .par_chunks(VERTEX_CHUNK)
            .map(|chunk| {
                chunk.iter().fold((0, 0), |(kept, gone), &s| (kept + (s == KEEP) as usize, gone + (s == GONE) as usize))
            })
            .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    }

    /// Carries the kept vertices across, in their original order, and records
    /// where each went.
    fn keep_vertices(&mut self, kept: usize) {
        let state = &self.state;
        let counts: Vec<usize> =
            state.par_chunks(VERTEX_CHUNK).map(|chunk| chunk.iter().filter(|&&s| s == KEEP).count()).collect();
        let mut firsts = Vec::with_capacity(counts.len());
        let mut total = 0;
        for &count in &counts {
            firsts.push(total as u32);
            total += count;
        }

        self.remap = vec![NONE; self.verts.len()];
        self.remap.par_chunks_mut(VERTEX_CHUNK).zip(state.par_chunks(VERTEX_CHUNK)).zip(firsts.par_iter()).for_each(
            |((remap, state), &first)| {
                let mut at = first;
                for (slot, &s) in remap.iter_mut().zip(state) {
                    if s == KEEP {
                        *slot = at;
                        at += 1;
                    }
                }
            },
        );

        // Room for what the cut adds, so the vertex list is allocated once.
        let spare = kept / 16 + 4096;
        self.out_verts = compact(self.verts, state, &counts, kept, spare);
        if let Some(mask) = self.mask {
            self.out_mask = compact(mask, state, &counts, kept, spare);
        }
        self.kept_count = kept as u32;
    }

    /// Carries every triangle the cut does not touch across, and returns the
    /// triangles it does, in order: those with a vertex kept and another
    /// removed or on the outline. A triangle with no vertex kept is dropped;
    /// at most it lies along the cut, with no area on the surviving side.
    fn keep_whole_triangles(&mut self) -> Vec<u32> {
        let (state, remap) = (&self.state, &self.remap);
        let kept = |t: &[u32]| t.iter().filter(|&&v| state[v as usize] == KEEP).count();
        let scans: Vec<(usize, Vec<u32>)> = self
            .tris
            .par_chunks(TRIANGLE_CHUNK * 3)
            .enumerate()
            .map(|(chunk, corners)| {
                let (mut whole, mut cut) = (0, Vec::new());
                for (k, t) in corners.chunks_exact(3).enumerate() {
                    match kept(t) {
                        3 => whole += 1,
                        0 => {}
                        _ => cut.push((chunk * TRIANGLE_CHUNK + k) as u32),
                    }
                }
                (whole, cut)
            })
            .collect();
        let whole: usize = scans.iter().map(|scan| scan.0).sum();
        let cut: Vec<u32> = scans.iter().flat_map(|scan| scan.1.iter().copied()).collect();

        let spare = whole / 16 + cut.len() * 2 + 4096;
        let mut out_tris = Vec::with_capacity((whole + spare) * 3);
        out_tris.resize(whole * 3, 0u32);
        let mut out_source = Vec::with_capacity(whole + spare);
        out_source.resize(whole, 0u32);
        let (mut tri_parts, mut source_parts) = (Vec::with_capacity(scans.len()), Vec::with_capacity(scans.len()));
        let (mut tri_rest, mut source_rest) = (&mut out_tris[..], &mut out_source[..]);
        for scan in &scans {
            let (tri_head, tri_tail) = tri_rest.split_at_mut(scan.0 * 3);
            let (source_head, source_tail) = source_rest.split_at_mut(scan.0);
            tri_parts.push(tri_head);
            source_parts.push(source_head);
            (tri_rest, source_rest) = (tri_tail, source_tail);
        }
        tri_parts.into_par_iter().zip(source_parts).zip(self.tris.par_chunks(TRIANGLE_CHUNK * 3)).enumerate().for_each(
            |(chunk, ((out, source), corners))| {
                let mut at = 0;
                for (k, t) in corners.chunks_exact(3).enumerate() {
                    if kept(t) == 3 {
                        for c in 0..3 {
                            out[at * 3 + c] = remap[t[c] as usize];
                        }
                        source[at] = (chunk * TRIANGLE_CHUNK + k) as u32;
                        at += 1;
                    }
                }
            },
        );
        self.out_tris = out_tris;
        self.out_source = out_source;
        cut
    }

    // ------------------------------------------------------------------- the cutting

    fn add_vertex(&mut self, position: Vec3, mask: f32) -> u32 {
        self.out_verts.push(position);
        if self.mask.is_some() {
            self.out_mask.push(mask);
        }
        self.out_verts.len() as u32 - 1
    }

    fn mask_of(&self, vertex: u32) -> f32 {
        if self.mask.is_some() { self.out_mask[vertex as usize] } else { 0.0 }
    }

    /// The output vertex of an input vertex that survives: a kept one's place
    /// among the kept, or the one copy of a vertex on the outline. Those are
    /// made here, only when a surviving triangle wants one, rather than
    /// carried across with the kept: a vertex on the outline with nothing
    /// kept around it has no triangle left, and would otherwise sit in the
    /// result attached to nothing.
    fn surviving_vertex(&mut self, original: u32) -> u32 {
        let kept = self.remap[original as usize];
        if kept != NONE {
            return kept;
        }
        if let Some(&copy) = self.on_vertices.get(&original) {
            return copy;
        }
        let mask = self.mask.map_or(0.0, |mask| mask[original as usize]);
        let copy = self.add_vertex(self.verts[original as usize], mask);
        self.on_vertices.insert(original, copy);
        copy
    }

    /// The vertex where the cut meets the edge between a surviving vertex and
    /// a removed one, made once per edge.
    ///
    /// From a vertex on the outline that is the vertex itself. Otherwise it is
    /// where the region test flips: pure bisection on the same predicate that
    /// classified the ends, so the point is on the boundary by construction
    /// and there is no separate surface it could be off by. The walk always
    /// runs from the uncovered end to the covered one, whichever of them this
    /// pass keeps, so cutting the same mesh once for each side (see `split`)
    /// puts both pieces' rims on exactly the same points.
    fn crossing_vertex(&mut self, survivor: u32, removed: u32) -> u32 {
        if self.state[survivor as usize] == ON {
            return self.surviving_vertex(survivor);
        }
        let key = edge_key(survivor, removed);
        if let Some(&existing) = self.crossings.get(&key) {
            return existing;
        }
        let (outside, inside) = if self.remove_covered { (survivor, removed) } else { (removed, survivor) };
        let (from, to) = (self.verts[outside as usize], self.verts[inside as usize]);
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..BISECTION_STEPS {
            let mid = (lo + hi) * 0.5;
            if self.probe.covers(from.lerp(to, mid)) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let t = (lo + hi) * 0.5;
        let mask = match self.mask {
            Some(mask) => mask[outside as usize] + (mask[inside as usize] - mask[outside as usize]) * t,
            None => 0.0,
        };
        let index = self.add_vertex(from.lerp(to, t), mask);
        self.crossings.insert(key, index);
        index
    }

    /// Adds a triangle of a cut face.
    ///
    /// A cut face joins rim vertices to each other as it pleases, and with
    /// vertices on the outline two of them may already be joined: by an edge
    /// of the surviving surface that runs from one stretch of the rim to
    /// another. A face that used that edge too would put four triangles round
    /// it. They are counted here, and the cut made again the long way.
    fn emit_triangle(&mut self, a: u32, b: u32, c: u32) {
        // A triangle collapsed to a line contributes nothing but a zero-area
        // face that then poisons its vertices' normals.
        if a == b || b == c || c == a {
            return;
        }
        self.out_tris.extend_from_slice(&[a, b, c]);
        self.out_source.push(self.current_source);
        if !self.sealed_edges.is_empty() {
            let doubled =
                [(a, b), (b, c), (c, a)].iter().filter(|&&(u, v)| self.sealed_edges.contains(&edge_key(u, v))).count();
            self.doubled_edges += doubled;
        }
    }

    /// Adds a triangle of the surviving surface, and notes which of its edges
    /// may be left open by the cut.
    ///
    /// Only an edge between two vertices the cut made can be: every other edge
    /// has a kept vertex, and the triangle across it survives at least in
    /// part. Each such edge is recorded the way this triangle walks it, and
    /// struck out again if the triangle across turns up walking it back. What
    /// is left when every triangle is in is exactly the rim of the hole,
    /// directed the way the surviving surface's winding runs, which is what
    /// makes the cut edges chain into loops with a consistent direction.
    fn emit_shell_triangle(&mut self, a: u32, b: u32, c: u32) {
        if a == b || b == c || c == a {
            return;
        }
        self.out_tris.extend_from_slice(&[a, b, c]);
        self.out_source.push(self.current_source);
        for (from, to) in [(a, b), (b, c), (c, a)] {
            if from < self.kept_count || to < self.kept_count {
                continue;
            }
            if self.open_edges.remove(&(to, from)) {
                self.sealed_edges.insert(edge_key(from, to));
            } else if !self.open_edges.insert((from, to)) {
                // Two triangles walking one edge the same way: the input was
                // not a clean surface there.
                self.ambiguous_edges += 1;
            }
        }
    }

    fn triangle_count(&self) -> u32 {
        (self.out_tris.len() / 3) as u32
    }

    /// The classic triangle-against-halfspace case split, except the
    /// "halfspace" is the swept region and a corner may lie on its boundary.
    /// Such a corner counts as surviving, and the cut meets any edge from it
    /// at the corner itself, so the piece on the far side of it comes out as a
    /// line and is dropped.
    fn clip_triangles(&mut self, cut: &[u32]) {
        for &t in cut {
            let base = t as usize * 3;
            let i = [self.tris[base], self.tris[base + 1], self.tris[base + 2]];
            let gone = i.map(|v| self.state[v as usize] == GONE);
            self.current_source = t;

            // The surface's own edge length right where the cut lands, which
            // is the density the cap has to be filled at. Measured from the
            // triangles being cut, never from the resulting rim: a cut crosses
            // a triangle on a chord, and a crossing that lands near an existing
            // vertex leaves a rim edge near zero, so rim edges come out far
            // shorter than the mesh they were cut from (0.35x on a sphere,
            // which as a density target over-fills the cap eightfold).
            let [v0, v1, v2] = i.map(|v| self.verts[v as usize]);
            self.shell_edge_sum += (v0.distance(v1) + v1.distance(v2) + v2.distance(v0)) as f64;
            self.shell_edge_count += 3;

            match gone.iter().filter(|&&g| g).count() {
                0 => {
                    // Nothing removed: it has a corner on the outline, and is
                    // here only for that corner's vertex.
                    let [a, b, c] = i.map(|v| self.surviving_vertex(v));
                    self.emit_shell_triangle(a, b, c);
                }
                1 => {
                    // Rotated to the cyclic order (a survives, b survives, c
                    // removed), which keeps the source winding whichever
                    // corner was the removed one.
                    let k = gone.iter().position(|&g| g).unwrap_or(0);
                    let (c, a, b) = (i[k], i[(k + 1) % 3], i[(k + 2) % 3]);
                    let p = self.crossing_vertex(b, c);
                    let q = self.crossing_vertex(a, c);
                    let (ra, rb) = (self.surviving_vertex(a), self.surviving_vertex(b));
                    // The surviving quad a-b-P-Q, fanned from a.
                    self.emit_shell_triangle(ra, rb, p);
                    self.emit_shell_triangle(ra, p, q);
                }
                _ => {
                    // Cyclic order (a survives, b removed, c removed).
                    let k = gone.iter().position(|&g| !g).unwrap_or(0);
                    let (a, b, c) = (i[k], i[(k + 1) % 3], i[(k + 2) % 3]);
                    let p = self.crossing_vertex(a, b);
                    let q = self.crossing_vertex(a, c);
                    let ra = self.surviving_vertex(a);
                    self.emit_shell_triangle(ra, p, q);
                }
            }
        }
    }

    // --------------------------------------------------------------- closing the hole

    /// The slot of a vertex on the cut boundary, giving it one (and working
    /// out where it sits on the swept surface) the first time it is met.
    fn rim_slot_of(&mut self, vertex: u32) -> usize {
        if let Some(&slot) = self.rim_slot.get(&vertex) {
            return slot as usize;
        }
        let tested = self.probe.symmetry.apply_inverse_point(self.out_verts[vertex as usize]);
        let screen = self.probe.view.projection.project(tested).unwrap_or_default();
        self.cut_arc.push(self.region().arc_position(screen));
        // View space looks down -Z, so a point in front of the camera has
        // negative z and this is its distance ahead of the eye.
        self.cut_depth.push(-self.probe.view.model_to_view.transform_point3(tested).z);
        self.next.push(NONE);
        self.rim_vertex.push(vertex);
        self.rim_slot.insert(vertex, self.rim_vertex.len() as u32 - 1);
        self.rim_vertex.len() - 1
    }

    /// Chains the open edges into closed loops of rim vertices.
    fn chain_loops(&mut self) -> Vec<Vec<u32>> {
        // Sorted, so the same cut always numbers its new vertices the same way.
        let mut edges: Vec<(u32, u32)> = self.open_edges.drain().collect();
        edges.sort_unstable();
        for (from, to) in edges {
            let slot = self.rim_slot_of(from);
            self.rim_slot_of(to);
            if self.next[slot] != NONE {
                // The rim leaves this vertex twice: the outline pinches the
                // surface here.
                self.ambiguous_edges += 1;
                continue;
            }
            self.next[slot] = to;
        }

        let mut visited = vec![false; self.next.len()];
        let mut loops = Vec::new();
        for start_slot in 0..self.next.len() {
            if self.next[start_slot] == NONE || visited[start_slot] {
                continue;
            }
            let start = self.rim_vertex[start_slot];
            let mut chain = Vec::new();
            let mut closed = false;
            let mut current = start;
            loop {
                let slot = self.rim_slot[&current] as usize;
                if std::mem::replace(&mut visited[slot], true) {
                    break;
                }
                chain.push(current);
                let following = self.next[slot];
                if following == NONE {
                    break;
                }
                if following == start {
                    closed = true;
                    break;
                }
                current = following;
            }
            if closed && chain.len() >= 3 {
                loops.push(chain);
            } else {
                self.unclosed_loops += 1;
            }
        }
        loops
    }
}

/// The entries of `source` whose vertex is kept, in order, with `spare`
/// further slots of capacity. `counts` is the number kept in each
/// `VERTEX_CHUNK` of `state`.
fn compact<T: Copy + Default + Send + Sync>(
    source: &[T],
    state: &[u8],
    counts: &[usize],
    kept: usize,
    spare: usize,
) -> Vec<T> {
    let mut out = Vec::with_capacity(kept + spare);
    out.resize(kept, T::default());
    let mut parts = Vec::with_capacity(counts.len());
    let mut rest = &mut out[..];
    for &count in counts {
        let (head, tail) = rest.split_at_mut(count);
        parts.push(head);
        rest = tail;
    }
    parts.into_par_iter().zip(source.par_chunks(VERTEX_CHUNK)).zip(state.par_chunks(VERTEX_CHUNK)).for_each(
        |((out, source), state)| {
            let mut at = 0;
            for (value, &s) in source.iter().zip(state) {
                if s == KEEP {
                    out[at] = *value;
                    at += 1;
                }
            }
        },
    );
    out
}

/// The signed step between two arc positions, taken the short way around the
/// closed outline: a loop is walked in small hops, so a raw difference of
/// nearly a whole perimeter always means it crossed the outline's seam rather
/// than sprinted around it.
fn shortest_arc_step(delta: f32, perimeter: f32) -> f32 {
    if perimeter <= 0.0 { delta } else { delta - perimeter * (delta / perimeter).round() }
}
