//! Map baking: the detail of a high-resolution sculpt written into pictures
//! laid over a low-resolution model's UVs. These are the normal,
//! displacement, ambient occlusion and colour maps that game engines and film
//! renderers put back on the light model so that it looks like the sculpt.
//!
//! **Texel by texel.** Every triangle of the low model (the target) is drawn
//! into the picture at its corner UVs (`raster.rs`). Each texel whose centre
//! falls inside a triangle stands for one point of the low surface; the
//! baker takes that point and the low model's smooth normal there, both in
//! the scene, and looks along the normal, outward and inward, for the sculpt
//! (the sources). It uses the same rule as Project Detail (`project.rs`): the
//! nearer hit within Reach whose surface faces the same way, or failing that
//! the nearest point within Reach that faces that way (`cast.rs`). Along the
//! normal, because that is the direction a renderer will push the low
//! surface or bend its light, so what the texel holds is what belongs over
//! it. With several sources the nearest of their answers wins. What the
//! sculpt is like at the point found is what the texel holds:
//!
//! - **normal**: the sculpt's smooth normal, written in the low model's
//!   MikkTSpace tangent frame, the one Blender, Unity and Unreal read normal
//!   maps in (`frame.rs`), green up (OpenGL);
//! - **displacement**: how far the sculpt stands out from (or sinks into) the
//!   low surface along its normal, in scene units, kept as floating point;
//! - **ambient occlusion**: how much open sky the sculpt sees there
//!   (`ao.rs`);
//! - **colour**: the paint, from the corners of the sculpt's triangle there.
//!
//! A texel that finds nothing is counted as missed (the share comes back, so
//! the panel can say Reach is too small) and is left to the padding.
//!
//! **Padding** (`pad.rs`): the texels just outside each island are filled
//! from their neighbours inside it, a few texels deep, because filtering and
//! mip maps read a little past an island's edge, and a seam would show the
//! empty picture behind it.
//!
//! **The work** is shared out in bands of rows, one thread to a band
//! (`rayon`), each knowing which triangles reach it, so no two threads write
//! the same texel and the result is the same however the work is split.
//! Nothing depends on chance: ambient occlusion uses a fixed set of
//! directions turned by an amount drawn from the texel's own place. The
//! caller's progress function is called from the caller's own thread, a few
//! times a second, while the bands are worked through on others; when it
//! says stop, every band stops at the end of the row it is on.
//!
//! **Space.** Both kinds of object are taken where they stand in the scene,
//! each with its own position, rotation and scale, and the low model as an
//! export would write it there: a reflected placement (a live mirror copy)
//! with its faces turned back the right way out, so its tangent frame is the
//! one the exported model will get. Nothing here changes a mesh or touches
//! the scene; the app hands in plain arrays and gets pictures back, and
//! `save` writes them.

// Until the Bake panel calls it, the app itself uses none of this.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use glam::{Mat3, Mat4, Vec2, Vec3, Vec4};
use rayon::prelude::*;

mod ao;
mod cast;
mod frame;
mod pad;
mod raster;
mod save;

use cast::Ready;
use frame::Frame;
use raster::Raster;

/// The low model the maps are for, as plain arrays.
pub struct Target<'a> {
    pub positions: &'a [Vec3],
    /// Smooth vertex normals, one per position.
    pub normals: &'a [Vec3],
    pub indices: &'a [u32],
    /// One per triangle corner (`uvs[t * 3 + c]`), V up as in OBJ and FBX.
    pub uvs: &'a [Vec2],
    pub to_world: Mat4,
}

/// A sculpt to bake from, and where its object stands in the scene.
pub struct Source<'a> {
    pub mesh: &'a crate::mesh::Mesh,
    pub to_world: Mat4,
    /// Per vertex of `mesh`, the paint as seen; `None` for an unpainted sculpt.
    pub colours: Option<&'a [[u8; 4]]>,
}

/// How to bake.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// The width and height of every picture, from 512 to 8192 texels.
    pub size: u32,
    /// How far from the low surface the sculpt may be, in scene units, as
    /// with Project Detail.
    pub reach: f32,
    pub maps: Maps,
    /// Rays a texel sends out for ambient occlusion: more is smoother and slower.
    pub ao_rays: u32,
    /// How many texels deep each island's edge is carried outward.
    pub padding: u32,
}

/// Which pictures to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Maps {
    pub normal: bool,
    pub displacement: bool,
    pub ao: bool,
    pub colour: bool,
}

/// The pictures baked, one for each map asked for.
#[derive(Clone)]
pub struct Baked {
    /// Tangent space, MikkTSpace, OpenGL Y+ (as Blender and Unity read it).
    pub normal: Option<image::RgbImage>,
    /// The signed distance from the low surface to the sculpt along the low
    /// normal, in scene units; with the least and greatest found.
    pub displacement: Option<(Heights, f32, f32)>,
    /// White is open sky, black is shut in.
    pub ao: Option<image::GrayImage>,
    pub colour: Option<image::RgbaImage>,
    /// The share of the covered texels whose look along the normal found nothing.
    pub missed: f32,
}

/// A picture of one floating-point number a texel: the displacement map.
pub type Heights = image::ImageBuffer<image::Luma<f32>, Vec<f32>>;

#[cfg_attr(not(test), allow(unused_imports))] // until the Bake panel calls it
pub use save::save;

/// The error `bake` gives when its progress function asked it to stop.
pub const CANCELLED: &str = "The bake was stopped.";

/// The smallest and largest picture sizes offered.
pub const SIZES: std::ops::RangeInclusive<u32> = 512..=8192;

/// How far the sky is looked for when baking ambient occlusion, as a share
/// of the largest side of the box around the sources: far enough that the
/// gap under a chin or between fingers shades, near enough that the far side
/// of the model does not darken all of it.
const AO_DISTANCE: f32 = 0.25;

/// How far an ambient occlusion ray starts off the sculpt's surface, as a
/// share of the same box: enough that it cannot hit the triangle it leaves.
const AO_LIFT: f32 = 1e-5;

/// How often the progress function is called while the work goes on.
const TICK: Duration = Duration::from_millis(150);

/// What an unpainted sculpt gives the colour map when others are painted:
/// white, which leaves a material's own colour as it is.
const UNPAINTED: [u8; 4] = [255, 255, 255, 255];

/// A flat normal-map texel: straight out of the surface.
const FLAT: [u8; 3] = [128, 128, 255];

/// Bakes `maps` from `sources` onto `target`. `progress` is told how far the
/// work has got (0 to 1) a few times a second, from the calling thread, and
/// stops the bake by answering false; the bake then gives [`CANCELLED`].
pub fn bake(
    target: &Target,
    sources: &[Source],
    settings: &Settings,
    progress: &dyn Fn(f32) -> bool,
) -> Result<Baked, String> {
    check(target, sources, settings)?;
    let size = settings.size;
    let painted = sources.iter().any(|s| s.colours.is_some());
    let wanted = Maps { colour: settings.maps.colour && painted, ..settings.maps };
    if wanted == (Maps { normal: false, displacement: false, ao: false, colour: false }) {
        return Err(if settings.maps.colour {
            "There is no paint on the sculpt to bake a colour map from.".into()
        } else {
            "Pick at least one map to bake.".into()
        });
    }
    let ticker = Ticker::default();
    if !progress(0.0) {
        return Err(CANCELLED.into());
    }

    let low = Low::new(target);
    let raster = Raster::new(&low.uvs, size);
    let ready: Vec<Ready> = sources.iter().map(Ready::new).collect();
    let covered: u64 = (0..raster.band_count())
        .into_par_iter()
        .map(|band| {
            let mut n = 0u64;
            raster.band(band, |_, _, _, _| n += 1, || false);
            n
        })
        .sum();
    if covered == 0 {
        return Err("The model's UVs cover none of the picture: they are all outside the square from 0 to 1, \
                    or have no area."
            .into());
    }

    let texels = size as usize * size as usize;
    let mut filled = vec![0u8; texels];
    let mut normal = if wanted.normal { FLAT.repeat(texels) } else { Vec::new() };
    let mut displacement = if wanted.displacement { vec![0.0f32; texels] } else { Vec::new() };
    let mut ao = if wanted.ao { vec![255u8; texels] } else { Vec::new() };
    let mut colour = if wanted.colour { vec![0u8; texels * 4] } else { Vec::new() };

    let span = ready
        .iter()
        .map(Ready::world_bounds)
        .fold(None, |all: Option<(Vec3, Vec3)>, (lo, hi)| Some(all.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi)))));
    let side = span.map_or(1.0, |(lo, hi)| (hi - lo).max_element()).max(1e-6);
    let job = Job {
        low: &low,
        raster: &raster,
        ready: &ready,
        wanted,
        reach: settings.reach,
        size,
        sky: wanted.ao.then(|| ao::Sky::new(settings.ao_rays.clamp(1, 4096), side * AO_DISTANCE, side * AO_LIFT)),
    };

    let rows = raster::BAND as usize * size as usize;
    let mut bands = Vec::with_capacity(raster.band_count());
    {
        let mut normal = normal.chunks_mut(rows * 3);
        let mut displacement = displacement.chunks_mut(rows);
        let mut ao = ao.chunks_mut(rows);
        let mut colour = colour.chunks_mut(rows * 4);
        for (b, filled) in filled.chunks_mut(rows).enumerate() {
            bands.push(BandOut {
                first: b * rows,
                filled,
                normal: normal.next().unwrap_or_default(),
                displacement: displacement.next().unwrap_or_default(),
                ao: ao.next().unwrap_or_default(),
                colour: colour.next().unwrap_or_default(),
            });
        }
    }
    let work = || {
        bands
            .into_par_iter()
            .enumerate()
            .map(|(b, out)| job.band(b, out, &ticker))
            .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    };
    let (found, missed) = with_progress(progress, (0.0, 0.95), covered, &ticker, work)?;
    if found == 0 {
        return Err("Nothing on the sculpt was found from the model: the two objects are not in the same place, \
                    or Reach is too small."
            .into());
    }

    let range = wanted.displacement.then(|| {
        let mut range = (f32::INFINITY, f32::NEG_INFINITY);
        for (d, f) in displacement.iter().zip(&filled) {
            if *f != 0 {
                range = (range.0.min(*d), range.1.max(*d));
            }
        }
        range
    });

    let times = settings.padding.clamp(0, 256);
    for step in 0..times {
        if !progress(0.95 + 0.05 * step as f32 / times as f32) {
            return Err(CANCELLED.into());
        }
        let frontier = pad::frontier(&filled, size);
        if frontier.is_empty() {
            break;
        }
        pad::spread(&mut normal, 3, size, &frontier);
        pad::spread(&mut displacement, 1, size, &frontier);
        pad::spread(&mut ao, 1, size, &frontier);
        pad::spread(&mut colour, 4, size, &frontier);
        pad::mark(&mut filled, &frontier);
    }
    if !progress(1.0) {
        return Err(CANCELLED.into());
    }

    let picture = "a picture is its size squared";
    Ok(Baked {
        normal: wanted.normal.then(|| image::RgbImage::from_raw(size, size, normal).expect(picture)),
        displacement: range
            .map(|(lo, hi)| (image::ImageBuffer::from_raw(size, size, displacement).expect(picture), lo, hi)),
        ao: wanted.ao.then(|| image::GrayImage::from_raw(size, size, ao).expect(picture)),
        colour: wanted.colour.then(|| image::RgbaImage::from_raw(size, size, colour).expect(picture)),
        missed: missed as f32 / (found + missed) as f32,
    })
}

/// Says in plain words what is wrong with what was handed in, if anything.
fn check(target: &Target, sources: &[Source], settings: &Settings) -> Result<(), String> {
    if target.uvs.is_empty() {
        return Err("The model has no UVs to bake onto: unwrap it first.".into());
    }
    if target.indices.is_empty() || !target.indices.len().is_multiple_of(3) {
        return Err("The model has no faces to bake onto.".into());
    }
    if target.uvs.len() != target.indices.len() {
        return Err(format!(
            "The model's UVs do not match its faces: {} UVs for {} triangle corners.",
            target.uvs.len(),
            target.indices.len()
        ));
    }
    if target.normals.len() != target.positions.len() {
        return Err("The model's normals do not match its points.".into());
    }
    let points = target.positions.len();
    if target.indices.iter().any(|&i| i as usize >= points) {
        return Err("The model has a face whose corner is not one of its points.".into());
    }
    if !SIZES.contains(&settings.size) {
        return Err(format!(
            "The picture size must be from {} to {} texels; {} was asked for.",
            SIZES.start(),
            SIZES.end(),
            settings.size
        ));
    }
    if !(settings.reach.is_finite() && settings.reach > 0.0) {
        return Err("Reach must be more than nothing.".into());
    }
    if sources.is_empty() || sources.iter().all(|s| s.mesh.triangle_count() == 0) {
        return Err("There is no sculpt to bake from.".into());
    }
    for source in sources {
        if source.colours.is_some_and(|c| c.len() != source.mesh.vertex_count()) {
            return Err("The sculpt's paint does not match its points.".into());
        }
    }
    Ok(())
}

/// The matrix that carries normals the way `m` carries points.
fn normal_matrix(m: Mat4) -> Mat3 {
    let linear = Mat3::from_mat4(m);
    if linear.determinant().abs() > 1e-20 { linear.inverse().transpose() } else { linear }
}

/// The target as the baker uses it: in the scene, each triangle corner with
/// its own position, normal, UV and tangent (`t * 3 + c`), faces wound the
/// right way out.
struct Low {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<Vec2>,
    /// MikkTSpace: the tangent, and in `w` the sign the bitangent takes.
    tangents: Vec<Vec4>,
}

impl Low {
    fn new(target: &Target) -> Low {
        let m = target.to_world;
        let turn = normal_matrix(m);
        // A reflection turns every face inside out; corners 1 and 2 change
        // places to turn it back, as the exporters do.
        let order = if Mat3::from_mat4(m).determinant() < 0.0 { [0, 2, 1] } else { [0, 1, 2] };
        let corners = target.indices.len();
        let (mut positions, mut normals, mut uvs) =
            (Vec::with_capacity(corners), Vec::with_capacity(corners), Vec::with_capacity(corners));
        for t in 0..corners / 3 {
            for c in order {
                let at = t * 3 + c;
                let v = target.indices[at] as usize;
                positions.push(m.transform_point3(target.positions[v]));
                normals.push((turn * target.normals[v]).normalize_or_zero());
                uvs.push(target.uvs[at]);
            }
        }
        let tangents = frame::tangents(&positions, &normals, &uvs);
        Low { positions, normals, uvs, tangents }
    }
}

/// Shared between the calling thread and the workers: how many texels are
/// done, and whether to stop.
#[derive(Default)]
struct Ticker {
    done: AtomicU64,
    stop: AtomicBool,
}

impl Ticker {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Runs `work` on other threads while this one tells `progress` how far it
/// has got (as `ticker` counts towards `total`, mapped onto `span`), and
/// passes a stop on to the workers.
fn with_progress<T: Send>(
    progress: &dyn Fn(f32) -> bool,
    span: (f32, f32),
    total: u64,
    ticker: &Ticker,
    work: impl FnOnce() -> T + Send,
) -> Result<T, String> {
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let _ = tx.send(work());
        });
        loop {
            match rx.recv_timeout(TICK) {
                Ok(out) => return if ticker.stopped() { Err(CANCELLED.to_string()) } else { Ok(out) },
                Err(RecvTimeoutError::Timeout) => {
                    let share = ticker.done.load(Ordering::Relaxed) as f32 / total.max(1) as f32;
                    if !progress(span.0 + (span.1 - span.0) * share.min(1.0)) {
                        ticker.stop.store(true, Ordering::Relaxed);
                    }
                }
                // The work panicked; leaving the scope passes the panic on.
                Err(RecvTimeoutError::Disconnected) => return Err("The bake failed.".to_string()),
            }
        }
    })
}

/// One band of rows of every picture being baked, for one thread to fill.
/// A map not asked for has empty slices.
struct BandOut<'a> {
    /// Where the band's first texel is in the whole picture.
    first: usize,
    filled: &'a mut [u8],
    normal: &'a mut [u8],
    displacement: &'a mut [f32],
    ao: &'a mut [u8],
    colour: &'a mut [u8],
}

/// Everything the workers read.
struct Job<'a> {
    low: &'a Low,
    raster: &'a Raster,
    ready: &'a [Ready<'a>],
    wanted: Maps,
    reach: f32,
    size: u32,
    sky: Option<ao::Sky>,
}

/// What one texel holds once baked.
struct Texel {
    normal: [u8; 3],
    displacement: f32,
    ao: u8,
    colour: [u8; 4],
}

impl Job<'_> {
    /// Bakes every texel of band `band` into `out`; how many found the sculpt
    /// and how many did not.
    fn band(&self, band: usize, out: BandOut, ticker: &Ticker) -> (u64, u64) {
        let (mut found, mut missed, mut unsaid) = (0u64, 0u64, 0u64);
        self.raster.band(
            band,
            |tri, x, y, weights| {
                let i = y as usize * self.size as usize + x as usize - out.first;
                match self.texel(tri, x, y, weights) {
                    Some(texel) => {
                        found += 1;
                        out.filled[i] = 1;
                        if self.wanted.normal {
                            out.normal[i * 3..i * 3 + 3].copy_from_slice(&texel.normal);
                        }
                        if self.wanted.displacement {
                            out.displacement[i] = texel.displacement;
                        }
                        if self.wanted.ao {
                            out.ao[i] = texel.ao;
                        }
                        if self.wanted.colour {
                            out.colour[i * 4..i * 4 + 4].copy_from_slice(&texel.colour);
                        }
                    }
                    None => missed += 1,
                }
                unsaid += 1;
                if unsaid == 64 {
                    ticker.done.fetch_add(unsaid, Ordering::Relaxed);
                    unsaid = 0;
                }
            },
            || ticker.stopped(),
        );
        ticker.done.fetch_add(unsaid, Ordering::Relaxed);
        (found, missed)
    }

    /// Bakes the texel at `x, y`, which is at `weights` in target triangle
    /// `tri`; `None` when the sculpt is not found from there.
    fn texel(&self, tri: u32, x: u32, y: u32, weights: [f32; 3]) -> Option<Texel> {
        let c = tri as usize * 3;
        let low = self.low;
        let mix = |v: &[Vec3]| v[c] * weights[0] + v[c + 1] * weights[1] + v[c + 2] * weights[2];
        let point = mix(&low.positions);
        let normal = mix(&low.normals).normalize_or_zero();
        if normal == Vec3::ZERO {
            return None;
        }
        let hit = cast::find(self.ready, point, normal, self.reach)?;
        let mut texel = Texel { normal: FLAT, displacement: 0.0, ao: 255, colour: UNPAINTED };
        if self.wanted.normal {
            let corners = |v: &[Vec3]| [v[c], v[c + 1], v[c + 2]];
            let tangents = [low.tangents[c], low.tangents[c + 1], low.tangents[c + 2]];
            let frame = Frame::at(corners(&low.normals), tangents, weights);
            texel.normal = frame::encode(frame.to_tangent(hit.normal));
        }
        if self.wanted.displacement {
            texel.displacement = (hit.pos - point).dot(normal);
        }
        if let Some(sky) = &self.sky {
            let open = sky.openness(self.ready, hit.pos, hit.normal, hit.face, x, y);
            texel.ao = (open * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        if self.wanted.colour {
            texel.colour = self.ready[hit.source].colour(hit.tri, hit.bary).unwrap_or(UNPAINTED);
        }
        Some(texel)
    }
}

#[cfg(test)]
mod tests;
