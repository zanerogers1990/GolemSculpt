//! Turning live tubes into geometry: Merge and Keep Separate.
//!
//! Both voxelize the tubes' distance field (`field`), the same surface the
//! preview shows on a finer lattice, so the result is watertight and evenly
//! tessellated whatever was drawn.
//!
//! Each is one step on the scene's history, and undoing it hands the tubes
//! back live, so a merge that came out too thick is Z, a slider, and Enter
//! again.

use std::time::Instant;

use glam::Vec3;
use sculpt_geom::TriMesh;
use sculpt_toolkit::{Command, Ctx, Job, MeshData, MeshId, NewObject, ObjectId, Placement, Scene};

use crate::field::{MAX_VOXEL_SAMPLES, TubeField};
use crate::live::{LiveTube, TubeState};
use crate::merge::{self, MergePlan, Stage};
use crate::tool::TubeTool;

/// Blend setting 1 is a fillet whose size (the smooth union's `k`) is twice
/// the tube radius.
const BLEND_PER_RADIUS: f32 = 2.0;
/// Finishes whose undo is still watched for.
const MAX_RECORDS: usize = 32;

/// What a finish made, by the ids that show whether it has been undone.
enum Made {
    /// Keep Separate: the objects added, and the scene as it was before
    /// they were. An object that is gone was either undone or deleted, and
    /// only an undo puts the whole scene back as it was; so a tube object
    /// deleted after the scene has moved on does not bring its tube back.
    Objects { added: Vec<ObjectId>, before: Vec<(ObjectId, MeshId)> },
    /// Merge: each model changed, with the mesh it had before and after.
    Merged(Vec<(ObjectId, MeshId, MeshId)>),
}

/// A finish the user may yet undo, and the tubes to hand back if they do.
struct Record {
    made: Made,
    tubes: Vec<TubeState>,
    undone: bool,
}

impl Record {
    /// Whether the scene shows the finish as undone (`false`: as done).
    /// `None` when it shows neither, as while something else has changed
    /// the same object.
    fn is_undone(&self, scene: &dyn Scene) -> Option<bool> {
        match &self.made {
            Made::Objects { added, before } => {
                let here = added.iter().filter(|id| scene.exists(**id)).count();
                if here == 0 {
                    (listing(scene) == *before).then_some(true)
                } else if here == added.len() {
                    Some(false)
                } else {
                    None
                }
            }
            Made::Merged(models) => {
                let mesh = |object: &ObjectId| scene.info(*object).map(|info| info.mesh);
                if models.iter().all(|(o, before, _)| mesh(o) == Some(*before)) {
                    Some(true)
                } else if models.iter().all(|(o, _, after)| mesh(o) == Some(*after)) {
                    Some(false)
                } else {
                    None
                }
            }
        }
    }
}

/// Every object of the scene with the mesh it shows.
fn listing(scene: &dyn Scene) -> Vec<(ObjectId, MeshId)> {
    scene.objects().into_iter().filter_map(|id| scene.info(id).map(|info| (id, info.mesh))).collect()
}

/// One model of a merge in progress, as it was when the merge started.
struct Target {
    object: ObjectId,
    name: String,
    mesh: MeshId,
    checksum: u64,
    tubes: Vec<TubeState>,
    too_thin: bool,
}

/// What a merge job is given for one model.
struct Work {
    target: TriMesh,
    to_field: glam::Mat4,
    field: TubeField,
    plan: MergePlan,
    blend: f32,
    /// The coordinate planes the tubes are exactly symmetric across.
    mirror: [bool; 3],
}

pub(crate) struct Fused {
    /// One per model, in the order they were given.
    meshes: Vec<Result<MeshData, String>>,
    seconds: f64,
}

#[derive(Default)]
pub(crate) struct Finishing {
    job: Option<Job<Fused>>,
    /// What the running job was started from.
    targets: Vec<Target>,
    /// Tubes left out of the running merge because they have no anchor.
    loose: usize,
    records: Vec<Record>,
}

impl Finishing {
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    /// What the running merge says it is doing.
    pub fn progress(&self) -> Option<String> {
        self.job.as_ref().map(Job::progress)
    }

    fn record(&mut self, made: Made, tubes: Vec<TubeState>) {
        // A new step on the scene's history ends any redo, so a finish that
        // was undone can no longer come back.
        self.records.retain(|r| !r.undone);
        self.records.push(Record { made, tubes, undone: false });
        if self.records.len() > MAX_RECORDS {
            self.records.remove(0);
        }
    }
}

/// A cheap fingerprint of a mesh's vertices. Brush strokes do not change a
/// mesh's id, so this is how a merge notices the model was sculpted while it
/// ran.
fn checksum(positions: &[Vec3]) -> u64 {
    let stride = (positions.len() / 4096).max(1);
    positions.iter().step_by(stride).fold(positions.len() as u64, |hash, p| {
        let bits = (p.x.to_bits() as u64) ^ ((p.y.to_bits() as u64) << 21) ^ ((p.z.to_bits() as u64) << 42);
        hash.rotate_left(7) ^ bits
    })
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn stage_text(stage: Stage) -> &'static str {
    match stage {
        Stage::Tubes => "Building the tubes",
        Stage::Union => "Fusing into the model",
        Stage::Blend => "Smoothing the join",
    }
}

impl TubeTool {
    // ---- merge ---------------------------------------------------------------

    /// Fuses every live tube into the object it was drawn on: one voxel
    /// union per object, at that object's own density, with a smooth fillet
    /// where each tube meets it (see `merge`). It runs in the background;
    /// the tubes hold still until it lands. Tubes drawn in empty space are
    /// left live.
    pub fn merge_all(&mut self, cx: &mut Ctx) {
        self.start_merge(cx);
        self.flush(cx);
    }

    fn start_merge(&mut self, cx: &mut Ctx) {
        self.cancel_gesture();
        self.commit_edit();
        if self.finish.is_busy() {
            return;
        }
        if self.tubes.is_empty() {
            self.say("Draw a tube first.");
            return;
        }
        self.refresh(cx, false);

        // Tubes by the object they were drawn on, in the order met.
        let mut groups: Vec<(ObjectId, Vec<usize>)> = Vec::new();
        for (index, tube) in self.tubes.iter().enumerate() {
            let Some(anchor) = tube.state.anchor else { continue };
            match groups.iter_mut().find(|(object, _)| *object == anchor) {
                Some((_, tubes)) => tubes.push(index),
                None => groups.push((anchor, vec![index])),
            }
        }
        if groups.is_empty() {
            self.say("These tubes were drawn in empty space, so there is nothing to merge them into. Use Keep Separate.");
            return;
        }

        let mut targets = Vec::new();
        let mut work = Vec::new();
        let mut failure = None;
        for (object, tubes) in groups {
            let scene = cx.scene();
            let (Some(info), Some(mesh)) = (scene.info(object), scene.mesh(object)) else { continue };
            // Every tube in the group shares the anchor, so they share its
            // frame and symmetry.
            let first = &self.tubes[tubes[0]];
            let (to_field, mirror) = (first.frame.local_to_frame(), first.symmetry.exact_planes());
            let mut field = TubeField::new();
            let (mut blend, mut max_radius, mut thinnest) = (0.0f32, 0.0f32, f32::MAX);
            for &index in &tubes {
                let tube = &self.tubes[index];
                tube.add_to(&mut field);
                let settings = &tube.state.settings;
                blend = blend.max(settings.blend * settings.radius * BLEND_PER_RADIUS);
                max_radius = max_radius.max(settings.radius);
                thinnest = thinnest.min(settings.radius);
            }
            if field.is_empty() {
                failure = Some(format!("{}: the tubes have no length.", info.name));
                continue;
            }
            let plan = merge::plan(mesh.positions, mesh.indices, &to_field, &field, max_radius, thinnest);
            targets.push(Target {
                object,
                name: info.name.to_owned(),
                mesh: info.mesh,
                checksum: checksum(mesh.positions),
                tubes: tubes.iter().map(|&i| self.tubes[i].state.clone()).collect(),
                too_thin: plan.too_thin,
            });
            // The job cannot borrow from the scene: it gets a copy.
            work.push(Work { target: mesh.to_tri_mesh(), to_field, field, plan, blend, mirror });
        }
        if work.is_empty() {
            self.tell(failure.unwrap_or_else(|| "Nothing to merge.".into()), false);
            return;
        }

        self.finish.loose = self.tubes.len() - self.mergeable_count();
        self.finish.targets = targets;
        self.finish.job = Some(cx.spawn("Merge Tubes", move |job| {
            let started = Instant::now();
            let count = work.len();
            let mut meshes = Vec::with_capacity(count);
            for (n, work) in work.into_iter().enumerate() {
                let stage = |stage: Stage| {
                    let which = if count > 1 { format!(" ({} of {count})", n + 1) } else { String::new() };
                    job.progress(format!("{}{which}", stage_text(stage)));
                    !job.cancelled()
                };
                let merged =
                    merge::merge(&work.target, &work.to_field, &work.field, &work.plan, work.blend, work.mirror, &stage);
                if job.cancelled() {
                    return Err("Cancelled.".into());
                }
                // Baked here, so the commit on the interface thread is
                // instant however large the model.
                meshes.push(merged.map(|mesh| {
                    job.progress("Preparing the mesh");
                    job.bake(mesh, None)
                }));
            }
            Ok(Fused { meshes, seconds: started.elapsed().as_secs_f64() })
        }));
        self.say("Merging the tubes into the model...");
    }

    /// Stops the merge in progress. False if none is running.
    pub fn cancel_merge(&mut self) -> bool {
        match &self.finish.job {
            Some(job) => {
                job.cancel();
                true
            }
            None => false,
        }
    }

    /// Collects the merge once it has finished.
    pub(crate) fn poll_job(&mut self, cx: &mut Ctx) {
        let Some(result) = self.finish.job.as_mut().and_then(Job::poll) else { return };
        self.finish.job = None;
        self.land(cx, result);
    }

    /// Blocks until the merge in progress has finished and takes its result.
    /// For the demo and for tests, where there is no later frame.
    pub fn wait_for_merge(&mut self, cx: &mut Ctx) {
        if let Some(job) = self.finish.job.take() {
            let result = job.wait();
            self.land(cx, result);
            self.flush(cx);
        }
    }

    /// The merged models replace the old ones: one step on the scene's
    /// history for all of them, with the tubes riding on it. One Z un-merges
    /// and hands them back live; Shift+Z merges them again.
    fn land(&mut self, cx: &mut Ctx, result: Result<Fused, String>) {
        let targets = std::mem::take(&mut self.finish.targets);
        let loose = std::mem::take(&mut self.finish.loose);
        let fused = match result {
            Ok(fused) => fused,
            Err(error) => {
                let cancelled = error == "Cancelled.";
                self.tell(if cancelled { "Merge cancelled. The tubes are still live.".into() } else { error }, cancelled);
                return;
            }
        };

        let mut commands = Vec::new();
        let mut landed = Vec::new();
        let mut failure = None;
        for (target, mesh) in targets.into_iter().zip(fused.meshes) {
            let mesh = match mesh {
                Ok(mesh) => mesh,
                Err(error) => {
                    failure = Some(format!("{}: {error}", target.name));
                    continue;
                }
            };
            // The model must still be the one the merge was given.
            let unchanged = cx.scene().mesh(target.object).is_some_and(|m| m.id == target.mesh && checksum(m.positions) == target.checksum);
            if !unchanged {
                failure = Some(format!("{} changed while its tubes were merging. Merge again.", target.name));
                continue;
            }
            commands.push(Command::Replace { object: target.object, mesh });
            landed.push(target);
        }

        let mut merged = 0;
        let thin = landed.iter().any(|t| t.too_thin);
        if !commands.is_empty() {
            cx.commit("Merge Tubes", commands);
            let mut models = Vec::new();
            let mut tubes = Vec::new();
            for target in landed {
                if let Some(info) = cx.scene().info(target.object).filter(|info| info.mesh != target.mesh) {
                    models.push((target.object, target.mesh, info.mesh));
                    merged += target.tubes.len();
                    tubes.extend(target.tubes);
                }
            }
            self.remove_tubes(&tubes);
            if !models.is_empty() {
                self.finish.record(Made::Merged(models), tubes);
            }
        }

        let mut message = if merged > 0 {
            format!("Merged {} in {:.1}s. Z undoes it and gives the tubes back.", plural(merged, "tube", "tubes"), fused.seconds)
        } else {
            "Nothing merged.".to_owned()
        };
        if thin {
            message += " A tube was thinner than the model's detail, so it came out thicker than drawn: to keep it \
                        thin, undo, remesh the model finer, and merge again.";
        }
        if loose > 0 {
            message += &format!(" {} drawn in empty space left live: use Keep Separate.", plural(loose, "tube", "tubes"));
        }
        if let Some(failure) = &failure {
            message += &format!(" Failed: {failure}");
        }
        self.tell(message, merged > 0 && failure.is_none());
    }

    // ---- keep separate -------------------------------------------------------

    /// Makes every live tube its own sculptable object. A tube drawn on an
    /// object takes that object's position and rotation, so its mirror
    /// copies share its mirror planes and sculpting one arm still sculpts
    /// both.
    pub fn keep_separate(&mut self, cx: &mut Ctx) {
        self.keep(cx);
        self.flush(cx);
    }

    fn keep(&mut self, cx: &mut Ctx) {
        self.cancel_gesture();
        self.commit_edit();
        if self.finish.is_busy() {
            return;
        }
        if self.tubes.is_empty() {
            self.say("Draw a tube first.");
            return;
        }
        self.refresh(cx, false);

        let mut commands = Vec::new();
        let mut kept = Vec::new();
        for tube in &self.tubes {
            let field = tube.field();
            if field.is_empty() {
                continue;
            }
            let planes = tube.symmetry.exact_planes();
            let Ok(built) = field.voxelize(self.separate_cell(cx, tube, &field), planes, MAX_VOXEL_SAMPLES) else {
                continue;
            };
            let mut mesh = built.surface.into_mesh();
            // The object sits at the tube's frame (the anchor's position and
            // rotation at its own size), where the field was built; with no
            // anchor, at the tube's own centre.
            let placement = match tube.state.anchor {
                Some(_) => Placement { scale: Vec3::ONE, ..tube.frame.placement },
                None => {
                    let centre = mesh.bounds().center();
                    mesh.positions.iter_mut().for_each(|p| *p -= centre);
                    Placement::at(centre)
                }
            };
            // It grew on the anchor, so it is sculpted the way the anchor is:
            // it takes the anchor's symmetry settings, as in the Unity app.
            let mut object = NewObject::new("Tube", mesh).placed(placement);
            if let Some(anchor) = tube.state.anchor {
                object = object.with_symmetry_of(anchor);
            }
            commands.push(Command::Add(object));
            kept.push(tube.state.clone());
        }
        if commands.is_empty() {
            self.say("Nothing to keep: the tubes have no length.");
            return;
        }

        let before = listing(cx.scene());
        let receipt = cx.commit("Keep Tubes", commands);
        if receipt.added.is_empty() {
            self.tell("The tubes could not be made into objects.", false);
            return;
        }
        let count = receipt.added.len();
        self.remove_tubes(&kept);
        self.finish.record(Made::Objects { added: receipt.added, before }, kept);
        self.say(format!("Made {}. Z undoes it and gives the tubes back.", plural(count, "tube object", "tube objects")));
    }

    /// Voxel size for a kept tube: the anchor's own density where there is
    /// one.
    fn separate_cell(&self, cx: &Ctx, tube: &LiveTube, field: &TubeField) -> f32 {
        let scale = tube.frame.placement.scale.abs();
        let anchor = tube.state.anchor.and_then(|a| cx.scene().mesh(a));
        let cell = anchor.map(|mesh| merge::density_cell(mesh.positions, mesh.indices) * (scale.x + scale.y + scale.z) / 3.0);
        merge::separate_cell(tube.state.settings.radius, cell, field)
    }

    // ---- handing tubes back --------------------------------------------------

    /// Takes these tubes out of the live list: they are geometry now. The
    /// tool's own history describes tubes that are part of a mesh, and
    /// replaying it would bring them back on top of their finished selves,
    /// so it goes too; undoing the finish is what hands them back.
    fn remove_tubes(&mut self, tubes: &[TubeState]) {
        self.cancel_gesture();
        self.tubes.retain(|live| !tubes.iter().any(|t| t.id == live.state.id));
        self.active = None;
        self.selected_point = None;
        self.clear_history();
        self.touch();
    }

    /// Puts tubes back live, for an undone Merge or Keep Separate. True if
    /// any came back.
    fn restore_tubes(&mut self, cx: &Ctx, tubes: &[TubeState]) -> usize {
        self.cancel_gesture();
        self.commit_edit();
        let mut restored = 0;
        for state in tubes {
            let homeless = state.anchor.is_some_and(|a| !cx.scene().exists(a));
            if homeless || self.tubes.iter().any(|live| live.state.id == state.id) {
                continue;
            }
            self.tubes.push(LiveTube::from_state(state.clone()));
            restored += 1;
        }
        self.clear_history();
        self.touch();
        if restored > 0 {
            self.select_tube(self.tubes.len() - 1, None);
        }
        restored
    }

    /// Notices that a finish was undone (its objects are gone, or its model
    /// has its old mesh back) and hands the tubes back, bringing the tool up
    /// so they can be seen and edited; and that it was redone, and takes
    /// them away again.
    pub(crate) fn watch_finishes(&mut self, cx: &mut Ctx) {
        if self.finish.is_busy() {
            return;
        }
        for index in 0..self.finish.records.len() {
            let record = &self.finish.records[index];
            let Some(undone) = record.is_undone(cx.scene()) else { continue };
            if undone == record.undone {
                continue;
            }
            let tubes = record.tubes.clone();
            self.finish.records[index].undone = undone;
            if undone {
                let restored = self.restore_tubes(cx, &tubes);
                if restored > 0 {
                    if !cx.active {
                        cx.start();
                    }
                    self.say(format!("{} live again.", plural(restored, "tube is", "tubes are")));
                }
            } else {
                self.remove_tubes(&tubes);
            }
        }
    }
}
