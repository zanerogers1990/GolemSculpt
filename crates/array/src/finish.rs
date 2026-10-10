//! Making the array real, taking it off, and handing it back when the scene's
//! Z undoes it. Ported from `ArrayController.Finish.cs`.
//!
//! Make It Real is one commit either way: the object's own mesh becomes every
//! copy's geometry together ("One Object": separate shells, which a remesh
//! fuses), or every copy becomes an object of its own, exactly where it is
//! ("Separate Objects"). Small meshes are built on the spot; large ones on a
//! background job, with the live array on screen until the commit lands, so a
//! bake of a million triangles times twenty copies never freezes the window.
//!
//! The array does not live in the scene, so undoing the commit cannot hand it
//! back by itself. The tool keeps what it retired and watches for the scene
//! undoing it (the object's mesh is the old one again, or the objects it added
//! are gone) and puts the array back, and takes it away again on a redo.

use sculpt_geom::TriMesh;
use sculpt_toolkit::{Command, Ctx, Job, JobCtx, MeshData, MeshId, NewObject, ObjectId, Placement, Scene};

use crate::bake::{bake_array, check_size};
use crate::make;
use crate::state::ArrayState;
use crate::tool::{ArrayTool, pose_of};
use crate::types::{ArrayPlacement, Pose};

/// A build with at most this many vertices in all runs on the spot: the
/// commit is as quick as the build, and nothing has to wait for a later frame.
const INLINE_VERTICES: usize = 400_000;
/// Arrays remembered for Z to hand back.
const RETIRED_LIMIT: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    /// The copies become part of the selected object's own mesh.
    One,
    /// Every copy becomes an object of its own.
    Separate,
}

/// What a finished build holds, ready to commit.
pub enum Built {
    One(MeshData),
    Separate(Vec<Piece>),
}

/// One copy as an object of its own.
pub struct Piece {
    pub name: String,
    pub mesh: MeshData,
    pub placement: Placement,
}

/// A build in the background, and what it was started from.
pub struct Building {
    job: Job<Built>,
    start: Start,
}

impl Building {
    /// What the build says it is doing.
    pub fn progress(&self) -> String {
        self.job.progress()
    }
}

/// What the scene looked like when a build began.
#[derive(Clone)]
struct Start {
    object: ObjectId,
    mesh: MeshId,
    state: ArrayState,
    apply: Apply,
    copies: usize,
    vertices: usize,
}

/// An array that was made real, so the scene's Z can hand it back.
pub struct Retired {
    object: ObjectId,
    state: ArrayState,
    apply: Apply,
    /// The object's mesh before and after (One Object).
    old_mesh: MeshId,
    new_mesh: MeshId,
    /// The objects it added (Separate Objects).
    added: Vec<ObjectId>,
    /// Whether the array is retired right now, rather than handed back.
    applied: bool,
}

/// Everything a build needs, copied out of the scene: a job cannot borrow it.
struct Work {
    name: String,
    source: TriMesh,
    mask: Option<Vec<f32>>,
    placements: Vec<ArrayPlacement>,
    pose: Pose,
    apply: Apply,
}

impl ArrayTool {
    /// A straight row of copies from the selected object, along whichever
    /// world axis runs most nearly left to right on screen.
    pub(crate) fn make_line(&mut self, cx: &mut Ctx) {
        let object = self.target(cx);
        let size = cx.scene().bounds(object).size();
        let state = make::line(&self.arrays.state(object), size, cx.view.right);
        let count = state.settings.count;
        self.make(cx, "Line Array", state);
        self.say(cx, format!("{count} copies in a line. Drag the end point to aim and stretch it."), true);
    }

    /// A ring of copies round a centre beside the selected object, facing the
    /// view.
    pub(crate) fn make_circle(&mut self, cx: &mut Ctx) {
        let object = self.target(cx);
        let size = cx.scene().bounds(object).size();
        let state = make::circle(&self.arrays.state(object), size, cx.view.right, cx.view.forward);
        let count = state.settings.count;
        self.make(cx, "Circle Array", state);
        self.say(cx, format!("{count} copies in a circle. Drag the centre to change the radius."), true);
    }

    /// Gives the selected object this array as one step, with its far end
    /// picked so the handle to drag is the one to hand.
    fn make(&mut self, cx: &mut Ctx, label: &'static str, state: ArrayState) {
        let object = self.target(cx);
        self.cancel_gesture();
        self.begin_edit(object, label, None);
        self.arrays.set(object, state);
        self.commit_edit();
        self.armed = false;
        self.picked = Some(1);
    }

    /// Makes the selected object's array real. Small ones are done here; a
    /// large one is handed to a background job and lands in `frame`.
    pub(crate) fn apply(&mut self, cx: &mut Ctx, apply: Apply) {
        if self.building.is_some() {
            return;
        }
        let object = self.target(cx);
        let Some(state) = self.arrays.has(object).then(|| self.arrays.state(object)) else {
            self.say(cx, "The selected object has no array to apply.", false);
            return;
        };
        self.cancel_gesture();
        self.commit_edit();

        let scene = cx.scene();
        let (Some(info), Some(mesh)) = (scene.info(object), scene.mesh(object)) else { return };
        let copies = state.settings.count as usize;
        if let Err(e) = check_size(mesh.vertex_count(), copies) {
            self.say(cx, e, false);
            return;
        }
        let start = Start { object, mesh: info.mesh, state, apply, copies, vertices: mesh.vertex_count() };
        // The layout has to match what is on screen, so it is the stored one.
        let Some(live) = self.arrays.get(object) else { return };
        // Carried over only if there is one worth carrying: all clear is the same as none.
        let masked = mesh.mask.len() == mesh.vertex_count() && mesh.mask.iter().any(|&m| m != 0.0);
        let mask = masked.then(|| mesh.mask.to_vec());
        let work = Work {
            name: info.name.to_owned(),
            source: mesh.to_tri_mesh(),
            mask,
            placements: live.layout.clone(),
            pose: pose_of(info.placement),
            apply,
        };

        if start.vertices * copies <= INLINE_VERTICES {
            let built = build(work, None);
            self.land(cx, start, built);
        } else {
            let label = match apply {
                Apply::One => "Apply Array",
                Apply::Separate => "Apply Array (separate objects)",
            };
            let job = cx.spawn(label, move |job| build(work, Some(job)));
            self.building = Some(Building { job, start });
            self.say(cx, "Making the copies real. The array stays up until it is done.", true);
        }
    }

    /// Collects a finished background build and commits it.
    pub(crate) fn land_finished(&mut self, cx: &mut Ctx) {
        let Some(result) = self.building.as_mut().and_then(|b| b.job.poll()) else { return };
        if let Some(building) = self.building.take() {
            self.land(cx, building.start, result);
        }
    }

    /// The build is done: one commit, then the array is retired and the tool
    /// put away, as the Unity tool goes back to sculpting.
    fn land(&mut self, cx: &mut Ctx, start: Start, built: Result<Built, String>) {
        let built = match built {
            Ok(built) => built,
            Err(e) => {
                self.say(cx, e, false);
                return;
            }
        };
        // A build that took seconds may find the scene has moved on.
        let still_there = cx.scene().info(start.object).is_some_and(|info| info.mesh == start.mesh);
        if !still_there {
            self.say(cx, "The object changed while the copies were being made, so nothing was applied.", false);
            return;
        }

        let commands: Vec<Command> = match built {
            Built::One(mesh) => vec![Command::Replace { object: start.object, mesh }],
            Built::Separate(pieces) => {
                // Hidden if the object is, so a copy does not appear out of nowhere.
                let info = cx.scene().info(start.object);
                let visible = info.is_some_and(|info| info.visible);
                // A mirrored array stays mirrored: each new object gets the same live
                // mirror, so the other side shows them as it showed the copies.
                let mirror = info.and_then(|info| info.mirror);
                pieces
                    .into_iter()
                    .map(|piece| {
                        let object = NewObject::new(piece.name, piece.mesh)
                            .placed(piece.placement)
                            .unselected()
                            .mirrored(mirror);
                        Command::Add(if visible { object } else { object.hidden() })
                    })
                    .collect()
            }
        };
        let receipt = cx.commit("Apply Array", commands);
        if receipt.failed > 0 {
            self.say(cx, "Some of the copies could not be added.", false);
        }

        let new_mesh = cx.scene().info(start.object).map_or(start.mesh, |info| info.mesh);
        self.retire(Retired {
            object: start.object,
            state: start.state.clone(),
            apply: start.apply,
            old_mesh: start.mesh,
            new_mesh,
            added: receipt.added.clone(),
            applied: true,
        });
        let message = match start.apply {
            Apply::One => format!(
                "Applied: {} copies are now one object ({} vertices). Remesh to fuse them together. \
                 Undo (Z) gives the live array back.",
                start.copies,
                crate::bake::grouped(start.vertices * start.copies)
            ),
            Apply::Separate => {
                format!("Applied: {} separate objects. Undo (Z) gives the live array back.", start.copies)
            }
        };
        self.say(cx, message, true);
        cx.finish();
    }

    /// Takes an applied array out of the live ones, remembering it.
    fn retire(&mut self, retired: Retired) {
        self.arrays.set(retired.object, ArrayState::none());
        self.history.forget(retired.object);
        self.picked = None;
        self.armed = false;
        self.hover = Default::default();
        self.retired.retain(|r| r.object != retired.object);
        self.retired.push(retired);
        if self.retired.len() > RETIRED_LIMIT {
            self.retired.remove(0);
        }
    }

    /// Takes the whole array off the selected object, leaving the object as
    /// it is. One step of the tool's history.
    pub(crate) fn remove(&mut self, cx: &mut Ctx) {
        let object = self.target(cx);
        if !self.arrays.has(object) {
            return;
        }
        self.cancel_gesture();
        self.edit(cx, "Remove Array", false, |state| *state = ArrayState::none());
        self.picked = None;
        self.say(cx, "Array removed. Undo (Z) brings it back.", true);
    }

    /// Notices that the scene's undo took an apply back (or its redo did it
    /// again) and hands the array back (or takes it away again).
    pub(crate) fn notice_undone_applies(&mut self, cx: &mut Ctx) {
        for i in 0..self.retired.len() {
            let Some(now) = self.retired[i].is_applied(cx.scene()) else { continue };
            let retired = &mut self.retired[i];
            if now == retired.applied {
                continue;
            }
            retired.applied = now;
            let object = retired.object;
            if now {
                // Done again: whatever was edited meanwhile is what comes back next time.
                retired.state = self.arrays.state(object);
                self.arrays.set(object, ArrayState::none());
                self.history.forget(object);
                self.picked = None;
            } else if !self.arrays.has(object) {
                let state = retired.state.clone();
                self.arrays.set(object, state);
                self.say(cx, "Undone: the live array is back.", true);
            }
        }
    }
}

impl Retired {
    /// Whether the scene shows the array as applied; `None` when it cannot
    /// tell (the object is gone, or its mesh is neither of the two).
    fn is_applied(&self, scene: &dyn Scene) -> Option<bool> {
        match self.apply {
            Apply::One => {
                let mesh = scene.info(self.object)?.mesh;
                if mesh == self.new_mesh {
                    Some(true)
                } else if mesh == self.old_mesh {
                    Some(false)
                } else {
                    None
                }
            }
            Apply::Separate => self.added.first().map(|&id| scene.exists(id)),
        }
    }
}

/// The geometry: every copy joined into the object's own mesh, or a mesh for
/// each copy but the object itself. `job` is the background job it is running
/// on, which builds the app's sculpting structures here rather than on the
/// interface thread.
fn build(work: Work, job: Option<&JobCtx>) -> Result<Built, String> {
    let bake = |mesh: TriMesh, mask: Option<Vec<f32>>| match job {
        Some(job) => job.bake(mesh, mask),
        None => MeshData::Plain { mesh, mask },
    };
    let progress = |text: String| {
        if let Some(job) = job {
            job.progress(text);
        }
    };
    let cancelled = || job.is_some_and(JobCtx::cancelled);
    let copies = work.placements.len();

    match work.apply {
        Apply::One => {
            progress(format!("Joining {copies} copies"));
            let mesh = bake_array(&work.source, &work.placements, &work.pose)?;
            if cancelled() {
                return Err("Cancelled.".into());
            }
            // Every copy keeps the mask the object had.
            let mask = work.mask.map(|m| m.repeat(copies));
            progress("Preparing the mesh".to_owned());
            Ok(Built::One(bake(mesh, mask)))
        }
        Apply::Separate => {
            let mut pieces = Vec::with_capacity(copies.saturating_sub(1));
            for (c, placement) in work.placements.iter().enumerate().skip(1) {
                if cancelled() {
                    return Err("Cancelled.".into());
                }
                progress(format!("Copy {c} of {}", copies - 1));
                let world = placement.apply(&work.pose);
                pieces.push(Piece {
                    name: format!("{} {}", work.name, c + 1),
                    mesh: bake(work.source.clone(), work.mask.clone()),
                    placement: Placement { position: world.position, rotation: world.rotation, scale: world.scale },
                });
            }
            Ok(Built::Separate(pieces))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use glam::{Quat, Vec3};

    use super::*;
    use crate::layout::compute;
    use crate::testing::uv_sphere;
    use crate::types::{ArrayKind, ArraySettings};

    fn work(source: TriMesh, count: i32, apply: Apply) -> Work {
        let mut settings = ArraySettings::new(ArrayKind::Line);
        settings.count = count;
        let points = [Vec3::ZERO, Vec3::X * 3.0 * count as f32];
        Work {
            name: "Ball".to_owned(),
            mask: Some(vec![0.25; source.vertex_count()]),
            source,
            placements: compute(&settings, &points),
            pose: Pose::new(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
            apply,
        }
    }

    #[test]
    fn one_object_is_every_copy_in_one_mesh_with_the_mask_repeated() {
        let ball = uv_sphere(12, 24, 1.0);
        let (vertices, triangles) = (ball.vertex_count(), ball.triangle_count());
        let Ok(Built::One(MeshData::Plain { mesh, mask })) = build(work(ball, 6, Apply::One), None) else {
            panic!("a plain mesh")
        };
        assert_eq!((mesh.vertex_count(), mesh.triangle_count()), (6 * vertices, 6 * triangles));
        assert_eq!(mask.unwrap().len(), 6 * vertices);
    }

    #[test]
    fn separate_objects_skip_the_object_and_are_named_by_their_place_in_the_run() {
        let Ok(Built::Separate(pieces)) = build(work(uv_sphere(6, 12, 1.0), 4, Apply::Separate), None) else {
            panic!("pieces")
        };
        let names: Vec<&str> = pieces.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Ball 2", "Ball 3", "Ball 4"]);
        assert!(pieces.iter().all(|p| matches!(&p.mesh, MeshData::Plain { mask: Some(_), .. })));
        assert!(
            pieces[2].placement.position.distance(Vec3::X * 12.0) < 1e-4,
            "the last copy is at the end of the line"
        );
    }

    /// Timing, on whatever machine runs it:
    /// `cargo test -p sculpt-array --release --target-dir target/w/array -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_of_making_a_large_array_real() {
        // A 1.1 million triangle sphere, 12 copies: about 13 million triangles.
        let ball = uv_sphere(740, 740, 1.0);
        let triangles = ball.triangle_count();
        for apply in [Apply::One, Apply::Separate] {
            let work = work(ball.clone(), 12, apply);
            let start = Instant::now();
            let built = build(work, None).expect("builds");
            let took = start.elapsed();
            let pieces = match &built {
                Built::One(_) => 1,
                Built::Separate(pieces) => pieces.len(),
            };
            println!("{triangles} triangles x 12 copies, {apply:?}: {took:?} for {pieces} mesh(es)");
        }
    }
}
