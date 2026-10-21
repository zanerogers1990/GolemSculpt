//! What closing the app, or putting another scene in place of this one (an
//! Open, a New Scene), would lose. The rest of the app asks two things, and
//! these are the two functions to call:
//!
//! - **Is anything unsaved?** [`Running::unsaved`] (app/files.rs): the scene
//!   differs from what its file holds. Save cures it.
//! - **What scaffold work would be lost?** [`Running::scaffolds_at_risk`]:
//!   the things that are in no file at all (SSpheres, unmerged tubes, planned
//!   keys, a lathe outline), each as a phrase a question can show. Save does
//!   not cure it; only finishing the work does (Make Skin, Merge, Build Keys,
//!   Create Mesh).
//!
//! Anything that closes the app or replaces the scene on the user's behalf
//! should refuse, or ask, when either has an answer; `closing_question`,
//! `opening_question` and `new_scene_question` are how the window's own close
//! button, Open and New Scene do.
//!
//! **Unsaved** is told by the scene's [`Mark`]. Most of what a file keeps
//! changes as a step of the undo history, and the history's own count tells
//! those. Some of it does not, on purpose: an object's symmetry, what Local
//! View shows and hides, and what a tool keeps for an object
//! (a live array's count, sizes, ticks and path) are saved with the scene
//! but are not undo steps. So the mark also carries a fingerprint of that
//! part of the file, [`kept_print`], worked out from the scene as it stands.
//! Being a fingerprint of the state and not a count of changes, it comes
//! back when the change is taken back: hide an object and show it again, and
//! nothing is unsaved.
//!
//! The fingerprint reads every object's entry as the file would write it,
//! except its mesh. It leaves out what the README says does not count: the
//! look, the camera view, whether the symmetry guides show, and which object
//! is selected.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use glam::Vec3;
use sculpt_symmetry::MirrorRepeater;
use sculpt_toolkit::Loss;

use super::Running;
use super::tools::Tools;
use crate::doc::Doc;
use crate::history::History;
use crate::scene::{Object, Scene, Symmetry};
use crate::sculpt::Radial;
use crate::ui::question::Question;

/// Where the scene stands, for telling whether it has changed: two marks
/// that differ mean it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mark {
    /// The undo history: steps ever recorded, steps to undo, steps to redo.
    history: (u64, usize, usize),
    /// What the file keeps that is not a step of that history ([`kept_print`]).
    kept: u64,
}

impl Mark {
    pub(super) fn of(history: &History, scene: &Scene, tools: &Tools) -> Mark {
        let (undo, redo) = history.steps();
        Mark { history: (history.edits, undo, redo), kept: kept_print(scene, tools) }
    }

    /// The fingerprint of what the file keeps outside the undo history, for a
    /// scripted check to print.
    pub(super) fn kept(self) -> u64 {
        self.kept
    }
}

fn hash_floats(values: &[f32], hasher: &mut impl Hasher) {
    for value in values {
        value.to_bits().hash(hasher);
    }
}

fn hash_vec3(v: Vec3, hasher: &mut impl Hasher) {
    hash_floats(&v.to_array(), hasher);
}

/// An object's symmetry, field by field. Taken apart by name, so that a field
/// added to `Symmetry` or `Radial` (both are saved) does not compile until it
/// is hashed here too.
fn hash_symmetry(symmetry: Symmetry, hasher: &mut impl Hasher) {
    let Symmetry { mirror, radial, space } = symmetry;
    let Radial { on, count, axis, custom_axis } = radial;
    (mirror, on, count, std::mem::discriminant(&axis), std::mem::discriminant(&space)).hash(hasher);
    hash_vec3(custom_axis, hasher);
}

/// A fingerprint of everything a file would keep of the objects besides
/// their meshes: each object's name, whether it shows, which objects share a
/// mesh, where it stands, its live mirror, its own symmetry, and what each
/// tool keeps for it. The same scene gives the same number; a change to any
/// of these gives, in practice, another. About a tenth of a microsecond an
/// object, and some twenty for an array along a path of 64 points
/// (`timing_of_the_kept_fingerprint`): cheap enough to ask whenever a
/// question is, and often enough for the recovery copies (app/recovery.rs).
pub(super) fn kept_print(scene: &Scene, tools: &Tools) -> u64 {
    let mut hasher = DefaultHasher::new();
    // Meshes in the order the file would write them: an object says which is
    // its own by that number, and so says which objects are instances.
    let mut meshes: Vec<u64> = Vec::new();
    for object in &scene.objects {
        // By name, for the same reason as in `hash_symmetry`.
        let Object { id, name, visible, mesh, position, rotation, scale, mirror } = object;
        let row = meshes.iter().position(|kept| kept == mesh).unwrap_or_else(|| {
            meshes.push(*mesh);
            meshes.len() - 1
        });
        (name, visible, row).hash(&mut hasher);
        hash_vec3(*position, &mut hasher);
        hash_floats(&rotation.to_array(), &mut hasher);
        hash_vec3(*scale, &mut hasher);
        match mirror {
            Some(MirrorRepeater { axis_mask, center }) => {
                axis_mask.hash(&mut hasher);
                hash_vec3(*center, &mut hasher);
            }
            None => 0u8.hash(&mut hasher),
        }
        hash_symmetry(scene.symmetry(*id), &mut hasher);
        tools.hash_kept(*id, &mut hasher);
    }
    hasher.finish()
}

/// The question to ask before the app closes, or `None` when it can close at
/// once: nothing is unsaved and no scaffold would be lost.
pub(super) fn closing_question(unsaved: bool, lost: Vec<String>) -> Option<Question> {
    (unsaved || !lost.is_empty()).then_some(Question::Exit { unsaved, lost })
}

/// The question to ask before a chosen file comes in, or `None` when it can
/// simply take the scene's place: nothing is unsaved and nothing stands on
/// the scene that would be lost, as for New Scene. The scene it replaces is
/// then one Undo away, file name and all, and adding a file's objects to the
/// scene instead is what Import is for. `lost` is what replacing the scene
/// would cost (`Loss::SceneReplaced`).
pub(super) fn opening_question(path: PathBuf, unsaved: bool, lost: Vec<String>) -> Option<Question> {
    (unsaved || !lost.is_empty()).then_some(Question::Open { path, unsaved, lost })
}

/// The question to ask before a new scene takes this one's place, or `None`
/// when it simply can: nothing is unsaved and nothing stands on the scene
/// that would be lost (`lost` is `Loss::SceneReplaced`, as for an Open), so
/// all that is put aside is one Undo away, file name and all.
pub(super) fn new_scene_question(unsaved: bool, lost: Vec<String>) -> Option<Question> {
    (unsaved || !lost.is_empty()).then_some(Question::NewScene { unsaved, lost })
}

impl Doc {
    /// The scene's mark now (see [`Running::history_mark`]).
    pub(super) fn mark(&self) -> Mark {
        Mark::of(&self.history, &self.scene, &self.tools)
    }

    /// The scaffold work that `loss` would cost: one phrase for each kind,
    /// SSpheres first, then the hosted tools in the order of their buttons.
    /// Empty when there is none.
    pub(crate) fn scaffolds_at_risk(&self, loss: Loss) -> Vec<String> {
        let mut lost = Vec::new();
        // Another scene (an Open, a New Scene) leaves the spheres standing
        // where they are; only closing loses them.
        if loss == Loss::Closing {
            let spheres = &self.spheres;
            let skin_in_scene = spheres.skin_object.is_some_and(|id| self.scene.get(id).is_some());
            lost.extend(spheres.at_risk(skin_in_scene));
        }
        lost.extend(self.tools.scaffolds_at_risk(loss));
        lost
    }
}

impl Running {
    /// What closing the app (`Loss::Closing`) or putting another scene in
    /// place of this one (`Loss::SceneReplaced`) would lose that is in no
    /// file, and so is lost whether or not the scene is saved: a phrase for
    /// each kind of work ("3 unmerged tubes (Merge or Keep Separate first)").
    /// Empty when nothing would be. With [`Running::unsaved`], this is the
    /// whole of "would anything be lost".
    pub(super) fn scaffolds_at_risk(&self, loss: Loss) -> Vec<String> {
        self.doc.scaffolds_at_risk(loss)
    }

    /// The question the close button asks, as things stand; `None` if the
    /// app can close without asking.
    pub(super) fn question_before_closing(&self) -> Option<Question> {
        closing_question(self.unsaved(), self.scaffolds_at_risk(Loss::Closing))
    }

    /// For scripted runs: makes one change of the kind a file keeps, through
    /// the routine the key or the card runs. `mirror` is the X key (Mirror X
    /// of the selected object) and `local` the / key, neither an undo step;
    /// `hide` is H and `show` Alt + H, which are.
    pub(super) fn kept_change(&mut self, what: &str) {
        match what {
            "mirror" => self.doc.edit_symmetry(|symmetry| symmetry.mirror[0] = !symmetry.mirror[0]),
            "hide" => self.hide_selection(),
            "show" => self.show_everything(),
            "local" => self.local_view_key(),
            _ => eprintln!("no such change as {what:?}: mirror, hide, show or local"),
        }
        self.redraw = true;
    }
}

mod play;
#[cfg(test)]
mod tests;
