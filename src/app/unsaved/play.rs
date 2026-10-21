//! A check, in the real app, of what closing and opening say would be lost:
//! the keys and tools are worked the way a person would, and a line is
//! printed per check, as `--demo-files play` does.
//!
//! ```text
//! sculpt.exe --screenshot out.png --demo-files lost
//! ```
//!
//! It asks the questions the close button and Open would ask, through the
//! routines they call, and never opens a box of the system's: nothing is
//! saved but to the files of the check, by name.

use std::path::Path;

use sculpt_toolkit::Loss;

use crate::app::Running;
use crate::doc::{Action, OpenHow};
use crate::mesh::primitives::Primitive;
use crate::ui::question::Question;

fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

impl Running {
    /// Saves the scene to the check's own file, which is its document from then on.
    fn lost_save(&mut self, file: &Path) -> bool {
        let saved = self.save_scene(file);
        self.doc.path = Some(file.to_owned());
        saved.is_ok() && !self.unsaved()
    }

    /// What the close button would ask now: whether it says the scene is
    /// unsaved, and what it names as lost. `None` if the app would close.
    fn closing(&self) -> Option<(bool, Vec<String>)> {
        match self.question_before_closing()? {
            Question::Exit { unsaved, lost } => Some((unsaved, lost)),
            _ => None,
        }
    }

    /// Chooses `file` in Open, and gives back the question that came up,
    /// taken down again with Esc: whether it says the scene is unsaved, and
    /// what it names as lost.
    fn choosing(&mut self, file: &Path) -> Option<(bool, Vec<String>)> {
        self.open_chosen(file.to_owned());
        let asked = match self.ui.graph.question() {
            Some(Question::Open { unsaved, lost, .. }) => Some((*unsaved, lost.clone())),
            _ => None,
        };
        self.ui.graph.answer(false);
        asked
    }

    pub(in crate::app) fn lost_play(&mut self, folder: &Path) {
        let (file, other) = (folder.join("the scene.sclp"), folder.join("another.sclp"));
        say("the scene the app starts with closes without a question", self.closing().is_none());
        let other_written = self.write_copy(&other);
        say("a second sculpt for Open to choose", other_written.is_ok());

        self.lost_kept_changes(&file);
        self.lost_array(&file);
        self.lost_tubes(&file, &other);
        self.lost_spheres(&file, &other);
        self.lost_keys_and_lathe(&file, &other);

        // What an Open really does to the spheres, which is why it does not name them.
        let spheres = self.doc.spheres.rig.alive_count();
        self.open_as(other, OpenHow::Replace);
        let stand = spheres > 1 && self.doc.spheres.rig.alive_count() == spheres;
        say("a sculpt opened in place of the scene leaves the spheres as they were", stand);
    }

    /// The keys that change what a save writes without an undo step, and H,
    /// which is one.
    fn lost_kept_changes(&mut self, file: &Path) {
        self.act(Action::New(Primitive::Cube));
        say("a scene with a cube added saves clean", self.lost_save(file) && self.closing().is_none());
        let steps = self.doc.history.steps();
        for (key, what, back) in [("mirror", "X (Mirror X)", "mirror"), ("local", "/", "local")] {
            self.kept_change(key);
            let asked = self.closing();
            let ordinary = asked == Some((true, Vec::new()));
            say(&format!("{what} makes the scene unsaved, and closing asks the ordinary question"), ordinary);
            say("and is no undo step", self.doc.history.steps() == steps);
            self.kept_change(back);
            say("taken back by its key, nothing is unsaved", !self.unsaved() && self.closing().is_none());
            self.kept_change(key);
            say("and Save makes it clean", self.unsaved() && self.lost_save(file));
            // As it was, for the next key.
            self.kept_change(back);
            self.lost_save(file);
        }
        // H hides as the eye buttons do: one undo step, which Z takes back.
        self.kept_change("hide");
        say("H makes the scene unsaved", self.unsaved() && self.closing().is_some());
        say("and is an undo step", self.doc.history.steps().0 == steps.0 + 1);
        self.act(Action::Undo);
        say("which Z takes back", self.doc.scene.objects.iter().all(|o| o.visible));
        self.lost_save(file);
    }

    /// An array is saved with its object and is the Array tool's own: its
    /// edits are steps of the tool's history, not the scene's.
    fn lost_array(&mut self, file: &Path) {
        self.tools_demo("array");
        say("an array made is unsaved work", self.unsaved() && self.closing().is_some_and(|(unsaved, _)| unsaved));
        say("which Save keeps", self.lost_save(file));
        // Z with the tool up steps the array's own history first.
        let steps = self.doc.history.steps();
        self.act(Action::Undo);
        let own = self.doc.history.steps() == steps;
        say("Z on the array's own edit is unsaved work, though the scene's history stands", own && self.unsaved());
        say("and closing asks the ordinary question", self.closing() == Some((true, Vec::new())));
        self.act(Action::Redo);
        say("Shift + Z puts the array back: nothing unsaved", self.doc.history.steps() == steps && !self.unsaved());
        self.tools_leave();
    }

    /// Live tubes stand on an object: closing loses them, and so does a
    /// scene opened in place of this one. Adding a file's objects does not.
    fn lost_tubes(&mut self, file: &Path, other: &Path) {
        self.tools_demo("tube");
        // The demo sets the object's symmetry for its picture, which is a change of its own.
        self.lost_save(file);
        let tube = vec!["1 unmerged tube (Merge or Keep Separate first)".to_owned()];
        say("a live tube on a saved scene: closing asks, and names it", self.closing() == Some((false, tube.clone())));
        self.tools_leave();
        say("put away, the tube is still named", self.closing() == Some((false, tube.clone())));
        say("choosing a file asks, and names it", self.choosing(other) == Some((false, tube.clone())));
        let objects = self.doc.scene.objects.len();
        say("Esc leaves the scene and the tube as they were", self.doc.scene.objects.len() == objects);
        say("and so does Enter, which is Cancel", {
            self.open_chosen(other.to_owned());
            let nothing = self.ui.graph.answer(true).is_none();
            nothing && self.doc.scene.objects.len() == objects && self.closing() == Some((false, tube.clone()))
        });
        // With a change to the scene as well, the one question says both.
        self.kept_change("mirror");
        say("with the scene changed too, closing says both", self.closing() == Some((true, tube.clone())));
        say("and so does choosing a file", self.choosing(other) == Some((true, tube.clone())));
        self.kept_change("mirror");
        // Adding a file's objects loses nothing, and asks nothing.
        self.open_as(other.to_owned(), OpenHow::Add);
        let added = self.doc.scene.objects.len() > objects && !self.ui.graph.asking();
        say("adding a file's objects asks nothing and keeps the tube", added && self.closing() == Some((true, tube)));
        self.act(Action::Undo);
        // Keep Separate makes every live tube an object of the scene.
        self.tools_demo("tube:object");
        self.tools_leave();
        say("kept separate, no tube is named", self.scaffolds_at_risk(Loss::Closing).is_empty());
        say("and the scene, with its new objects, saves clean", self.lost_save(file) && self.closing().is_none());
    }

    /// The spheres stand in the world: closing loses them, a new scene does not.
    fn lost_spheres(&mut self, file: &Path, other: &Path) {
        self.spheres_demo("attached");
        self.lost_save(file);
        let named = |lost: &[String]| {
            let rig = lost.first().map_or("", String::as_str);
            lost.len() == 1 && rig.starts_with("an SSphere rig of ") && rig.ends_with(" spheres (Make Skin first)")
        };
        let asked = self.closing().is_some_and(|(unsaved, lost)| !unsaved && named(&lost));
        say("an SSphere rig on a saved scene: closing asks, and names it", asked);
        let unnamed = self.choosing(other) == Some((false, Vec::new()));
        say("a new scene leaves the spheres standing, so Open does not name them", unnamed);
        self.act(Action::MakeSkin);
        say("after Make Skin the spheres are not named", self.scaffolds_at_risk(Loss::Closing).is_empty());
        say("and the skin is unsaved work like any object", self.closing() == Some((true, Vec::new())));
        self.act(Action::Undo);
        say("Z takes the skin away: the spheres are named again", named(&self.scaffolds_at_risk(Loss::Closing)));
        self.act(Action::Redo);
        self.lost_save(file);
    }

    /// Joints moved by hand and an outline shaped by hand; the ones the
    /// tools lay out by themselves are not work.
    fn lost_keys_and_lathe(&mut self, file: &Path, other: &Path) {
        self.tools_demo("lathe");
        say("the lathe's starting vase is not named", self.scaffolds_at_risk(Loss::Closing).is_empty());
        self.tools_demo("lathe:edit");
        let outline = vec!["a lathe outline you have shaped (Create Mesh first)".to_owned()];
        say("an outline shaped by hand is named before closing", self.closing() == Some((false, outline.clone())));
        say("and not before an Open, which leaves it standing", self.choosing(other) == Some((false, Vec::new())));
        self.tools_demo("lathe:created");
        say("created, it is an object and is not named", self.scaffolds_at_risk(Loss::Closing).is_empty());
        self.lost_save(file);

        // The Keys demo cuts the selected object in two and plans the joints.
        self.act(Action::New(Primitive::Sphere));
        self.tools_demo("keys");
        say("joints the Keys tool planned by itself are not named", self.scaffolds_at_risk(Loss::Closing).is_empty());
        self.tools_leave();
        self.act(Action::New(Primitive::Sphere));
        self.tools_demo("keys:move");
        let lost = self.scaffolds_at_risk(Loss::Closing);
        let planned = lost.len() == 1 && lost[0].ends_with("(Build Keys first)");
        say("a joint moved by hand: the planned keys are named before closing", planned);
        let by_an_open = self.scaffolds_at_risk(Loss::SceneReplaced);
        say("and before an Open, which takes the pieces they stand on", by_an_open == lost);
        self.tools_leave();
    }
}
