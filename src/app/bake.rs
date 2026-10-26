//! UVs & Maps on the app's side: Unwrap UVs, and Bake Maps from a sculpt
//! onto the selected object.
//!
//! - **Unwrap UVs** gives the selected object UVs made for it (crates/uv),
//!   with seams along its face groups if asked. The model does not move: the
//!   step puts a copy with UVs in its place, the old one kept whole for Undo,
//!   as a change of level does. The copy is the mesh numbered afresh, so a
//!   model dynamic topology has worked on is unwrapped without the points its
//!   merges left behind.
//! - **Bake Maps** works out normal, displacement, ambient occlusion and
//!   colour maps (src/bake) from the object the card names, as Project Detail
//!   names its source (app/project.rs), onto the selected object, which needs
//!   UVs, and writes them as files in a folder the person picks. It runs in
//!   the background, as Remesh does (app/remesh_job.rs), on copies of both
//!   models as they were when it began, so the person can go on working; the
//!   job writes the files, and the status line says where when it lands.
//!
//! The settings are the scene's and not kept between runs (`Choices`), as
//! Project Detail's are.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::{Mat4, Vec2, Vec3};
use sculpt_toolkit::Job;

use super::Running;
use crate::mesh::Mesh;

/// How often a frame is drawn while a bake works, to land it.
const POLL: Duration = Duration::from_millis(100);
/// Rays per texel for the ambient occlusion map: enough to keep its grain
/// below what a texture shows, at a few seconds for a 2048 map.
const AO_RAYS: u32 = 32;
/// Texels of edge padding round every island, so mip-mapping never pulls in
/// the empty background along a seam.
const PADDING: u32 = 8;

/// The UVs & Maps card's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Choices {
    /// Unwrap UVs cuts along face group borders, so seams go where the
    /// sculptor drew them.
    pub seams_on_groups: bool,
    /// The object Bake Maps takes detail from; `None` (or one that has gone)
    /// for the one overlapping the selected object most (app/project.rs).
    pub from: Option<u64>,
    /// The maps' size in pixels, square.
    pub size: u32,
    /// How far from the selected object's surface Bake Maps looks for the
    /// other's, as a share of the object's size (as Project Detail's Reach).
    pub reach: f32,
    pub normal: bool,
    pub displacement: bool,
    pub ao: bool,
    pub colour: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Choices {
            seams_on_groups: true,
            from: None,
            size: 2048,
            reach: crate::project::DEFAULT_REACH,
            normal: true,
            displacement: true,
            ao: false,
            colour: true,
        }
    }
}

/// The map sizes the card offers.
pub const SIZES: [u32; 4] = [1024, 2048, 4096, 8192];

/// A copy of `mesh` with UVs made for it, with seams along its face groups
/// when `seams` is asked and it has any, and how many pieces they came to.
/// `mesh` itself is left as it is.
pub(crate) fn unwrapped(mesh: &Mesh, seams: bool) -> Result<(Mesh, usize, bool), String> {
    let mut copy = crate::scene::copy_of(mesh);
    let seams = seams && copy.has_groups();
    let groups = seams.then(|| copy.group_ids());
    let options = sculpt_uv::Options::default();
    let made = match by_polygons(&copy, groups, &options) {
        Some(made) => made?,
        None => {
            let made = sculpt_uv::unwrap(&copy.positions, &copy.indices, None, groups, &options)?;
            (made.uvs, made.charts)
        }
    };
    copy.set_uvs(made.0)?;
    Ok((copy, made.1, seams))
}

/// The unwrap of a mesh with polygons (a quad remesh), keeping every polygon
/// whole in one piece. The unwrapper wants each polygon's triangles as the
/// fan cut from its first corner, in the polygons' order; a built mesh keeps
/// its triangles in the order its spatial index put them, and a quad may be
/// cut along either diagonal. So the fan is cut afresh and unwrapped, and
/// since a polygon lies in one piece, each of its corners has one UV: a
/// mesh triangle takes its corners' UVs from the polygon that holds all
/// three of them. `None` for a mesh without polygons, or with a triangle no
/// polygon holds.
fn by_polygons(
    mesh: &Mesh,
    groups: Option<&[u16]>,
    options: &sculpt_uv::Options,
) -> Option<Result<(Vec<Vec2>, usize), String>> {
    use std::collections::HashMap;
    let polys = mesh.polys.as_ref()?;
    let faces: Vec<&[u32]> = polys.start.windows(2).map(|w| &polys.indices[w[0] as usize..w[1] as usize]).collect();
    // The polygons each vertex is a corner of.
    let mut of_vertex: Vec<Vec<u32>> = vec![Vec::new(); mesh.vertex_count()];
    for (f, corners) in faces.iter().enumerate() {
        for &v in corners.iter() {
            of_vertex.get_mut(v as usize)?.push(f as u32);
        }
    }
    // The polygon holding each mesh triangle.
    let holder: Option<Vec<u32>> = mesh
        .indices
        .chunks_exact(3)
        .map(|t| of_vertex[t[0] as usize].iter().copied().find(|&f| t.iter().all(|v| faces[f as usize].contains(v))))
        .collect();
    let holder = holder?;
    let mut fan = Vec::with_capacity(mesh.indices.len());
    let mut fan_face = Vec::with_capacity(mesh.indices.len() / 3);
    for (f, corners) in faces.iter().enumerate() {
        for k in 1..corners.len().saturating_sub(1) {
            fan.extend_from_slice(&[corners[0], corners[k], corners[k + 1]]);
            fan_face.push(f);
        }
    }
    // A polygon's group: the group of a mesh triangle it holds.
    let fan_groups: Option<Vec<u16>> = groups.map(|g| {
        let mut of_face = vec![0u16; faces.len()];
        for (t, &f) in holder.iter().enumerate() {
            of_face[f as usize] = g[t];
        }
        fan_face.iter().map(|&f| of_face[f]).collect()
    });
    let made = match sculpt_uv::unwrap(&mesh.positions, &fan, Some((&polys.start, &polys.indices)), fan_groups.as_deref(), options) {
        Ok(made) => made,
        Err(why) => return Some(Err(why)),
    };
    let mut corner_uv: HashMap<(u32, u32), Vec2> = HashMap::with_capacity(fan.len());
    for (i, &v) in fan.iter().enumerate() {
        corner_uv.insert((fan_face[i / 3] as u32, v), made.uvs[i]);
    }
    let mut uvs = Vec::with_capacity(mesh.indices.len());
    for (t, corners) in mesh.indices.chunks_exact(3).enumerate() {
        for &v in corners {
            uvs.push(*corner_uv.get(&(holder[t], v))?);
        }
    }
    Some(Ok((uvs, made.charts)))
}

/// The low model a bake works on, copied out of the scene when it began.
struct LowCopy {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
    uvs: Vec<Vec2>,
    to_world: Mat4,
}

/// The sculpt a bake takes detail from, copied out: built into a mesh of its
/// own on the job, with its paint as seen, one colour per vertex.
struct HighCopy {
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    colours: Option<Vec<[u8; 4]>>,
    to_world: Mat4,
}

/// What a finished bake hands back: the files written and the share of the
/// surface that found nothing within reach.
type Done = (Vec<PathBuf>, f32);

/// A bake at work, and what its status line will name.
pub(super) struct Pending {
    job: Job<Done>,
    names: (String, String),
    folder: PathBuf,
    started: Instant,
}

/// Bakes `low` from `high` and writes the maps into `folder` as `base`_normal.png
/// and the rest: the whole of a bake job's work, off the window's thread.
fn bake_and_save(
    low: LowCopy,
    high: HighCopy,
    settings: crate::bake::Settings,
    folder: &Path,
    base: &str,
    progress: &dyn Fn(f32) -> bool,
) -> Result<Done, String> {
    // The sculpt, built here so that the window's thread never waits for a big one.
    let (mesh, went) = Mesh::new_mapped(high.positions, high.indices, None);
    let colours = high.colours.map(|given| {
        let mut moved = vec![[255u8; 4]; mesh.vertex_count()];
        for (old, &new) in went.iter().enumerate() {
            if let (Some(slot), Some(c)) = (moved.get_mut(new as usize), given.get(old)) {
                *slot = *c;
            }
        }
        moved
    });
    let target = crate::bake::Target {
        positions: &low.positions,
        normals: &low.normals,
        indices: &low.indices,
        uvs: &low.uvs,
        to_world: low.to_world,
    };
    let source = crate::bake::Source { mesh: &mesh, to_world: high.to_world, colours: colours.as_deref() };
    let baked = crate::bake::bake(&target, &[source], &settings, progress)?;
    let files = crate::bake::save(&baked, folder, base)?;
    Ok((files, baked.missed))
}

impl Running {
    /// Why Bake Maps cannot start just now, if it cannot.
    fn bake_refusal(&self) -> Option<String> {
        if self.bake.is_some() {
            return Some("Bake Maps is still working. Wait for it, or press its Cancel.".into());
        }
        if let Some(job) = self.doc.tools.jobs.running().first() {
            return Some(format!("{} is still working. Wait for it, or press its Cancel, then bake.", job.label));
        }
        if !self.doc.mesh.has_uvs() {
            return Some("This object has no UVs to bake onto. Press Unwrap UVs first.".into());
        }
        let c = &self.doc.opts.bake;
        if !(c.normal || c.displacement || c.ao || c.colour) {
            return Some("Tick at least one map to bake.".into());
        }
        if crate::app::project::source(&self.doc.scene, &self.doc.mesh, c.from).is_none() {
            return Some(
                "Bake Maps needs the sculpt to bake from where this object is: no other object overlaps it.".into(),
            );
        }
        None
    }

    /// The Bake Maps button: asks for the folder to write into, then bakes.
    pub(super) fn bake_maps_asking(&mut self) {
        if let Some(why) = self.bake_refusal() {
            return self.status(why, false);
        }
        let Some(folder) = rfd::FileDialog::new().set_title("Bake Maps Into").pick_folder() else { return };
        self.bake_maps(folder);
    }

    /// Starts baking the card's maps from its source onto the selected object,
    /// into `folder`, in the background.
    pub(super) fn bake_maps(&mut self, folder: PathBuf) {
        if let Some(why) = self.bake_refusal() {
            return self.status(why, false);
        }
        let doc = &self.doc;
        let c = doc.opts.bake;
        let Some(from) = crate::app::project::source(&doc.scene, &doc.mesh, c.from) else { return };
        let Some(high_mesh) = doc.scene.mesh(from.mesh, &doc.mesh) else { return };
        let active = doc.scene.active();
        let low = LowCopy {
            positions: doc.mesh.positions.clone(),
            normals: doc.mesh.normals.clone(),
            indices: doc.mesh.indices.clone(),
            uvs: doc.mesh.uvs().to_vec(),
            to_world: active.matrix(),
        };
        let colours = (c.colour && high_mesh.has_paint())
            .then(|| crate::io::painted::Painted::of(high_mesh, &self.export_paint()))
            .flatten()
            .map(|p| p.seen.iter().map(|&[r, g, b]| [r, g, b, 255]).collect());
        let high =
            HighCopy { positions: high_mesh.positions.clone(), indices: high_mesh.indices.clone(), colours, to_world: from.matrix() };
        let settings = crate::bake::Settings {
            size: c.size,
            reach: crate::app::project::reach(&doc.scene, &doc.mesh, c.reach),
            maps: crate::bake::Maps { normal: c.normal, displacement: c.displacement, ao: c.ao, colour: c.colour },
            ao_rays: AO_RAYS,
            padding: PADDING,
        };
        let names = (active.name.clone(), from.name.clone());
        let base = names.0.clone();
        let into = folder.clone();
        let job = doc.tools.jobs.spawn("Bake Maps", move |cx| {
            let progress = |done: f32| {
                cx.progress(format!("{:.0}%", done * 100.0));
                !cx.cancelled()
            };
            bake_and_save(low, high, settings, &into, &base, &progress)
        });
        self.bake = Some(Pending { job, names, folder, started: Instant::now() });
        self.redraw = true;
    }

    /// Once a frame: lands a bake that has finished.
    pub(super) fn bake_frame(&mut self) {
        let Some(pending) = &mut self.bake else { return };
        let Some(result) = pending.job.poll() else {
            let at = Instant::now() + POLL;
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            return;
        };
        if let Some(Pending { names, folder, started, .. }) = self.bake.take() {
            self.bake_land(names, &folder, started, result);
        }
    }

    /// For a run with no frames to come: waits for the bake at work, and lands it.
    pub(super) fn bake_wait(&mut self) {
        let Some(Pending { job, names, folder, started }) = self.bake.take() else { return };
        let result = job.wait();
        self.bake_land(names, &folder, started, result);
    }

    /// Says how a finished bake of `low` from `high` into `folder` went.
    fn bake_land(&mut self, (low, high): (String, String), folder: &Path, started: Instant, result: Result<Done, String>) {
        self.redraw = true;
        match result {
            Ok((files, missed)) => {
                // A map can be more than one file (displacement: an EXR, a PNG and a note of its range).
                let mut stems: Vec<_> = files.iter().filter_map(|f| f.file_stem()).collect();
                stems.sort_unstable();
                stems.dedup();
                let maps = stems.len();
                let missed = if missed > 0.05 {
                    format!(
                        " {:.0}% of the surface found nothing of \"{high}\" within reach and is left flat: raise \
                         Reach if that is too much.",
                        missed * 100.0
                    )
                } else {
                    String::new()
                };
                self.toast("Bake Maps");
                self.status(
                    format!(
                        "Baked {maps} maps of \"{low}\" from \"{high}\" into {} ({:.1} s).{missed}",
                        folder.display(),
                        started.elapsed().as_secs_f64()
                    ),
                    true,
                );
            }
            Err(why) if why == crate::bake::CANCELLED => self.status(why, true),
            Err(why) => self.status(why, false),
        }
    }

    /// The Unwrap UVs button: UVs for the selected object, as one undo step.
    pub(super) fn unwrap_uvs(&mut self) {
        let started = Instant::now();
        let (copy, charts, seams) = match unwrapped(&self.doc.mesh, self.doc.opts.bake.seams_on_groups) {
            Ok(made) => made,
            Err(why) => return self.status(why, false),
        };
        let name = self.doc.scene.active().name.clone();
        let along = if seams { ", seams along its face groups" } else { "" };
        let text = format!(
            "Unwrapped \"{name}\": {charts} pieces in the square{along} ({:.0} ms). Export it as OBJ or FBX to take \
             the UVs with it, or bake maps onto it.",
            started.elapsed().as_secs_f64() * 1000.0
        );
        self.doc.history.name_next(Some("Unwrap UVs".to_owned()));
        self.replace_mesh(copy, true);
        self.doc.history.name_next(None);
        self.toast("Unwrap UVs");
        self.status(text, true);
    }
}

mod play;
#[cfg(test)]
mod tests;
