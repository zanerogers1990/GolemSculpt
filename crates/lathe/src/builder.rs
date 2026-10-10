//! Revolves a [`LatheProfile`] around the local Y axis into a clean, welded triangle mesh.
//!
//! The geometry is built for sculpting, not just for looking at:
//!   - the profile is resampled at an even arc-length spacing matched to the widest ring's vertex
//!     spacing, so faces come out close to square rather than long strips;
//!   - with `even_triangles`, each ring gets a vertex count proportional to its radius, and rings of
//!     different counts are zipped together, so there are no sliver fans at the poles;
//!   - an end on the axis becomes ONE pole vertex (never a ring of coincident vertices), so the mesh
//!     is 2-manifold and closed - ready for brushes, remesh, booleans and the mold maker;
//!   - it is EXACTLY mirror-symmetric across both the X and Z planes, positions and topology alike
//!     (see [`crate::rings`]), because this app's mirrored sculpting pairs vertices by position and
//!     smooths them over their neighbours.
//!
//! Winding is counter-clockwise seen from outside, so a closed result has a positive
//! [`TriMesh::signed_volume`]. Vertices go round from +X toward +Z.
//!
//! A [`LatheBuilder`] and a [`LatheBuild`] are meant to be kept and reused: a live drag rebuilds
//! into the same buffers on every pointer move and allocates nothing once they have grown to size.

use glam::{Vec2, Vec3};
use rayon::prelude::*;
use sculpt_geom::TriMesh;

use crate::curve::{self, ResampleScratch, Sample};
use crate::outline;
use crate::profile::LatheProfile;
use crate::rings::{self, RingRef};

pub const MIN_SEGMENTS: usize = 8;
pub const MAX_SEGMENTS: usize = 1024;

/// Upper bound on profile samples, so a tall, thin profile cannot ask for an absurd number of
/// rings. Past this the profile spacing grows instead.
pub const MAX_PROFILE_SAMPLES: usize = 4096;

/// Upper bound on (profile samples x radial segments) - the vertex count before even triangles
/// thins the rings near the axis. Keeps high detail on a tall, thin profile from asking for several
/// million vertices; past it both directions coarsen together.
pub const MAX_VERTICES: usize = 1_000_000;

/// The preview never builds finer than this, whatever the detail setting - above it smooth analytic
/// normals already show the shape exactly, and a million-triangle rebuild on every slider tick
/// would stall the app. The final mesh is always built at full detail.
pub const PREVIEW_MAX_SEGMENTS: usize = 256;

/// Output smaller than this many vertices (or three times as many indices) is filled on one thread:
/// forking costs more than the fill saves.
const PARALLEL_LEAF_VERTICES: usize = 8192;

/// Rounds a detail request to what the builder uses: a multiple of 4 within the allowed range.
pub fn round_segments(segments: usize) -> usize {
    ((segments as f32 / 4.0).round_ties_even() as usize * 4).clamp(MIN_SEGMENTS, MAX_SEGMENTS)
}

/// Detail and closure options for a lathe mesh. The SHAPE lives in [`LatheProfile`]; these only
/// decide how it is tessellated and whether open ends get closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatheSettings {
    /// Vertices around the widest ring. Rounded to a multiple of 4 (see [`round_segments`]).
    pub radial_segments: usize,
    /// Closes an open end that stops short of the axis with a flat disc, so the result is a
    /// watertight solid. Off leaves the end open (a vase, a tube).
    pub cap_ends: bool,
    /// Fewer vertices on rings near the axis, so triangles stay close to the same size all over
    /// instead of squeezing into slivers at the poles. What sculpting brushes want.
    pub even_triangles: bool,
}

impl Default for LatheSettings {
    fn default() -> LatheSettings {
        LatheSettings { radial_segments: 256, cap_ends: true, even_triangles: true }
    }
}

impl LatheSettings {
    /// The settings for the live preview: never finer than [`PREVIEW_MAX_SEGMENTS`], and with
    /// `draft` (a rebuild that has been too slow to keep up with a drag) half of that again.
    pub fn for_preview(self, draft: bool) -> LatheSettings {
        let mut settings = self;
        settings.radial_segments = settings.radial_segments.min(PREVIEW_MAX_SEGMENTS);
        if draft {
            settings.radial_segments = round_segments(settings.radial_segments / 2);
        }
        settings
    }
}

/// What a build would make, without making it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatheCounts {
    pub vertices: usize,
    pub triangles: usize,
}

/// Everything one build produced: the mesh, its smooth normals, the tessellated profile (for
/// drawing the silhouette exactly as the mesh has it), and what is worth telling the artist about
/// it. Reused across builds, so a live drag rebuilds without allocating.
#[derive(Clone, Debug, Default)]
pub struct LatheBuild {
    pub mesh: TriMesh,
    /// One unit normal per vertex, smooth across the whole surface (a pole's points along the
    /// axis). The mesh itself carries none.
    pub normals: Vec<Vec3>,
    /// The profile as tessellated - caps included, oriented so the surface faces outward - in
    /// (radius, height).
    pub profile: Vec<Vec2>,
    pub profile_is_loop: bool,
    /// No boundary edges: both ends closed (on the axis or capped), or a loop.
    pub watertight: bool,
    /// The profile crosses itself, so the revolved surface passes through itself.
    pub self_intersecting: bool,
    /// Part of the curve swung across the axis between two points and was held just off it - the
    /// shape has a very thin neck there, which is rarely what was meant.
    pub touches_axis: bool,
}

impl LatheBuild {
    pub fn vertex_count(&self) -> usize {
        self.mesh.vertex_count()
    }

    pub fn triangle_count(&self) -> usize {
        self.mesh.triangle_count()
    }

    /// Empties the build, keeping the buffers' capacity.
    pub fn clear(&mut self) {
        self.mesh.positions.clear();
        self.mesh.indices.clear();
        self.normals.clear();
        self.profile.clear();
        self.profile_is_loop = false;
        self.watertight = false;
        self.self_intersecting = false;
        self.touches_axis = false;
    }

    /// Takes the mesh, leaving the build empty of geometry.
    pub fn into_mesh(self) -> TriMesh {
        self.mesh
    }

    /// Bakes `scale` into the vertices and moves the origin to the mesh's vertical centre on the
    /// axis, so a created object has a unit transform and its pivot - and its mirror plane - sit in
    /// the middle of it. Returns that centre height (in unscaled profile units), which is where the
    /// object goes. Multiplying both halves of a mirror pair by the same factor keeps them exact
    /// negations of each other.
    pub fn centre_and_scale(&mut self, scale: f32) -> f32 {
        let (min, max) = self
            .mesh
            .positions
            .par_iter()
            .map(|p| (p.y, p.y))
            .reduce(|| (f32::MAX, f32::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
        if min > max {
            return 0.0;
        }
        let centre = (min + max) * 0.5;
        self.mesh.positions.par_iter_mut().for_each(|p| {
            *p = Vec3::new(p.x * scale, (p.y - centre) * scale, p.z * scale);
        });
        centre
    }
}

/// One-shot build into a fresh mesh. A live preview should keep a [`LatheBuilder`] instead.
pub fn build_mesh(profile: &LatheProfile, settings: &LatheSettings) -> Result<TriMesh, String> {
    let mut out = LatheBuild::default();
    LatheBuilder::new().build(profile, settings, &mut out)?;
    Ok(out.into_mesh())
}

/// A ring of the revolved surface before it is written out.
#[derive(Clone, Copy)]
struct RingPlan {
    radius: f32,
    height: f32,
    /// Outward normal in (radius, height); for a pole, straight along the axis.
    normal: Vec2,
    count: u32,
}

struct Prepared {
    spacing: f32,
    segments: usize,
    closed_loop: bool,
    touches_axis: bool,
}

struct Totals {
    vertices: usize,
    indices: usize,
    start_pole: bool,
    end_pole: bool,
}

/// Reusable working storage for building lathe meshes: the resampler's buffers, the ring plan and
/// the cached trig tables.
pub struct LatheBuilder {
    resampler: ResampleScratch,
    samples: Vec<Sample>,
    counting_profile: Vec<Vec2>,
    rings: Vec<RingPlan>,
    /// First vertex of each ring, plus the total at the end.
    vertex_offsets: Vec<usize>,
    /// First index of each strip (the band from ring i to ring i+1), plus the total at the end.
    index_offsets: Vec<usize>,
    /// Unit-circle tables by ring vertex count / 4.
    angle_tables: Vec<Vec<[f32; 2]>>,
    /// Pieces of output smaller than this are filled on one thread. A field so tests can force the
    /// single-threaded path and compare.
    pub(crate) parallel_leaf_vertices: usize,
}

impl Default for LatheBuilder {
    fn default() -> LatheBuilder {
        LatheBuilder::new()
    }
}

impl LatheBuilder {
    pub fn new() -> LatheBuilder {
        LatheBuilder {
            resampler: ResampleScratch::default(),
            samples: Vec::new(),
            counting_profile: Vec::new(),
            rings: Vec::new(),
            vertex_offsets: Vec::new(),
            index_offsets: Vec::new(),
            angle_tables: Vec::new(),
            parallel_leaf_vertices: PARALLEL_LEAF_VERTICES,
        }
    }

    /// Revolves `profile` into `out`, replacing what it held. On failure `out` is left empty and
    /// the error says, in words the artist can act on, what is wrong with the profile.
    pub fn build(
        &mut self,
        profile: &LatheProfile,
        settings: &LatheSettings,
        out: &mut LatheBuild,
    ) -> Result<(), String> {
        let prepared = match self.prepare(profile, settings, &mut out.profile) {
            Ok(prepared) => prepared,
            Err(error) => {
                out.clear();
                return Err(error);
            }
        };
        let closed_loop = prepared.closed_loop;
        out.profile_is_loop = closed_loop;
        out.touches_axis = prepared.touches_axis;
        out.self_intersecting = outline::crosses_itself(&out.profile, closed_loop);

        let totals = self.plan(&out.profile, &prepared, settings.even_triangles, true);
        out.watertight = closed_loop || (totals.start_pole && totals.end_pole);

        // Resizing rather than clearing: a rebuild at the same size writes over what is there
        // instead of zeroing it first.
        out.mesh.positions.resize(totals.vertices, Vec3::ZERO);
        out.normals.resize(totals.vertices, Vec3::ZERO);
        out.mesh.indices.resize(totals.indices, 0);

        let fill = Fill {
            rings: &self.rings,
            vertex_offsets: &self.vertex_offsets,
            index_offsets: &self.index_offsets,
            angle_tables: &self.angle_tables,
            closed_loop,
            leaf_vertices: self.parallel_leaf_vertices,
        };
        let strips = self.index_offsets.len() - 1;
        let (positions, normals, indices) = (&mut out.mesh.positions, &mut out.normals, &mut out.mesh.indices);
        if totals.vertices <= fill.leaf_vertices {
            fill.fill_rings(0, self.rings.len(), positions, normals);
            fill.fill_strips(0, strips, indices);
        } else {
            rayon::join(
                || fill.fill_rings(0, self.rings.len(), positions, normals),
                || fill.fill_strips(0, strips, indices),
            );
        }
        Ok(())
    }

    /// The vertex and triangle counts [`build`](LatheBuilder::build) would produce with these
    /// settings, without building it - only the profile is tessellated. Lets the preview run at a
    /// lighter detail than the final mesh while the panel still reports what the final one will be.
    pub fn count(&mut self, profile: &LatheProfile, settings: &LatheSettings) -> Result<LatheCounts, String> {
        let mut outline = std::mem::take(&mut self.counting_profile);
        let counts = self.prepare(profile, settings, &mut outline).map(|prepared| {
            let totals = self.plan(&outline, &prepared, settings.even_triangles, false);
            LatheCounts { vertices: totals.vertices, triangles: totals.indices / 3 }
        });
        self.counting_profile = outline;
        counts
    }

    /// Validation, spacing and the tessellated, outward-oriented profile written to `outline` -
    /// everything `build` and `count` share.
    fn prepare(
        &mut self,
        profile: &LatheProfile,
        settings: &LatheSettings,
        outline: &mut Vec<Vec2>,
    ) -> Result<Prepared, String> {
        outline.clear();
        let mut segments = round_segments(settings.radial_segments);
        if profile.len() < profile.min_points() {
            return Err(if profile.closed_loop() {
                "A closed loop needs at least 3 points.".to_string()
            } else {
                "Click in the viewport to add profile points (at least 2).".to_string()
            });
        }
        if profile.points().iter().any(|p| !p.position.is_finite()) {
            return Err("A profile point is not a number.".to_string());
        }

        let max_radius = profile.max_radius();
        if max_radius < profile.size() * 1e-3 {
            return Err("Every point is on the axis - drag one out to give the shape some width.".to_string());
        }

        let closed_loop = profile.closed_loop();
        let circumference = 2.0 * std::f32::consts::PI * max_radius;

        // The spacing that makes a face on the widest ring square.
        let mut spacing = circumference / segments as f32;
        let mut estimated_length = curve::control_polygon_length(profile.points(), closed_loop) * 1.25;
        if settings.cap_ends && !closed_loop {
            estimated_length += 2.0 * max_radius;
        }

        // Over the vertex budget: coarsen BOTH directions by the same factor, so faces stay square
        // and asking for more detail saturates at the budget instead of getting coarser. (Widening
        // only the profile spacing would also thin every even-triangles ring, so a tall, thin
        // profile came out coarser at 1024 than at 256.)
        let vertices = estimated_length as f64 / spacing as f64 * segments as f64;
        if vertices > MAX_VERTICES as f64 {
            let k = (vertices / MAX_VERTICES as f64).sqrt() as f32;
            segments = round_segments((segments as f32 / k).floor() as usize);
            spacing = circumference / segments as f32;
        }
        // A profile absurdly long for its radius: past this the profile spacing grows instead.
        if estimated_length / spacing > MAX_PROFILE_SAMPLES as f32 {
            spacing = estimated_length / MAX_PROFILE_SAMPLES as f32;
        }

        self.samples.clear();
        self.resampler.resample(profile.points(), closed_loop, spacing, &mut self.samples);
        if self.samples.len() < 2 {
            return Err("The profile is too short to revolve.".to_string());
        }

        let rules = outline::tessellate(
            &self.samples,
            closed_loop,
            settings.cap_ends,
            spacing,
            profile.min_interior_radius(),
            outline,
        );
        if outline.len() < if closed_loop { 3 } else { 2 } {
            return Err("The profile is too short to revolve.".to_string());
        }

        outline::orient_outward(outline, closed_loop);
        Ok(Prepared { spacing, segments, closed_loop, touches_axis: rules.touches_axis })
    }

    /// Decides every ring's vertex count and where everything lands in the output. With `tables`
    /// the trig tables the fill will read are made too.
    fn plan(&mut self, outline: &[Vec2], prepared: &Prepared, even: bool, tables: bool) -> Totals {
        let Prepared { spacing, segments, closed_loop, .. } = *prepared;
        let count = outline.len();
        let start_pole = !closed_loop && outline[0].x <= 0.0;
        let end_pole = !closed_loop && outline[count - 1].x <= 0.0;

        self.rings.clear();
        self.vertex_offsets.clear();
        let mut vertices = 0;
        for (i, p) in outline.iter().enumerate() {
            let pole = (i == 0 && start_pole) || (i == count - 1 && end_pole);
            let ring = if pole { 1 } else { rings::ring_vertex_count(p.x, spacing, segments, even) };
            let mut normal = outline::normal(outline, i, closed_loop);
            if pole {
                let sign = if normal.y != 0.0 {
                    normal.y.signum()
                } else if i == 0 {
                    -1.0
                } else {
                    1.0
                };
                normal = Vec2::new(0.0, sign);
            } else if tables {
                self.ensure_angle_table(ring);
            }
            self.rings.push(RingPlan { radius: p.x, height: p.y, normal, count: ring as u32 });
            self.vertex_offsets.push(vertices);
            vertices += ring;
        }
        self.vertex_offsets.push(vertices);

        self.index_offsets.clear();
        let mut indices = 0;
        let strips = if closed_loop { count } else { count - 1 };
        for i in 0..strips {
            self.index_offsets.push(indices);
            indices += 3 * rings::strip_triangles(self.rings[i].count, self.rings[(i + 1) % count].count);
        }
        self.index_offsets.push(indices);

        debug_assert!(vertices <= u32::MAX as usize);
        Totals { vertices, indices, start_pole, end_pole }
    }

    fn ensure_angle_table(&mut self, ring_count: usize) {
        let slot = ring_count / 4;
        if self.angle_tables.len() <= slot {
            self.angle_tables.resize_with(slot + 1, Vec::new);
        }
        if self.angle_tables[slot].is_empty() {
            self.angle_tables[slot] = rings::angle_table(ring_count);
        }
    }
}

/// The plan, borrowed for writing the output. Both fills split their output buffers in half and
/// hand the halves to `rayon::join` until a piece is small, so they need no allocation and no
/// unsafe code.
struct Fill<'a> {
    rings: &'a [RingPlan],
    vertex_offsets: &'a [usize],
    index_offsets: &'a [usize],
    angle_tables: &'a [Vec<[f32; 2]>],
    closed_loop: bool,
    leaf_vertices: usize,
}

impl Fill<'_> {
    /// Writes rings `lo..hi`; `positions` and `normals` cover exactly their vertices.
    fn fill_rings(&self, lo: usize, hi: usize, positions: &mut [Vec3], normals: &mut [Vec3]) {
        let vertices = self.vertex_offsets[hi] - self.vertex_offsets[lo];
        if hi - lo > 1 && vertices > self.leaf_vertices {
            let mid = (lo + hi) / 2;
            let cut = self.vertex_offsets[mid] - self.vertex_offsets[lo];
            let (positions_a, positions_b) = positions.split_at_mut(cut);
            let (normals_a, normals_b) = normals.split_at_mut(cut);
            rayon::join(
                || self.fill_rings(lo, mid, positions_a, normals_a),
                || self.fill_rings(mid, hi, positions_b, normals_b),
            );
            return;
        }

        let base = self.vertex_offsets[lo];
        for i in lo..hi {
            let ring = &self.rings[i];
            let range = self.vertex_offsets[i] - base..self.vertex_offsets[i + 1] - base;
            let (positions, normals) = (&mut positions[range.clone()], &mut normals[range]);
            if ring.count == 1 {
                positions[0] = Vec3::new(0.0, ring.height, 0.0);
                normals[0] = Vec3::new(0.0, ring.normal.y, 0.0);
                continue;
            }
            let table = &self.angle_tables[ring.count as usize / 4];
            for ((p, n), &[cos, sin]) in positions.iter_mut().zip(normals.iter_mut()).zip(table) {
                *p = Vec3::new(ring.radius * cos, ring.height, ring.radius * sin);
                *n = Vec3::new(ring.normal.x * cos, ring.normal.y, ring.normal.x * sin);
            }
        }
    }

    /// Writes strips `lo..hi`; `indices` covers exactly their triangles.
    fn fill_strips(&self, lo: usize, hi: usize, indices: &mut [u32]) {
        let size = self.index_offsets[hi] - self.index_offsets[lo];
        if hi - lo > 1 && size > self.leaf_vertices.saturating_mul(3) {
            let mid = (lo + hi) / 2;
            let cut = self.index_offsets[mid] - self.index_offsets[lo];
            let (a, b) = indices.split_at_mut(cut);
            rayon::join(|| self.fill_strips(lo, mid, a), || self.fill_strips(mid, hi, b));
            return;
        }

        let base = self.index_offsets[lo];
        let ring_at = |i: usize| {
            let i = if self.closed_loop { i % self.rings.len() } else { i };
            RingRef { start: self.vertex_offsets[i] as u32, count: self.rings[i].count }
        };
        for i in lo..hi {
            let range = self.index_offsets[i] - base..self.index_offsets[i + 1] - base;
            rings::write_strip(ring_at(i), ring_at(i + 1), &mut indices[range]);
        }
    }
}
