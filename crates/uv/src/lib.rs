//! UVs made for a mesh that has none, so maps can be baked onto it.
//!
//! A sculpt has no UVs, and a Quad Remesh of one has none either. Before a
//! normal map or any other map can be baked onto the remeshed model, every
//! triangle needs a place in the square of the picture, with no two places
//! overlapping. Finding those places is an unwrap, and it has three parts:
//!
//! 1. **Cutting into charts.** The surface is cut into pieces that each bend
//!    little enough to be laid flat without much stretching. A piece is grown
//!    from the flattest unclaimed part of the surface, taking in neighbouring
//!    faces while they stay within [`Options::max_angle_degrees`] of the
//!    piece's average direction, and preferring to stop at creases.
//! 2. **Laying each chart flat.** Least Squares Conformal Maps (LSCM) keeps
//!    the angles of every triangle, then a few rounds of as-rigid-as-possible
//!    (ARAP) even out the sizes, so a texel covers about the same patch of
//!    surface everywhere on the chart. A chart that folds over itself, flips a
//!    triangle or stretches too much is cut in two and both halves are tried
//!    again, so the result is always valid.
//! 3. **Packing.** Every chart is turned to its tightest bounding rectangle,
//!    drawn into a grid with a margin round it, and placed where it fits
//!    lowest, nesting into the hollows of charts placed before it. All charts
//!    share one scale, so texels are the same size all over the model.
//!
//! The work is done by the `uvunwrap` crate (version 0.3.0, MIT), which was
//! read through before it was taken on: it forbids unsafe code, has no build
//! script, touches no files, network or processes, and refuses bad input
//! with an error rather than panicking. What this crate adds is the
//! sculptor's side of the join:
//!
//! * **Quads stay whole.** Given the polygon faces a Quad Remesh made, each
//!   polygon's triangles always share a chart, so seams run along polygon
//!   edges and every corner of a quad has one UV.
//! * **Face groups are seams.** Given face groups, no chart crosses from one
//!   group to another: painting groups is how a sculptor says where the seams
//!   go.
//! * **Padding as a share of the square**, so it holds whatever size the map
//!   is baked at, and the square kept square: the packer crops its page to
//!   the rows it used, and that crop is undone so one UV unit is the same
//!   length across and down.
//! * **Plain refusals** for what cannot be unwrapped, and a measure of how
//!   much the result stretches ([`Unwrapped::stretch`]).
//!
//! # Why the direct solver
//!
//! Laying a chart flat is a sparse least-squares problem, one unknown per UV
//! coordinate. `uvunwrap` solves it either by conjugate gradients, which
//! needs nothing more, or by a sparse Cholesky factorization from the `faer`
//! linear-algebra crate (its `direct-solver` feature). The direct solver is
//! on here, because the smooth models the app unwraps make big charts and
//! conjugate gradients slow down badly on those. Measured on 2026-10-26 (32
//! threads): 200,000 triangles of smooth quads took 7.5 s without it and
//! 0.46 s with it, a 327,680-triangle sphere 12 s and 0.62 s, a bumpy
//! 200,000-triangle quad model 4.2 s and 0.41 s.
//!
//! What it costs: about 35 more crates to build (`faer` and its matrix-kernel
//! crates, which use unsafe code and processor-specific instructions inside,
//! as every fast linear-algebra library does; their build scripts only
//! write generated kernels into the build folder), and `faer` picks its
//! kernels by processor at run time, so two different processors may give
//! UVs that differ in the last bits. The same model on the same machine
//! always gives the same UVs, to the bit, whatever the thread count: the
//! factorization runs on one thread per chart and charts are put together
//! in a fixed order.

use glam::{Vec2, Vec3};

mod stretch;

#[cfg(test)]
mod tests;

/// The most triangles [`unwrap`] takes. Charts on a mesh this size are
/// already too small to bake well into one picture, and the time and memory
/// grow with the count.
pub const MAX_TRIANGLES: usize = 2_000_000;

/// How the model is cut and packed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// The gap kept between any two charts, as a share of the side of the
    /// square: 0.004 is about 4 pixels on a 1024 map and 8 on a 2048 one. The
    /// edges of the square keep half of it. Kept at least this wide (a little
    /// wider when it is very small).
    pub padding: f32,
    /// How far, in degrees, a chart's faces may turn away from the chart's
    /// average direction. Smaller makes flatter charts with less stretch but
    /// more seams. Held between 1 and 89.
    pub max_angle_degrees: f32,
}

impl Default for Options {
    fn default() -> Self {
        Options { padding: 0.004, max_angle_degrees: 66.0 }
    }
}

/// What [`unwrap`] makes.
#[derive(Clone, Debug, PartialEq)]
pub struct Unwrapped {
    /// One UV per triangle corner, in the triangles' order: corner `c` of
    /// triangle `t` is `uvs[t * 3 + c]`. All lie in `[0, 1]`. A corner on a
    /// seam has a different UV in the triangles on either side of it.
    /// Triangles squashed to a line or a point (thinner than a millionth of
    /// the model's size) get no space of their own: their corners sit on a
    /// UV of a neighbouring face.
    pub uvs: Vec<Vec2>,
    /// How many charts the model was cut into: separate pieces in the
    /// square, each one connected through shared edges.
    pub charts: usize,
    /// How much the UVs stretch the surface, over the whole model, weighted
    /// by area: 1 means every triangle is laid flat at the same scale, with no
    /// stretching at all. This is Sander's L2 stretch, normalized for the
    /// overall scale. Spheres, tori and real sculpts measure 1.001 to 1.03;
    /// stretching starts to show in a baked map past about 1.1.
    pub stretch: f32,
}

/// Makes UVs for a model that has none.
///
/// * `positions` and `triangles` are the model: three corners per triangle,
///   each an index into `positions`.
/// * `polys`, for a model of polygon faces (a Quad Remesh), are those faces
///   as `(start, corners)`: face `f`'s corners are
///   `corners[start[f]..start[f + 1]]`. `triangles` must be the faces cut into
///   fans in order, `k - 2` triangles for a face of `k` corners. Each face's
///   triangles are kept in one chart, so a quad is never split by a seam.
/// * `groups`, one per triangle, are face groups: no chart crosses from one
///   group to another. Where one polygon's triangles disagree, the polygon
///   goes with the group of its first triangle, since a polygon is never
///   split.
///
/// Refuses, with a message to show the sculptor, an empty model, one over
/// [`MAX_TRIANGLES`], one whose triangles point at missing corners or whose
/// points are not numbers, data that does not fit the triangles, a model
/// with no surface to lay flat, and one that breaks into more charts than
/// fit the square at this padding. Open rims, edges shared by more than two
/// faces, faces wound the wrong way and squashed triangles are not refused:
/// the charts are cut there.
pub fn unwrap(
    positions: &[Vec3],
    triangles: &[u32],
    polys: Option<(&[u32], &[u32])>,
    groups: Option<&[u16]>,
    options: &Options,
) -> Result<Unwrapped, String> {
    let triangle_count = check_mesh(positions, triangles)?;
    let polygon_of = match polys {
        Some((start, corners)) => Some(polygon_ids(triangles, start, corners)?),
        None => None,
    };
    let materials = match groups {
        Some(groups) => Some(group_ids(groups, triangle_count, polygon_of.as_deref())?),
        None => None,
    };

    let points: Vec<[f32; 3]> = positions.iter().map(|p| p.to_array()).collect();
    let mesh = uvunwrap::MeshInput {
        face_materials: materials.as_deref(),
        polygons: polygon_of.as_deref(),
        ..uvunwrap::MeshInput::new(&points, triangles)
    };
    let (page, padding) = page_and_padding(options.padding);
    let angle = if options.max_angle_degrees.is_finite() {
        options.max_angle_degrees.clamp(1.0, 89.0)
    } else {
        Options::default().max_angle_degrees
    };
    let settings = uvunwrap::Options {
        max_chart_angle: angle,
        padding,
        density: uvunwrap::Density::Auto { page_size: page },
        ..uvunwrap::Options::default()
    };
    let atlas = generate(mesh, settings)?;
    if atlas.chart_count == 0 {
        return Err("The model has no surface to lay flat: every triangle is squashed \
                    to a line or a point."
            .to_string());
    }
    if atlas.page_count > 1 {
        return Err(format!(
            "The model breaks into too many pieces ({} charts) to fit one square \
             with this padding. Lower the padding, or smooth or remesh the model first.",
            atlas.chart_count
        ));
    }
    let uvs = corner_uvs(&atlas);
    let charts = count_pieces(&atlas.meshes[0], &uvs);
    let stretch = stretch::l2_stretch(positions, triangles, &uvs);
    Ok(Unwrapped { uvs, charts, stretch })
}

/// The UV of every triangle corner, in the square.
///
/// The packer measures UVs across in page widths and down in the height it
/// cropped the page to. Measuring both in page widths keeps the square
/// square: the charts keep their shapes and one scale.
fn corner_uvs(atlas: &uvunwrap::Atlas) -> Vec<Vec2> {
    let down = atlas.height as f32 / atlas.width as f32;
    let out = &atlas.meshes[0];
    out.indices
        .iter()
        .map(|&i| {
            let [u, v] = out.vertices[i as usize].uv;
            Vec2::new(u, v * down).clamp(Vec2::ZERO, Vec2::ONE)
        })
        .collect()
}

/// Charts beyond which packing by shape takes too long. Shape packing
/// searches the square for every chart: about 0.1 s for 340 charts, but 32 s
/// for 19,000 (a million-triangle print file of a feathered bird, measured
/// 2026-10-26), where packing in rows took 2.4 s.
const SHAPE_PACKING_CHARTS: usize = 2000;

/// Runs the unwrapper, packing by shape (denser, charts nested into each
/// other's hollows) unless the model was cut into more than
/// [`SHAPE_PACKING_CHARTS`] charts, when it is run again packing in rows.
///
/// The charts are counted as they are laid flat (the unwrapper reports each
/// one as it finishes), and the run is stopped as packing begins if there are
/// too many. The second run repeats the cutting and laying flat, which costs
/// a few seconds on such a model but keeps the choice down to the chart
/// count, so the same model always unwraps the same way.
fn generate(mesh: uvunwrap::MeshInput<'_>, settings: uvunwrap::Options) -> Result<uvunwrap::Atlas, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let laid_flat = AtomicUsize::new(0);
    let by_shape = uvunwrap::generate_with_progress(&[mesh], &settings, |p| match p.stage {
        uvunwrap::Stage::Parameterizing => {
            laid_flat.fetch_add(1, Ordering::Relaxed);
            true
        }
        uvunwrap::Stage::Packing => laid_flat.load(Ordering::Relaxed) <= SHAPE_PACKING_CHARTS,
        _ => true,
    });
    let atlas = match by_shape {
        Err(uvunwrap::Error::Cancelled) => {
            let in_rows = uvunwrap::Options { packing: uvunwrap::Packing::Fast, ..settings };
            uvunwrap::generate(&[mesh], &in_rows)
        }
        other => other,
    };
    atlas.map_err(|e| format!("The model could not be unwrapped: {e}."))
}

/// Checks the model itself and returns its triangle count.
fn check_mesh(positions: &[Vec3], triangles: &[u32]) -> Result<usize, String> {
    if triangles.is_empty() || positions.is_empty() {
        return Err("There is nothing to unwrap: the model has no triangles.".to_string());
    }
    if !triangles.len().is_multiple_of(3) {
        return Err(format!(
            "The model's triangle list is broken: it holds {} corners, which is not a \
             whole number of triangles.",
            triangles.len()
        ));
    }
    let count = triangles.len() / 3;
    if count > MAX_TRIANGLES {
        return Err(format!(
            "The model has {count} triangles; UVs can be made for at most {MAX_TRIANGLES}. \
             Bring it lower first, with Decimate or Quad Remesh."
        ));
    }
    if let Some(at) = triangles.iter().position(|&i| i as usize >= positions.len()) {
        return Err(format!(
            "Triangle {} uses point {}, but the model has only {} points.",
            at / 3,
            triangles[at],
            positions.len()
        ));
    }
    if let Some(at) = positions.iter().position(|p| !p.is_finite()) {
        return Err(format!("Point {at} of the model is not a number."));
    }
    Ok(count)
}

/// The polygon each triangle was cut from, checking that the triangles are
/// the polygons cut into fans in order. `start` may end with the total corner
/// count (the app's form) or leave it off.
fn polygon_ids(triangles: &[u32], start: &[u32], corners: &[u32]) -> Result<Vec<u32>, String> {
    let mismatch = |why: String| Err(format!("The model's polygon faces do not match its triangles: {why}."));
    let ends: Vec<u32> = if start.last() == Some(&(corners.len() as u32)) {
        start.to_vec()
    } else {
        start.iter().copied().chain([corners.len() as u32]).collect()
    };
    let count = triangles.len() / 3;
    let mut ids = Vec::with_capacity(count);
    for (f, span) in ends.windows(2).enumerate() {
        let (a, b) = (span[0] as usize, span[1] as usize);
        if a > b || b > corners.len() {
            return mismatch(format!("face {f} has its corners out of order"));
        }
        let face = &corners[a..b];
        if face.len() < 3 {
            return mismatch(format!("face {f} has fewer than three corners"));
        }
        for _ in 0..face.len() - 2 {
            let t = ids.len();
            if t == count {
                return mismatch(format!("the faces need more triangles than the {count} there are"));
            }
            if !triangles[t * 3..t * 3 + 3].iter().all(|v| face.contains(v)) {
                return mismatch(format!("triangle {t} is not a piece of face {f}"));
            }
            ids.push(f as u32);
        }
    }
    if ids.len() != count {
        return mismatch(format!("the faces make {} triangles but there are {count}", ids.len()));
    }
    Ok(ids)
}

/// How many separate pieces the UVs make in the square: triangles joined
/// wherever they share an edge with the same UVs at both ends. `out` is what
/// the unwrapper made, `uvs` the UVs of its triangles' corners. Triangles
/// with no UV area, the squashed ones the unwrapper parks on a neighbour's
/// UV, are not pieces of their own.
///
/// This is counted rather than taken from the unwrapper, which can leave a
/// chart it had to split in more than one piece: with polygon faces, a quad
/// or two can end up laid out on its own beside the rest. That is valid (it
/// overlaps nothing) but it is a separate piece in the square, with seams
/// round it, so it is counted as a chart.
///
/// Points are told apart by their UVs, not their numbers: a model whose
/// triangles do not share points (a print file is often like that) is still
/// welded by the unwrapper wherever positions match, and its pieces are as
/// big as on the same model with shared points.
fn count_pieces(out: &uvunwrap::OutputMesh, uvs: &[Vec2]) -> usize {
    let count = out.indices.len() / 3;
    let whole = |t: usize| {
        let [a, b, c] = [0, 1, 2].map(|k| uvs[t * 3 + k].as_dvec2());
        (b - a).perp_dot(c - a) > 0.0
    };
    // One number per distinct UV among the unwrapper's vertices. No two
    // charts share a UV (they are padded apart), so within a chart one UV is
    // one point of the surface.
    let mut number_of = std::collections::HashMap::with_capacity(out.vertices.len());
    let point: Vec<u32> = out
        .vertices
        .iter()
        .map(|v| {
            let next = number_of.len() as u32;
            *number_of.entry(v.uv.map(f32::to_bits)).or_insert(next)
        })
        .collect();
    // Every edge of every whole triangle, keyed by its two points, so
    // triangles sharing an edge sort next to each other.
    let mut edges: Vec<(u64, u32)> = Vec::with_capacity(count * 3);
    for t in (0..count).filter(|&t| whole(t)) {
        for k in 0..3 {
            let a = point[out.indices[t * 3 + k] as usize];
            let b = point[out.indices[t * 3 + (k + 1) % 3] as usize];
            edges.push((((a.min(b) as u64) << 32) | a.max(b) as u64, t as u32));
        }
    }
    edges.sort_unstable();
    let mut parent: Vec<u32> = (0..count as u32).collect();
    fn root(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    for pair in edges.windows(2) {
        if pair[0].0 == pair[1].0 {
            let (a, b) = (root(&mut parent, pair[0].1), root(&mut parent, pair[1].1));
            parent[a.max(b) as usize] = a.min(b);
        }
    }
    (0..count).filter(|&t| whole(t) && parent[t] == t as u32).count()
}

/// The face group of every triangle as the unwrapper's material ids, a
/// polygon's triangles all taking the group of its first.
fn group_ids(groups: &[u16], count: usize, polygon_of: Option<&[u32]>) -> Result<Vec<u32>, String> {
    if groups.len() != count {
        return Err(format!("There are face groups for {} triangles, but the model has {count}.", groups.len()));
    }
    let mut ids: Vec<u32> = groups.iter().map(|&g| g as u32).collect();
    if let Some(polygon_of) = polygon_of {
        // A polygon's triangles are consecutive, so its first is the first
        // triangle seen with its id.
        for t in 1..count {
            if polygon_of[t] == polygon_of[t - 1] {
                ids[t] = ids[t - 1];
            }
        }
    }
    Ok(ids)
}

/// The packer's page size and margin, in texels, for a gap between charts of
/// `share` of the square's side.
///
/// The packer keeps charts at least two margins apart, so the gap is
/// `2 * margin / page`. A page of about a thousand texels is fine enough to
/// nest charts closely; its exact size is chosen so a whole number of texels
/// gives at least the gap asked for.
fn page_and_padding(share: f32) -> (u32, u32) {
    const PAGE: f32 = 1024.0;
    if !(share > 0.0) {
        return (PAGE as u32, 0);
    }
    let share = share.min(0.1);
    let margin = (share * PAGE / 2.0).round().max(1.0);
    let page = (2.0 * margin / share).floor().clamp(256.0, 4096.0);
    (page as u32, margin as u32)
}
