//! New Scene: a fresh scene in place of the one that is open, without
//! restarting the app.
//!
//! A new scene is an Open of a document that is in no file. That document
//! (`fresh_document`) holds what the app starts with: the starting sphere, at
//! the Starting Detail, seen from the starting view. It goes in by the road
//! every opened file takes (`Running::put_in_place`), so what an Open already
//! gets right is right here too:
//!
//! - It is one undo step. Undo brings the scene from before back with its
//!   own file and whether it was saved (`file_undo.rs`); Redo gives the new
//!   scene back.
//! - The new scene has no file, so Save asks where it should go and can never
//!   be sent to the file of the scene it replaced. Until something is done to
//!   it nothing in it is unsaved, so closing asks nothing and no recovery
//!   copy is written for it (`FileState::new_scene`).
//! - The hosted tools are told their objects went, exactly as for an Open.
//!   What stood on those objects (unmerged tubes, planned keys) goes with
//!   them; what stands by itself (SSpheres, a lathe outline, rulers) is still
//!   there. `Loss::SceneReplaced` says which, and the question asked before a
//!   new scene names what it says (`unsaved.rs`). A live array belongs to its
//!   object and comes back with it on Undo.
//!
//! What a new scene leaves as it is. The look (material, finish, lighting,
//! backdrop) stays as it is on screen, although the app itself starts with
//! the default look: it was chosen for working in, it is one click to
//! change, and the document names none. (The view is the starting one, and
//! Undo gives the scene from before its own view back: `files.rs`,
//! `seen_stepped`.) The brush, the tool that is up, the interface size and
//! the other settings are the user's, not the scene's, and stay too.
//!
//! When it is not done. A stroke or a gizmo drag in progress ignores it, as
//! it ignores every command (`Running::act`). While a background job is at
//! work (a merge, a cut) it is refused with a line that says so: the job was
//! started for an object that would be gone when it came back, and what it
//! made would land in the new scene. While a 360 loop is being recorded the
//! panels are away and the keys are the recording's, so it cannot be asked
//! for; it is refused all the same. A timelapse simply goes on recording: the
//! new scene is part of the session.

use glam::{Quat, Vec3};
use sculpt_toolkit::Loss;

use super::Running;
use super::unsaved::new_scene_question;
use crate::camera::Camera;
use crate::history::FileState;
use crate::io;
use crate::mesh::Mesh;
use crate::mesh::bvh::Aabb;
use crate::mesh::primitives::{self, Primitive};

mod play;
#[cfg(test)]
mod tests;

/// The shape every scene starts as: a ball one unit across, of about
/// `triangles` triangles.
pub(super) fn starting_mesh(triangles: f32) -> Mesh {
    primitives::build(Primitive::Sphere, triangles as usize, 0.5)
}

/// The view every scene starts with: all of `bounds` in the picture, from a
/// little to one side and above. `aspect` is the window's width over height.
pub(super) fn starting_view(bounds: Aabb, aspect: f32) -> Camera {
    let mut camera = Camera::default();
    camera.frame(bounds, aspect, false);
    (camera.yaw, camera.pitch) = (25.0, 15.0);
    camera
}

/// The scene New Scene puts in place, as a file would hold it: what the app
/// starts with. One ball of about `triangles` triangles, showing, in the
/// middle, with no symmetry set and selected, and the starting view of it.
/// It names no look, so the one on screen stays (see the top of this file).
pub(super) fn fresh_document(triangles: f32, aspect: f32) -> io::SavedScene {
    let mesh = starting_mesh(triangles);
    let view = starting_view(mesh.bounds(), aspect).saved_view();
    let ball = io::SavedObject {
        name: Primitive::Sphere.name().to_owned(),
        visible: true,
        mesh: 0,
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };
    let mut details = io::Details::default();
    details.set(io::slot::CAMERA, &view);
    let lost = io::Lost::default();
    io::SavedScene { meshes: vec![mesh], objects: vec![ball], active: 0, details, repaired: 0, left_out: 0, lost }
}

/// Why a new scene cannot be started just now, in words for the status line;
/// `None` when it can. `job` is the name of a background job that is still
/// at work, `looping` whether a 360 loop is being recorded.
pub(super) fn refusal(job: Option<&str>, looping: bool) -> Option<String> {
    if looping {
        let wait = "A 360 loop is being recorded. Let it finish, or press Esc to cancel it, then start a new scene.";
        return Some(wait.to_owned());
    }
    let job = job?;
    Some(format!("{job} is still working. Wait for it, or press its Cancel, then start a new scene."))
}

impl Running {
    /// Why a new scene cannot be started just now, if it cannot.
    fn new_scene_refused(&self) -> Option<String> {
        let jobs = self.doc.tools.jobs.running();
        refusal(jobs.first().map(|job| job.label.as_str()), self.doc.turntable.recording.is_some())
    }

    /// The New Scene button, or Ctrl + N. Asks first if the scene has changes
    /// that are not saved, or if work that is in no file stands on its
    /// objects; with neither, the scene it replaces is one Undo away, file
    /// name and all, so it is simply done.
    pub(super) fn new_scene_action(&mut self) {
        if let Some(why) = self.new_scene_refused() {
            return self.status(why, false);
        }
        let lost = self.scaffolds_at_risk(Loss::SceneReplaced);
        match new_scene_question(self.unsaved(), lost) {
            Some(question) => self.ui.graph.ask(question),
            None => self.new_scene(),
        }
    }

    /// The answer to that question. Saving first falls back to Save As for a
    /// scene with no file yet; if that is cancelled, or the save fails, the
    /// scene stays rather than be put aside unsaved.
    pub(super) fn new_scene_as(&mut self, save: bool) {
        if save && !self.save_action(false) {
            return;
        }
        self.new_scene();
    }

    /// Puts a fresh scene in place of the one that is open, as one undo step.
    /// Nothing is asked here: the interface asks first (`new_scene_action`).
    pub(super) fn new_scene(&mut self) {
        if let Some(why) = self.new_scene_refused() {
            return self.status(why, false);
        }
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let document = fresh_document(self.doc.opts.new_shape_tris, aspect);
        self.put_in_place(document, FileState::new_scene());
        self.status("New scene. Undo (Z) brings back the scene you had.", true);
        // The new scene is seen through the lens last chosen, framed as it was.
        self.doc.camera.set_lens(self.doc.opts.lens);
        self.toast("New Scene");
    }
}
