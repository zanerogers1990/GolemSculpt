//! A check, in the real app, of New Scene and the Exit button: worked through
//! the routines the buttons and the keys run, with real files, and printed a
//! line per check as `--demo-files play` is.
//!
//! ```text
//! sculpt.exe --screenshot out.png --demo --demo-files new          # the whole check
//! sculpt.exe --screenshot out.png --demo --demo-files new:fresh    # the File card after New Scene on a saved sculpt
//! sculpt.exe --screenshot out.png --demo --demo-files new:undone   # and after Undo of it
//! sculpt.exe --screenshot out.png --demo --demo-files ask-new      # the question New Scene asks of unsaved strokes
//! ```
//!
//! Nothing here asks the user anything: Save is taken as far as the box it
//! would put up, and no further (`save_target`), and "Save, Then New Scene"
//! is only pressed for a scene that has a file of its own to go to.

use std::path::Path;
use std::time::{Duration, Instant};

use glam::{Quat, Vec2, Vec3};
use sculpt_toolkit::{JobCtx, Loss};

use super::{starting_mesh, starting_view};
use crate::app::Running;
use crate::app::recovery::copy_owed;
use crate::doc::{Action, Tool};
use crate::look::Finish;
use crate::mesh::Mesh;
use crate::scene::Symmetry;
use crate::sculpt::PointerSample;
use crate::ui::question::Question;
use crate::{io, prefs, recovery};

fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

/// The fingerprint of the scene a file holds; 0 for a file that does not read.
fn print_of(path: &Path) -> u64 {
    let Ok(saved) = io::load_scene(path) else { return 0 };
    recovery::fingerprint(&saved.meshes.iter().collect::<Vec<&Mesh>>(), &saved.objects)
}

const TUBE: &str = "1 unmerged tube (Merge or Keep Separate first)";

impl Running {
    /// Whether the scene is the one the app starts with: one ball at the
    /// Starting Detail, in the middle, showing, selected, with no symmetry
    /// and no live mirror.
    fn is_fresh(&self) -> bool {
        let scene = &self.doc.scene;
        let [ball] = &scene.objects[..] else { return false };
        let placed = ball.position == Vec3::ZERO && ball.rotation == Quat::IDENTITY && ball.scale == Vec3::ONE;
        let plain = ball.visible && ball.mirror.is_none() && scene.symmetry(ball.id) == Symmetry::default();
        let selected = scene.active_id() == ball.id && scene.selected().to_vec() == [ball.id];
        let wanted = starting_mesh(self.doc.opts.new_shape_tris);
        let shape = self.doc.mesh.positions == wanted.positions && self.doc.mesh.indices == wanted.indices;
        ball.name == "Sphere" && placed && plain && selected && shape
    }

    /// Whether the scene is seen as a fresh start shows it.
    fn at_starting_view(&self) -> bool {
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let wanted = starting_view(self.doc.scene.bounds(&self.doc.mesh), aspect).saved_view();
        self.doc.camera.saved_view() == wanted
    }

    /// The question that is up, if it is New Scene's: whether it says the
    /// scene is unsaved, and what it names as lost.
    fn new_scene_asked(&self) -> Option<(bool, Vec<String>)> {
        match self.ui.graph.question()? {
            Question::NewScene { unsaved, lost } => Some((*unsaved, lost.clone())),
            _ => None,
        }
    }

    /// A new scene with no file and nothing unsaved, which closing would not ask about.
    fn stands_new(&self) -> bool {
        self.is_fresh() && self.doc.path.is_none() && !self.unsaved() && self.question_before_closing().is_none()
    }

    /// The whole check, or (`new:fresh`, `new:undone`) only a saved sculpt,
    /// New Scene and perhaps its Undo, for a picture of the File card.
    pub(in crate::app) fn new_scene_play(&mut self, folder: &Path, what: &str) {
        let (first, second) = (folder.join("first sculpt.sclp"), folder.join("second sculpt.sclp"));
        if what != "new" {
            // The Objects card shut, so the File card under it is in view.
            let shut = std::collections::BTreeMap::from([("Objects".to_owned(), false)]);
            prefs::cards::restore(&self.egui_ctx, &shut);
            let saved = self.save_to(first.clone());
            self.new_scene_action();
            if what == "new:undone" {
                self.act(Action::Undo);
            }
            if !saved || self.ui.graph.asking() {
                say("(the picture could not be set up)", false);
            }
            return;
        }
        self.new_over_unsaved_work();
        self.new_after_new();
        self.new_over_a_saved_sculpt(&first, &second);
        self.new_saving_first(&first);
        self.new_over_scaffolds(&first);
        self.new_while_busy();
        self.exit_button();
    }

    /// The run starts with the demo's strokes: unsaved work in a scene that
    /// has no file.
    fn new_over_unsaved_work(&mut self) {
        let start = (self.doc.path.clone(), self.unsaved(), self.print_now(), self.doc.history.steps().0);
        let mirrored = self.doc.scene.active_symmetry().mirror[0];
        let as_expected = start.0.is_none() && start.1 && mirrored;
        say("the run starts with unsaved strokes under Mirror X, in a scene with no file", as_expected);
        // A look, a brush and a view that are plainly not the ones the app starts with.
        let opts = &mut self.doc.opts;
        (opts.look.finish, opts.look.lighting) = (Finish::Metal, 6);
        (opts.matcap_on, opts.base_color) = (false, [0.8, 0.3, 0.1]);
        self.doc.sculptor.settings.radius_px = 77.0;
        (self.doc.camera.yaw, self.doc.camera.pitch, self.doc.camera.ortho) = (140.0, -20.0, true);
        self.sync_view();

        self.new_scene_action();
        let unsaved_only = Some((true, Vec::new()));
        say("New Scene over unsaved work asks, and names nothing else", self.new_scene_asked() == unsaved_only);
        let untouched = self.print_now() == start.2 && self.doc.history.steps().0 == start.3;
        say("and nothing has changed while it asks", untouched);
        let enter = self.ui.graph.question().and_then(Question::default_action);
        say("Enter would save first", enter == Some(Action::NewSceneAs { save: true }));
        let asks_where = self.save_target(false).is_none();
        say("which, with no file yet, asks where (the check stops short of that box)", asks_where);
        self.ui.graph.answer(false);
        say("Esc leaves the scene alone", !self.ui.graph.asking() && self.print_now() == start.2 && self.unsaved());

        self.act(Action::NewSceneAs { save: false });
        say("New Scene Without Saving: one sphere at the Starting Detail, selected, with no symmetry", self.is_fresh());
        say("seen from the view the app starts with", self.at_starting_view() && !self.doc.camera.ortho);
        say("it is one undo step", self.doc.history.steps() == (start.3 + 1, 0));
        say("it has no file", self.doc.path.is_none());
        say("and nothing in it is unsaved", !self.unsaved());
        say("so closing straight after asks nothing", self.question_before_closing().is_none());
        say("and no recovery copy is owed for it", !copy_owed(true, self.unsaved(), None, self.history_mark()));
        say("Save would ask where", self.save_target(false).is_none());
        let opts = &self.doc.opts;
        let look = (opts.look.finish, opts.look.lighting, opts.matcap_on, opts.base_color);
        say("the look stayed as it was", look == (Finish::Metal, 6, false, [0.8, 0.3, 0.1]));
        say("and the brush", self.doc.sculptor.settings.radius_px == 77.0);

        self.act(Action::Undo);
        let back = self.print_now() == start.2 && self.doc.scene.active_symmetry().mirror[0];
        say("Undo brings the scene from before back", back);
        say("with no file, as it had none, and still unsaved", self.doc.path.is_none() && self.unsaved());
        say("and New Scene would ask again", {
            self.new_scene_action();
            let asked = self.new_scene_asked() == Some((true, Vec::new()));
            self.ui.graph.answer(false);
            asked
        });
        self.act(Action::Redo);
        say("Redo: the new scene again, with no file and nothing unsaved", self.stands_new());
    }

    /// New Scene on a new scene, untouched and then edited.
    fn new_after_new(&mut self) {
        let steps = self.doc.history.steps().0;
        self.new_scene_action();
        let another = !self.ui.graph.asking() && self.stands_new() && self.doc.history.steps().0 == steps + 1;
        say("New Scene on an untouched new scene asks nothing and gives another, one more undo step", another);
        self.edit();
        let asks_where = self.save_target(false).is_none();
        say("an edit makes it unsaved, and Save would still ask where", self.unsaved() && asks_where);
        self.new_scene_action();
        say("New Scene now asks", self.new_scene_asked() == Some((true, Vec::new())));
        self.ui.graph.answer(false);
        self.act(Action::NewSceneAs { save: false });
        say("New Scene Without Saving: a third", self.stands_new());
        self.act(Action::Undo);
        let edited = self.doc.scene.objects.len() == 2 && self.doc.path.is_none() && self.unsaved();
        say("Undo: the edited scene, still with no file and still unsaved", edited);
        self.act(Action::Undo);
        self.act(Action::Undo);
        say("Undo twice more: the first new scene, with nothing unsaved", self.stands_new());
        self.act(Action::Undo);
        let strokes = !self.is_fresh() && self.doc.path.is_none() && self.unsaved();
        say("and once more: the strokes the run began with, unsaved", strokes);
        for _ in 0..4 {
            self.act(Action::Redo);
        }
        say("Redo all the way: the third new scene", self.stands_new() && !self.doc.history.can_redo());
    }

    /// A sculpt with a file of its own: the new scene must never be saved over it.
    fn new_over_a_saved_sculpt(&mut self, first: &Path, second: &Path) {
        self.edit();
        let saved = self.save_to(first.to_owned());
        let old = self.print_now();
        say("a sculpt saved to a file", saved && self.is_file(first) && !self.unsaved() && print_of(first) == old);

        let steps = self.doc.history.steps().0;
        self.new_scene_action();
        let made = !self.ui.graph.asking() && self.is_fresh() && self.doc.history.steps().0 == steps + 1;
        say("New Scene on a saved sculpt asks nothing: it is one Undo away", made);
        say("the new scene has no file and nothing unsaved", self.doc.path.is_none() && !self.unsaved());
        let asks = self.save_target(false).is_none() && self.save_target(true).is_none();
        say("Save would ask where: it has no way to the file of the sculpt it replaced", asks);
        self.edit();
        say("after an edit Save still asks where", self.unsaved() && self.save_target(false).is_none());
        say("and the sculpt's file is as it was", print_of(first) == old);
        self.act(Action::Undo);
        self.act(Action::Undo);
        let back = self.print_now() == old && self.is_file(first) && !self.unsaved();
        say("Undo twice: the sculpt is back, with its file and nothing unsaved", back);
        let wrote = self.save_unasked();
        say("Save writes it to its own file, which holds it", wrote.as_deref() == Some(first) && print_of(first) == old);
        self.act(Action::Redo);
        self.act(Action::Redo);
        let forward = self.doc.scene.objects.len() == 2 && self.doc.path.is_none() && self.unsaved();
        say("Redo twice: the new scene and its edit, with no file", forward && self.save_target(false).is_none());
        self.act(Action::Undo);
        self.act(Action::Undo);

        // Saved under a name of its own, the new scene keeps that name through Undo and Redo.
        self.new_scene_action();
        let saved_as = self.save_to(second.to_owned());
        let sphere = self.print_now();
        say("a new scene saved under a name of its own", saved_as && self.is_file(second) && !self.unsaved());
        say("each file holds its own scene", print_of(first) == old && print_of(second) == sphere && sphere != old);
        self.act(Action::Undo);
        let back = self.print_now() == old && self.is_file(first) && !self.unsaved();
        say("Undo: the first sculpt and its own file", back);
        self.act(Action::Redo);
        let forward = self.print_now() == sphere && self.is_file(second) && !self.unsaved();
        say("Redo: the second, under the name it was saved as", forward);
        self.act(Action::Undo);
    }

    /// "Save, Then New Scene", for a sculpt that has a file to be saved to.
    fn new_saving_first(&mut self, first: &Path) {
        self.edit();
        let edited = self.print_now();
        self.new_scene_action();
        say("a saved sculpt with changes: New Scene asks", self.new_scene_asked() == Some((true, Vec::new())));
        let own_file = self.save_target(false).as_deref() == Some(first);
        let enter = self.ui.graph.answer(true);
        say("Enter is Save, Then New Scene", enter == Some(Action::NewSceneAs { save: true }));
        if !own_file {
            return say("(the sculpt has no file of its own: saving first would ask where, so the check stops)", false);
        }
        self.act(Action::NewSceneAs { save: true });
        say("the sculpt was saved to its own file first", print_of(first) == edited);
        say("and the new scene took its place, with no file", self.stands_new());
        self.act(Action::Undo);
        let back = self.print_now() == edited && self.is_file(first) && !self.unsaved();
        say("Undo: the sculpt and its file, with nothing unsaved since it was saved", back);
    }

    /// Work that is in no file: what stands on the scene's objects goes with
    /// them and is named; what stands by itself stays and is not.
    fn new_over_scaffolds(&mut self, first: &Path) {
        // A live array is the object's own, and is saved with it.
        self.tools_demo("array");
        self.save_to(first.to_owned());
        let arrays = self.kept_arrays();
        say("an array on a saved sculpt", arrays.iter().any(Option::is_some) && !self.unsaved());
        self.new_scene_action();
        say("New Scene asks nothing: the array is in the file", !self.ui.graph.asking() && self.is_fresh());
        say("the new sphere has no array", self.kept_arrays().iter().all(Option::is_none));
        say("and the Array tool has no step of its own to answer Z before the scene's", !self.doc.tools.can_undo());
        self.act(Action::Undo);
        let back = self.kept_arrays() == arrays && self.is_file(first) && !self.unsaved();
        say("Undo: the sculpt with the array it had", back);
        self.tools_leave();

        // A live tube stands on its object.
        self.new_scene_action();
        self.tools_demo("tube");
        self.tools_leave();
        // The demo sets the object's symmetry for its picture, which is a change of its own.
        let saved = self.save_to(first.to_owned());
        let (before, tube) = (self.print_now(), vec![TUBE.to_owned()]);
        say("a live tube on a saved sculpt", saved && self.scaffolds_at_risk(Loss::SceneReplaced) == tube);
        self.new_scene_action();
        say("New Scene asks, and names the tube", self.new_scene_asked() == Some((false, tube.clone())));
        let enter = self.ui.graph.answer(true);
        let stayed = self.print_now() == before && self.scaffolds_at_risk(Loss::SceneReplaced) == tube;
        say("Enter is Cancel: the sculpt and the tube stay", enter.is_none() && stayed);
        self.kept_change("mirror");
        self.new_scene_action();
        let both = Some((true, tube.clone()));
        say("with the sculpt changed too, the one question says both", self.new_scene_asked() == both);
        say("and Enter is still Cancel", self.ui.graph.answer(true).is_none() && self.print_now() == before);
        self.kept_change("mirror");
        self.act(Action::NewSceneAs { save: false });
        self.sync_view();
        self.tools_frame();
        let gone = self.scaffolds_at_risk(Loss::Closing).is_empty();
        say("New Scene and Lose Them: a new scene, and the tube is gone as the question said", self.stands_new() && gone);
        self.act(Action::Undo);
        self.sync_view();
        self.tools_frame();
        let back = self.print_now() == before && self.is_file(first) && !self.unsaved();
        say("Undo brings the sculpt and its file back", back);
        say("but not the tube, as the question said", self.scaffolds_at_risk(Loss::Closing).is_empty());

        // The same with the Tube tool still up, where Z could have been a step of the tool's own.
        self.new_scene_action();
        self.tools_demo("tube");
        let saved = self.save_to(first.to_owned());
        let before = self.print_now();
        say("a live tube again, the Tube tool left up", saved && self.scaffolds_at_risk(Loss::SceneReplaced) == tube);
        self.act(Action::NewSceneAs { save: false });
        self.sync_view();
        self.tools_frame();
        let gone = self.scaffolds_at_risk(Loss::Closing).is_empty();
        say("New Scene with the tool up: the tube goes the same way", self.stands_new() && gone);
        self.act(Action::Undo);
        let back = self.print_now() == before && self.is_file(first) && !self.unsaved();
        let gone = self.scaffolds_at_risk(Loss::Closing).is_empty();
        say("and Z is the scene's: the sculpt and its file are back, without the tube", back && gone);
        self.tools_leave();

        // Spheres stand in the world: a new scene leaves them, and lets go of the object they rested on.
        self.spheres_demo("attached");
        self.set_tool(Tool::Sculpt);
        self.save_to(first.to_owned());
        let spheres = self.doc.spheres.rig.alive_count();
        let attached = self.doc.spheres.attached_to().is_some();
        say("an SSphere rig attached to an object of a saved sculpt", spheres > 1 && attached && !self.unsaved());
        self.new_scene_action();
        say("New Scene does not name the spheres, so it asks nothing", !self.ui.graph.asking() && self.is_fresh());
        self.spheres_frame();
        say("the spheres stand as they were", self.doc.spheres.rig.alive_count() == spheres);
        say("attached to nothing, now their object has gone", self.doc.spheres.attached_to().is_none());
        let named = self.scaffolds_at_risk(Loss::Closing);
        say("and closing would still ask about them", named.len() == 1 && named[0].starts_with("an SSphere rig of "));
        say("though nothing of the new scene is unsaved", !self.unsaved() && self.doc.path.is_none());
    }

    /// In the middle of a stroke, and while a background job is at work.
    fn new_while_busy(&mut self) {
        let ids = |app: &Running| app.doc.scene.objects.iter().map(|o| o.id).collect::<Vec<u64>>();
        let before = ids(self);
        let work = |job: &JobCtx| -> Result<(), String> {
            while !job.cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        };
        let mut job = self.doc.tools.jobs.spawn("A slow job", work);
        self.new_scene_action();
        let refused = !self.ui.graph.asking() && ids(self) == before;
        say("while a background job is at work New Scene is refused", refused);
        println!("     the status line: {}", self.ui.status.0);
        say("with a line that names the job", self.ui.status.0.starts_with("A slow job is still working."));
        self.act(Action::NewSceneAs { save: false });
        say("and so is the answer to a question that was already up", ids(self) == before);
        self.doc.tools.jobs.cancel(0);
        let started = Instant::now();
        while job.poll().is_none() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(2));
        }
        self.new_scene_action();
        let went = !self.doc.tools.jobs.is_busy() && ids(self) != before && self.is_fresh();
        say("once the job is over, New Scene goes ahead", went);

        // A stroke begun on the new sphere and not yet let go.
        self.sync_view();
        let centre = self.view.size * 0.5;
        let at = |i: usize| {
            let pos = centre + Vec2::new(3.0 * i as f32, 0.0);
            PointerSample { pos, pressure: None, time: i as f64 / 240.0 }
        };
        let before = ids(self);
        let doc = &mut self.doc;
        let began = doc.begin_stroke(&self.local, at(0), false, false);
        for i in 1..=12 {
            doc.sculptor.sample(&mut doc.mesh, &self.local, at(i));
        }
        self.act(Action::NewScene);
        let ignored = began && self.doc.sculptor.is_active() && !self.ui.graph.asking() && ids(self) == before;
        say("in the middle of a stroke New Scene does nothing, like every other command", ignored);
        self.finish_stroke();
        self.new_scene_action();
        let unsaved_only = Some((true, Vec::new()));
        say("the stroke finished, it is unsaved work and New Scene asks", self.new_scene_asked() == unsaved_only);
        self.ui.graph.answer(false);
    }

    /// The Exit button: the close button's question, or none.
    fn exit_button(&mut self) {
        let would_ask = self.question_before_closing();
        let (closing, asked) = self.exit_pressed();
        let same = !closing && asked.is_some() && asked == would_ask;
        say("Exit with work that would be lost asks what the close button asks", same);
        if let Some(Question::Exit { unsaved, lost }) = &asked {
            println!("     it says: unsaved {unsaved}, and would lose {lost:?}");
        }
        say("and the app stays open until it is answered", !self.quitting());
        // Nothing unsaved and nothing in no file: put the spheres away with the scene saved as it is.
        self.doc.spheres = Default::default();
        self.act(Action::NewSceneAs { save: false });
        let (closing, asked) = self.exit_pressed();
        say("Exit with nothing to lose closes at once, as the close button does", closing && asked.is_none());
    }
}
