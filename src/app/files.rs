//! Files, on the app's side: saving and opening `.sclp` scenes, the questions
//! asked before work could be lost, the settings remembered between runs and
//! the recent files. The formats themselves are in `io.rs`, `prefs.rs` and
//! `recent.rs`; this is where they meet the running app.
//!
//! "Unsaved" means the scene's mark has moved since the scene was last saved
//! or opened. The mark is the undo history (something was sculpted, added,
//! moved, undone) together with what a file keeps that is not an undo step:
//! an object's symmetry, what the hide keys show, a live array's settings
//! (`unsaved.rs`). Changing only the look or the view does not count, so
//! trying a matcap and closing the app asks nothing. (The Unity app has no
//! such flag and asks every time.)
//!
//! What is in no file at all (SSpheres, unmerged tubes and the like) is
//! another matter, which the same questions name: also `unsaved.rs`.
//!
//! Which file the scene belongs to, and whether it is unsaved, follow the
//! scene through Undo and Redo of an Open: see `file_undo.rs`. New Scene is
//! an Open of a scene that is in no file: see `new_scene.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};
use sculpt_toolkit::{Arrival, Loss};
use winit::event_loop::ActiveEventLoop;
use winit::window::Fullscreen;

use super::file_undo::File;
use super::tools::Tools;
pub(super) use super::unsaved::Mark;
use super::unsaved::opening_question;
use super::{Running, import_mesh, install, is_native, stem};
use crate::camera::{Camera, SavedView};
use crate::doc::{Action, OpenHow};
use crate::history::FileState;
use crate::io;
use crate::look::Look;
use crate::mesh::Mesh;
use crate::prefs::{self, BrushPrefs, Home, Prefs};
use crate::recent;
use crate::render::DrawItem;
use crate::scene::Scene;
use crate::printable::Cure;
use crate::ui::question::Question;
use crate::unity_import;

/// How often the settings are compared with what is on disk.
const SETTINGS_CHECK: Duration = Duration::from_secs(1);

pub(super) struct State {
    home: Home,
    /// The brush folder beside the settings file (see `app/brushes.rs`).
    pub(super) brushes: crate::brush_files::Shelf,
    /// The materials folder beside it (see `app/materials.rs`).
    pub(super) materials: crate::materials::files::Shelf,
    /// The settings as last written or read, to tell when they have changed.
    kept: Prefs,
    next_check: Instant,
    /// The scene's mark at which it is what its file holds (see `File::clean`).
    clean: Option<Mark>,
    /// The app has been told it may close.
    quit: bool,
    /// It was told so with a question about closing still to ask, which a
    /// scripted run for a picture never puts up: nobody gave anything up.
    unasked: bool,
    /// How many damaged points the last save wrote mended (see `io::save_scene_with`).
    mended_on_save: usize,
    /// How many damaged points the file last put in place of the scene had, mended as it was read.
    repaired_on_open: usize,
    /// How many triangles that file held that could not be used, and were left out.
    left_out_on_open: usize,
    /// How many of that file's meshes had subdivision levels or sculpt layers that could not be read.
    lost_on_open: io::Lost,
}

/// Everything there is to say about damage found as a file was read, as one
/// piece for a status line: see `Running::opened_damage`.
fn damage_notes(repaired: usize, left_out: usize, lost: io::Lost) -> Option<String> {
    let notes: Vec<String> = io::notes_opened(repaired, left_out).into_iter().chain(lost.note()).collect();
    (!notes.is_empty()).then(|| notes.join(" "))
}

impl State {
    pub(super) fn new(screenshot: bool) -> State {
        let home = Home::for_run(screenshot);
        let kept = home.load();
        let next_check = Instant::now() + SETTINGS_CHECK;
        // `clean` is set once the scene the app starts with is in place (`mark_saved`).
        let (quit, unasked, clean) = (false, false, None);
        let (mended_on_save, repaired_on_open, left_out_on_open) = (0, 0, 0);
        let lost_on_open = io::Lost::default();
        State {
            brushes: crate::brush_files::Shelf::new(home.folder(), home.writable()),
            materials: crate::materials::files::Shelf::new(home.folder(), home.writable()),
            home,
            kept,
            next_check,
            clean,
            quit,
            unasked,
            mended_on_save,
            repaired_on_open,
            left_out_on_open,
            lost_on_open,
        }
    }

    /// The Starting Detail the settings remember: how many triangles the
    /// sphere the app starts with is made of.
    pub(super) fn starting_detail(&self) -> f32 {
        self.kept.starting_detail()
    }
}

/// Whether the picture a saved look names is there to be loaded. The path
/// comes out of the file being opened, which may be anybody's, and it is
/// looked for whoever opens the file: the user, the Claude bridge's
/// `open_file` and `launch`, a recovery copy being restored. Merely asking
/// Windows whether `\\server\share\sky.hdr` is a file signs in to `server`,
/// which is what the bridge's rule about paths exists to prevent
/// (`bridge_guard::off_limits`). So a path that rule refuses is never asked
/// about (`is_file` is the look at the disk, and is not called), and counts
/// as a picture that is not there: the look is kept, with the note a missing
/// picture gets, and Load HDRI brings the picture back.
fn saved_picture_is_there(path: &Path, is_file: impl FnOnce(&Path) -> bool) -> bool {
    crate::bridge_guard::off_limits(&path.to_string_lossy()).is_none() && is_file(path)
}

/// Whether a close is the user's own doing as far as the scene's work goes:
/// there is nothing unsaved, or the question about closing was asked and
/// answered with a way to close (`told`, and not `unasked`). Anything else
/// that closes the app over unsaved work (the Claude bridge's quit, a
/// scripted run that is never asked) is not the user giving that work up.
fn settled(told: bool, unasked: bool, unsaved: bool) -> bool {
    !unsaved || (told && !unasked)
}

/// A file's name without its folders, for a status line.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// What the status line says when the file named on the command line could not
/// be opened: which file, why in plain words, and what the app shows instead.
pub(super) fn start_up_failure(file: &std::path::Path, error: &str) -> String {
    if !file.exists() {
        let name = file.file_name().map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().into_owned());
        return format!("{name} is not there, so nothing was opened. Starting with a new sphere instead.");
    }
    format!("{}. Starting with a new sphere instead.", error.trim_end_matches('.'))
}

fn how_many(count: usize) -> String {
    if count == 1 { "1 object".to_owned() } else { format!("{count} objects") }
}

/// Adds a loaded file's objects to the scene, where they were saved, under
/// names nothing in the scene has yet, and selects the first of them.
/// Objects that shared a mesh in the file still share it. Gives back the row
/// of the file and the id in the scene of each object that was added.
pub(super) fn append(scene: &mut Scene, slot: &mut Mesh, saved: io::SavedScene) -> Vec<(usize, u64)> {
    let mut added = Vec::new();
    let mut meshes: Vec<Option<Mesh>> = saved.meshes.into_iter().map(Some).collect();
    // For each mesh of the file, the first object added that shows it.
    let mut shown_by: Vec<Option<u64>> = vec![None; meshes.len()];
    let mut first = None;
    for (row, o) in saved.objects.into_iter().enumerate() {
        let Some(owner) = shown_by.get(o.mesh).copied() else { continue };
        let id = match (owner, meshes[o.mesh].take()) {
            (Some(owner), _) => {
                // An instance: a second object on a mesh that is already in.
                scene.select(owner, slot);
                let id = scene.instance_active();
                scene.move_to(id, scene.objects.len() - 1);
                if scene.active().name != o.name {
                    let name = scene.unique_name(&o.name);
                    scene.active_mut().name = name;
                }
                id
            }
            (None, Some(mesh)) => {
                let id = scene.add(&o.name, mesh, slot);
                shown_by[o.mesh] = Some(id);
                id
            }
            (None, None) => continue,
        };
        let object = scene.active_mut();
        (object.visible, object.position, object.rotation, object.scale) = (o.visible, o.position, o.rotation, o.scale);
        object.mirror = crate::scene::saved_mirror(&saved.details, row);
        // Its own symmetry comes with it, as its live mirror does.
        if let Some(symmetry) = crate::scene::saved_symmetry(&saved.details, row) {
            scene.place_symmetry(id, symmetry);
        }
        first.get_or_insert(id);
        added.push((row, id));
    }
    if let Some(first) = first {
        scene.select(first, slot);
    }
    added
}

impl Running {
    // ---- start-up, settings

    /// Start-up: puts back what the settings file remembered, then the look
    /// and view of the file named on the command line, if one was opened.
    pub(super) fn files_start(&mut self, opened: Option<io::Details>) {
        let kept = self.files.kept.clone();
        self.apply_prefs(&kept);
        if let Some(details) = opened {
            self.apply_details(&details);
            if let Some(path) = self.doc.path.clone() {
                self.note_recent(&path, false);
            }
            if let Some(note) = unity_import::status_note(&details, true) {
                self.status(note, true);
            }
        }
        self.mark_saved();
    }

    fn apply_prefs(&mut self, kept: &Prefs) {
        let opts = &mut self.doc.opts;
        opts.ui_scale = kept.interface_size();
        opts.tooltips = kept.tooltips;
        opts.folded = [kept.left_panel_folded, kept.right_panel_folded];
        opts.tabs = kept.tabs();
        opts.theme = kept.theme();
        opts.remesh_resolution = kept.remesh_resolution();
        opts.quad_target = kept.quad_target();
        opts.decimate_target = kept.decimate_target();
        opts.subdivide_flat = kept.subdivide_flat;
        opts.new_shape_tris = kept.starting_detail();
        self.doc.sculptor.dyntopo.enabled = kept.dynamic_topology;
        self.doc.sculptor.dyntopo.detail = kept.dynamic_topology_detail();
        self.doc.tools.unit = kept.length_unit();
        self.brushes_restore(kept);
        self.materials_restore();
        prefs::cards::restore(&self.egui_ctx, &kept.cards);
        sculpt_toolkit::floating::restore(&self.egui_ctx, &kept.floating_cards);
        self.ui.recent.restore(kept.recent.clone(), self.files.home.pictures());
        // Not for a run that was given a window size: it asked for a window.
        if kept.fullscreen && self.args.screenshot.is_none() && self.args.size.is_none() {
            self.window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
    }

    /// The settings as they are right now.
    pub(super) fn prefs_now(&self) -> Prefs {
        let opts = &self.doc.opts;
        let (library, settings) = (&self.doc.sculptor.library, &self.doc.sculptor.settings);
        Prefs {
            interface_size: opts.ui_scale,
            tooltips: opts.tooltips,
            fullscreen: self.window.fullscreen().is_some(),
            unit: self.doc.tools.unit.suffix().to_owned(),
            cards: prefs::cards::snapshot(&self.egui_ctx),
            left_panel_folded: opts.folded[0],
            right_panel_folded: opts.folded[1],
            sculpt_tab: opts.tabs.sculpt.name().to_owned(),
            scene_tab: opts.tabs.scene.name().to_owned(),
            theme: crate::ui::theme::THEMES[opts.theme.min(crate::ui::theme::THEMES.len() - 1)].name.to_owned(),
            floating_cards: sculpt_toolkit::floating::snapshot(&self.egui_ctx),
            // The file keeps the thirteen base brushes here, whichever
            // brushes are in hand; the others are files of their own.
            brush: BrushPrefs::of(&library.base_image(settings)),
            library: crate::brush_files::Kept::of(library, settings),
            remesh_resolution: opts.remesh_resolution,
            quad_target: opts.quad_target,
            decimate_target: opts.decimate_target,
            subdivide_flat: opts.subdivide_flat,
            starting_detail: opts.new_shape_tris,
            dynamic_topology: self.doc.sculptor.dyntopo.enabled,
            dynamic_topology_detail: self.doc.sculptor.dyntopo.detail,
            recent: self.ui.recent.paths().to_vec(),
        }
    }

    /// Writes the settings if they have changed since they were last written.
    /// Called now and then while the app runs, and as it closes.
    pub(super) fn save_prefs(&mut self) {
        self.brushes_keep();
        self.materials_keep();
        let now = self.prefs_now();
        if now == self.files.kept {
            return;
        }
        if let Err(e) = self.files.home.save(&now) {
            log::warn!("{e}");
        }
        // Even if that failed: trying again every second would not help.
        self.files.kept = now;
    }

    /// Once a frame: every second or so, between strokes, the settings are
    /// compared with what was last written.
    pub(super) fn files_frame(&mut self) {
        let now = Instant::now();
        if self.doc.sculptor.is_active() {
            return;
        }
        if now < self.files.next_check {
            // Frames are only drawn when something happens, and this may be
            // the last one for a while: ask for one more when the look is
            // due, so the last change before the app goes quiet is written.
            let due = self.files.next_check;
            self.repaint_at = Some(self.repaint_at.map_or(due, |at| at.min(due)));
            return;
        }
        self.files.next_check = now + SETTINGS_CHECK;
        self.save_prefs();
    }

    // ---- unsaved work, closing

    /// The scene's mark now. This one function is what "the scene has
    /// changed" means, for the question about unsaved work and for the
    /// recovery copies (app/recovery.rs) alike: a change that should count
    /// for both has to move it. It is the undo history's own count and, for
    /// what a file keeps that is not an undo step (a symmetry, the hide keys,
    /// an array's settings), a fingerprint of that part of the scene.
    pub(super) fn history_mark(&self) -> Mark {
        self.doc.mark()
    }

    /// Start-up: the scene is the one the app starts with, or what the file
    /// named on the command line holds. Nothing in it is unsaved.
    pub(super) fn mark_saved(&mut self) {
        self.files.clean = Some(self.history_mark());
    }

    /// Whether anything has been done to the scene since it was last saved
    /// or opened. It errs on the side of asking: the mark counts every step
    /// ever recorded, and that count never goes back, so undoing an edit made
    /// after a save still counts as unsaved although the scene is what the
    /// file holds again. Only Undo and then Redo back to the very point of
    /// the save is clean. A change that is not an undo step (a symmetry, a
    /// hide key, an array's settings) is unsaved until it is saved or put
    /// back as it was. Undo or Redo of an Open is another matter: the
    /// scene it arrives at is unsaved exactly if it differs from what is in
    /// its own file (`File::stepped`).
    ///
    /// This is half of "would anything be lost": what is in no file at all
    /// is `scaffolds_at_risk` (unsaved.rs).
    pub(super) fn unsaved(&self) -> bool {
        self.unsaved_at(self.history_mark())
    }

    /// The same, for a caller that has the scene's mark in hand already.
    pub(super) fn unsaved_at(&self, now: Mark) -> bool {
        Some(now) != self.files.clean
    }

    /// The scene's file, for `file_undo.rs` to work on.
    pub(super) fn file(&mut self) -> File<'_> {
        File {
            path: &mut self.doc.path,
            clean: &mut self.files.clean,
            restored: &mut self.recovery.restored,
            history: &mut self.doc.history,
        }
    }

    /// Undo (`undo`) or Redo has just taken a step: if it was an Open, or a
    /// restored recovery copy, the scene now on screen gets its own file
    /// back. `was_saved` is whether the scene had nothing unsaved before the
    /// step.
    pub(super) fn file_stepped(&mut self, undo: bool, was_saved: bool) {
        self.seen_stepped(undo);
        let now = self.history_mark();
        if !self.file().stepped(undo, was_saved, now) {
            return;
        }
        // The status line may still say the other file was opened.
        let unsaved = self.unsaved();
        let text = match &self.doc.path {
            Some(path) if unsaved => format!("Back to {}, with changes that are not saved.", file_name(path)),
            Some(path) => format!("Back to {}.", file_name(path)),
            None if unsaved => "Back to a scene that is not saved yet: Save (Ctrl + S) asks where to keep it.".into(),
            None => "Back to the scene from before, which has no file.".into(),
        };
        self.status(text, true);
    }

    /// Undo (`undo`) or Redo has just taken a step: if it put one scene in
    /// place of another, the scene now on screen is seen as it was when it
    /// was last on screen, with its own look and from its own view, and the
    /// step keeps those of the scene just left for the way back. An opened
    /// file brings a look and a view with it, and without this Undo of the
    /// Open left the scene from before lit, coloured and framed as the file.
    fn seen_stepped(&mut self, undo: bool) {
        if !self.doc.history.keeps_seen(undo) {
            return;
        }
        let here = self.seen();
        if let Some(there) = self.doc.history.cross_seen(undo, here.clone()) {
            self.see(&there, Some(&here));
        }
    }

    /// Whether this is still the scene the app started with, untouched and
    /// never saved: there is nothing in it to keep or to add to.
    fn untouched(&self) -> bool {
        self.doc.path.is_none() && !self.unsaved() && !self.doc.history.can_undo() && !self.doc.history.can_redo()
    }

    /// The window's close button, Alt+F4 or Ctrl+Q. Closes at once unless
    /// there is unsaved work, or work that is in no file (SSpheres, unmerged
    /// tubes and the like); then it asks, naming what would be lost.
    pub(super) fn close_requested(&mut self, event_loop: &ActiveEventLoop) {
        self.exit_action();
        if self.quitting() {
            event_loop.exit();
        }
    }

    /// The File card's Exit button: the close button, from the interface,
    /// which has no event loop to tell. It asks the same question when there
    /// is one to ask; when there is none it says the app may close, and the
    /// loop closes it between events (`App::before_waiting`), the way it
    /// does after "Exit Without Saving". Either way the app closes by the
    /// one road, so what that road looks after (the settings, the recovery
    /// copies, a timelapse still recording) is looked after here too.
    pub(super) fn exit_action(&mut self) {
        match self.question_before_closing() {
            // A scripted run for a picture has nobody to answer, and closes.
            // Nobody was asked, so nothing counts as given up (`close_settled`).
            Some(_) if self.args.screenshot.is_some() => (self.files.quit, self.files.unasked) = (true, true),
            Some(question) => self.ui.graph.ask(question),
            None => self.files.quit = true,
        }
        self.redraw = true;
    }

    /// Whether the app, closing now, closes properly as far as its work
    /// goes (see [`settled`]). The recovery copies are kept or removed by
    /// this (`recovery_exit`).
    pub(super) fn close_settled(&self) -> bool {
        settled(self.files.quit, self.files.unasked, self.unsaved())
    }

    /// The answer to that question. Saving first falls back to Save As for a
    /// scene with no file yet; if that is cancelled, or the save fails, the
    /// app stays open rather than close over work that was never written.
    pub(super) fn quit(&mut self, save: bool) {
        if save && !self.save_action(false) {
            return;
        }
        self.files.quit = true;
    }

    /// Whether the app has been told it may close.
    pub(super) fn quitting(&self) -> bool {
        self.files.quit
    }

    // ---- saving

    /// Save, or Save As: asks where if the scene has no file yet. Returns
    /// whether the scene was written.
    pub(super) fn save_action(&mut self, save_as: bool) -> bool {
        match self.save_target(save_as).or_else(|| self.ask_where_to_save()) {
            Some(path) => self.save_to(path),
            None => false,
        }
    }

    /// The file Save writes to without asking: the scene's own, if it has
    /// one. A sculpt from the Unity app is never saved over: Save asks where
    /// its .sclp should go, as Save As does, offering the same name.
    pub(super) fn save_target(&self, save_as: bool) -> Option<PathBuf> {
        self.doc.path.clone().filter(|path| !save_as && !unity_import::is_unity(path))
    }

    /// The box that asks where to save. `None` if it was cancelled.
    fn ask_where_to_save(&self) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title("Save Sculpt").add_filter("Sculpt", &["sclp"]);
        // Save As starts from the file the scene already has; a scene
        // restored after a crash, from the file it was a copy of. (Only that
        // scene: Undo of the restore takes the offer away with it.)
        dialog = match &self.doc.path.clone().or_else(|| self.recovered_document()) {
            Some(current) => {
                let unity = unity_import::is_unity(current);
                let dialog = dialog.set_file_name(if unity { unity_import::save_name(current) } else { file_name(current) });
                match current.parent() {
                    Some(folder) => dialog.set_directory(folder),
                    None => dialog,
                }
            }
            None => dialog.set_file_name("sculpt.sclp"),
        };
        dialog.save_file()
    }

    /// Writes the scene to `path`, which is its file from then on, and says
    /// so. Returns whether it was written.
    pub(super) fn save_to(&mut self, path: PathBuf) -> bool {
        match self.save_scene(&path) {
            Ok(()) => {
                let size = std::fs::metadata(&path).map_or(String::new(), |m| format!(" ({:.1} MB)", m.len() as f64 / 1e6));
                let note = self.saved_note();
                self.status(format!("Saved {}{size}{note}", path.display()), true);
                self.toast("Saved");
                self.doc.path = Some(path);
                true
            }
            Err(e) => {
                self.status(e, false);
                false
            }
        }
    }

    /// Writes every object to a native file, with the look, the view and the
    /// mirror settings; instances keep sharing a mesh. The file already there
    /// is only replaced once the new one is whole.
    pub(super) fn save_scene(&mut self, path: &Path) -> io::Result<()> {
        let mended = {
            let (meshes, objects, active) = self.scene_for_file()?;
            io::save_scene_with(&meshes, &objects, active, &self.details(), path)?
        };
        self.files.mended_on_save = mended;
        let now = self.history_mark();
        let restored = self.file().written(path, now);
        self.note_recent(path, true);
        // The work is in a file of the user's: the recovery copies of this
        // scene have had their day, and no others.
        self.recovery_saved(restored);
        Ok(())
    }

    /// The scene as a file holds it: each mesh once, the objects naming
    /// theirs by number (so instances keep sharing one), and the row of the
    /// active object. Only reads the scene; a recovery copy is written from
    /// the same (app/recovery.rs).
    pub(super) fn scene_for_file(&self) -> io::Result<(Vec<&Mesh>, Vec<io::SavedObject>, usize)> {
        let scene = &self.doc.scene;
        let mut ids: Vec<u64> = Vec::new();
        let mut objects = Vec::new();
        for o in &scene.objects {
            let mesh = ids.iter().position(|&id| id == o.mesh).unwrap_or_else(|| {
                ids.push(o.mesh);
                ids.len() - 1
            });
            objects.push(io::SavedObject {
                name: o.name.clone(),
                visible: o.visible,
                mesh,
                position: o.position,
                rotation: o.rotation,
                scale: o.scale,
            });
        }
        let meshes: Vec<&Mesh> = ids.iter().filter_map(|&id| scene.mesh(id, &self.doc.mesh)).collect();
        if meshes.len() != ids.len() {
            return Err("The scene is missing a mesh; nothing was saved.".into());
        }
        let active = scene.index_of(scene.active_id()).unwrap_or(0);
        Ok((meshes, objects, active))
    }

    /// What the file keeps besides the objects: see `io::Details`. A new
    /// part is one more `set` here (or in `seen`, if it is part of how the
    /// scene looks) and one more `get` in `apply_details` (or in `see`).
    pub(super) fn details(&self) -> io::Details {
        let mut details = self.seen();
        self.doc.scene.keep_mirrors(&mut details);
        self.doc.scene.keep_symmetry(&mut details);
        self.doc.tools.keep_in_file(&self.doc.scene, &mut details);
        self.doc.opts.paint.keep(&mut details);
        details
    }

    /// How the scene is seen, as a file keeps it: the shading, the look,
    /// whether the mirror guides show, and the view. A file keeps these with
    /// its objects, and so does the undo step of a scene put in place of
    /// another (`put_in_place`).
    pub(super) fn seen(&self) -> io::Details {
        let opts = &self.doc.opts;
        let matcap = self.ui.assets.matcaps.get(opts.matcap).map_or(String::new(), |m| format!("{}/{}", m.category, m.name));
        let mut details = io::Details::default();
        details.set(io::slot::SHADING, &io::Shading::of(opts, matcap));
        details.set(io::slot::LOOK, &opts.look);
        details.set(io::slot::CAMERA, &self.doc.camera.saved_view());
        // Whether the guides show is the scene's. The axes beside it are the
        // selected object's: every object's own symmetry is kept under the
        // object, and these are what a version of the app from before that
        // makes of the file.
        let selected = self.doc.scene.active_symmetry();
        let [x, y, z] = selected.mirror;
        details.set(io::slot::MIRROR, &io::Mirror { x, y, z, guides: opts.symmetry_guides, radial: selected.radial });
        details
    }

    /// Puts an opened file's look, mirror settings and view in place. A part
    /// the file does not have (it is from before that part was kept) leaves
    /// the app as it is; with no saved view, the model is framed instead.
    fn apply_details(&mut self, details: &io::Details) {
        self.doc.scene.restore_mirrors(details);
        // Each object's own symmetry; a file from before objects had one
        // gives its one setting to every object in it.
        self.doc.scene.restore_symmetry(details);
        // The file's objects are the scene's now, row for row: the tools get what they kept for them.
        let rows = Tools::rows_of(&self.doc.scene);
        self.doc.tools.restore_from_file(&rows, details, Arrival::Replaced);
        unity_import::restore_dynamic_topology(details, &mut self.doc.sculptor.dyntopo);
        // The palette the file's paint names its materials by; a file without one has none.
        self.doc.opts.paint = crate::mesh::paint::palette::ScenePaint::read(details).unwrap_or_default();
        self.see(details, None);
    }

    /// Puts a look and a view in place: an opened file's, or across an Open
    /// on Undo or Redo those of the scene come back to (`seen_stepped`). A
    /// part `details` does not have leaves the app as it is; with no view in
    /// it, the model is framed instead. `now` is what is in place already,
    /// where that is known: a part that is the same in both is left alone,
    /// so that a lighting picture is not read from the disk again for nothing.
    fn see(&mut self, details: &io::Details, now: Option<&io::Details>) {
        let changes = |slot: &str| now.is_none_or(|now| !now.same(details, slot));
        if let Some(shading) = details.get_over(io::slot::SHADING, &io::Shading::default()).filter(|_| changes(io::slot::SHADING)) {
            shading.apply(&mut self.doc.opts);
            // The picture is looked up by name: the list differs between
            // computers. One that is not here leaves the matcap alone.
            let matcaps = &self.ui.assets.matcaps;
            let found = matcaps
                .iter()
                .position(|m| format!("{}/{}", m.category, m.name) == shading.matcap)
                // A sculpt from the Unity app names the picture alone, without its folder.
                .or_else(|| matcaps.iter().position(|m| m.name.eq_ignore_ascii_case(&shading.matcap)));
            if let Some(index) = found {
                self.act(Action::Matcap(index));
                self.doc.opts.matcap_on = shading.matcap_on;
            }
        }
        if let Some(look) = details.get_over(io::slot::LOOK, &Look::default()).filter(|_| changes(io::slot::LOOK)) {
            self.set_look(look);
        }
        // The guides are the scene's; the axes kept beside them are the
        // objects' own (`apply_details`).
        if let Some(mirror) = details.get_over(io::slot::MIRROR, &io::Mirror::default()) {
            self.doc.opts.symmetry_guides = mirror.guides;
        }
        let view = details.get::<SavedView>(io::slot::CAMERA);
        if !view.is_some_and(|view| self.doc.camera.restore_view(&view)) {
            self.frame_view();
        }
        self.sync_view();
        self.redraw = true;
    }

    /// Takes on a saved look. The HDRI picture itself is not in the file,
    /// only where it was: if it is still there it is loaded again, and if
    /// not the settings are kept with a note where the picture should be.
    /// A place that is not on this computer's own drives is not looked at
    /// at all ([`saved_picture_is_there`]).
    fn set_look(&mut self, mut look: Look) {
        let (wanted, path) = (look.hdri.enabled, look.hdri.path.clone());
        look.wood.axis = look.wood.axis.clamp(0, 2);
        look.hdri.loaded = false;
        look.hdri.failed = false;
        look.hdri.status = "No HDRI loaded.".into();
        self.doc.opts.look = look;
        match path {
            Some(path) if saved_picture_is_there(&path, Path::is_file) => self.load_hdri(path),
            Some(path) => {
                // Not the picture the last scene was using: that would light
                // this one wrongly while the note says its own is missing.
                self.renderer.set_hdri(&self.gpu, None);
                let hdri = &mut self.doc.opts.look.hdri;
                hdri.status = format!("Saved HDRI not found: {}", file_name(&path));
                hdri.failed = true;
            }
            None => self.renderer.set_hdri(&self.gpu, None),
        }
        // Loading switches the HDRI on; the file says whether it was.
        self.doc.opts.look.hdri.enabled = wanted;
    }

    /// Puts `path` at the front of the recent files. `picture` takes a new
    /// picture for its tile (right after a save); otherwise one is only
    /// taken if the file has none yet.
    fn note_recent(&mut self, path: &Path, picture: bool) {
        self.ui.recent.add(path);
        if self.ui.recent.keeps_pictures() && (picture || !self.ui.recent.has_picture(path)) {
            let image = self.picture();
            self.ui.recent.keep_picture(path, &image);
        }
    }

    /// Brings a camera that already frames the scene's bounding box in as
    /// close as the models themselves allow. A box is a loose fit for
    /// anything rounded (a ball fills barely half the picture), and a tile is
    /// small, so the picture is fitted to where the surface really reaches:
    /// a sample of each mesh's vertices, measured along the camera's axes.
    fn fit_closely(&self, camera: &mut Camera, aspect: f32) {
        const SAMPLES: usize = 20_000;
        let turn = camera.rotation();
        let (right, up, toward) = (turn * Vec3::X, turn * Vec3::Y, turn * Vec3::Z);
        let scene = &self.doc.scene;
        // How far the surface reaches sideways, up and toward the eye.
        let mut reach = Vec3::ZERO;
        for o in scene.objects.iter().filter(|o| o.visible) {
            let Some(mesh) = scene.mesh(o.mesh, &self.doc.mesh) else { continue };
            let step = (mesh.vertex_count() / SAMPLES).clamp(1, usize::MAX);
            // The object and its live mirror copies.
            for (_, model) in o.placements() {
                for p in mesh.positions.iter().step_by(step) {
                    let from = model.transform_point3(*p) - camera.pivot;
                    reach = reach.max(Vec3::new(from.dot(right).abs(), from.dot(up).abs(), from.dot(toward)));
                }
            }
        }
        let tan = camera.tan_half_fov();
        // A little air around it; the sample may have missed a tip.
        let fitted = (reach.y / tan).max(reach.x / (tan * aspect)) * 1.12;
        // Seen in perspective, what is nearer the eye looks bigger.
        let fitted = if camera.ortho { fitted } else { fitted + reach.z };
        if fitted.is_finite() && fitted > 1e-5 {
            // Only ever closer than the box's frame, never farther.
            camera.distance = fitted.min(camera.distance);
        }
    }

    /// A picture of the scene for a recent-file tile: the model alone, from
    /// the angle it is being looked at, pulled back (or in) until all of it
    /// fits. No panels, brush ring, guides or tool overlays. Drawn on the
    /// side, at its own size; the window is not touched.
    fn picture(&mut self) -> image::RgbaImage {
        // Twice the tile's size, so scaling it down smooths it.
        let size = (recent::PICTURE.0 * 2, recent::PICTURE.1 * 2);
        let extent = Vec2::new(size.0 as f32, size.1 as f32);

        // The meshes as they are now, in case no frame has drawn them yet.
        let doc = &mut self.doc;
        self.renderer.sync_mesh(&self.gpu, doc.scene.active_mesh(), &mut doc.mesh, doc.mesh_replaced);
        let shown: Vec<u64> = doc.scene.objects.iter().map(|o| o.mesh).collect();
        for (id, mesh) in doc.scene.parked_mut() {
            if shown.contains(&id) {
                self.renderer.sync_mesh(&self.gpu, id, mesh, false);
            }
        }

        // A camera of its own at the live one's angle, framed on everything.
        let live = &self.doc.camera;
        let mut camera = Camera::default();
        (camera.yaw, camera.pitch, camera.fov_y, camera.ortho) = (live.yaw, live.pitch, live.fov_y, live.ortho);
        (camera.pivot, camera.distance) = (live.pivot, live.distance);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        camera.frame(bounds, extent.x / extent.y, false);
        self.fit_closely(&mut camera, extent.x / extent.y);
        let view = camera.view(Vec2::ZERO, extent, camera.distance, self.scene_size);

        // The frame's shader inputs are worked out from the app's view and
        // camera, so those are swapped for a moment.
        let live_camera = std::mem::replace(&mut self.doc.camera, camera);
        let live_view = std::mem::replace(&mut self.view, view);
        let mut globals = self.globals();
        let look = self.look_uniform();
        self.doc.camera = live_camera;
        self.view = live_view;
        globals.symmetry[..3].fill(0.0);

        let scene = &self.doc.scene;
        let items: Vec<DrawItem> = scene
            .objects
            .iter()
            .filter(|o| o.visible && !self.spheres_stand_in_for(o.id) && !self.tools_hide(o.id))
            .map(|o| {
                let mut model = o.matrix().to_cols_array_2d();
                model[3][3] = if o.id == scene.active_id() { 1.0 } else { 0.0 };
                DrawItem { mesh: o.mesh, model }
            })
            .collect();
        let items = self.tools_copies(items);
        let items = self.mirror_items(items);

        // Nothing over the model. The next frame puts each of these back.
        self.renderer.flash.show(&self.gpu, &globals.view_proj, &[]);
        self.renderer.spheres.hide();
        self.renderer.previews.show(&self.gpu, &view, &[]);
        let wireframe = std::mem::replace(&mut self.renderer.wire.enabled, false);

        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("recent picture"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target = texture.create_view(&Default::default());
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.draw(&self.gpu, &mut encoder, &target, size, &globals, &look, &self.doc.opts, &items);
        self.gpu.queue.submit([encoder.finish()]);
        self.renderer.wire.enabled = wireframe;
        self.redraw = true;
        crate::render::read_texture(&self.gpu, &texture)
    }

    // ---- opening, importing

    /// Open...: pick a `.sclp` (or a `.sculpt` from the Unity app), then say
    /// how it should come in.
    pub(super) fn open_action(&mut self) {
        let pick = rfd::FileDialog::new()
            .set_title("Open Sculpt")
            .add_filter("Sculpts", &["sclp", unity_import::EXTENSION])
            .add_filter("Sculpt", &["sclp"])
            .add_filter("Unity app sculpt", &[unity_import::EXTENSION])
            .pick_file();
        if let Some(path) = pick {
            self.open_chosen(path);
        }
    }

    /// A recent file's tile was clicked.
    pub(super) fn open_recent(&mut self, path: PathBuf) {
        self.open_chosen(path);
    }

    /// A `.sclp` has been chosen. In place of the scene or added to it are
    /// both sensible, so that is asked, with a warning if replacing would put
    /// unsaved work aside, or lose work that stands on the scene's objects
    /// and is in no file (unmerged tubes, planned keys). Only the untouched
    /// scene the app starts with, with nothing of that kind on it, is
    /// replaced without a question: there is nothing in it to keep.
    pub(super) fn open_chosen(&mut self, path: PathBuf) {
        if !path.is_file() {
            self.ui.recent.remove(&path);
            self.status(format!("{} is no longer there.", file_name(&path)), false);
            return;
        }
        let lost = self.scaffolds_at_risk(Loss::SceneReplaced);
        match opening_question(path.clone(), self.untouched(), self.unsaved(), lost) {
            Some(question) => self.ui.graph.ask(question),
            None => self.open_as(path, OpenHow::Replace),
        }
    }

    /// The answer to that question.
    pub(super) fn open_as(&mut self, path: PathBuf, how: OpenHow) {
        match how {
            OpenHow::Replace => self.load(path, true),
            OpenHow::Add => self.load(path, false),
            OpenHow::SaveThenReplace => {
                // Not saved (cancelled, or it failed): the scene stays.
                if self.save_action(false) {
                    self.load(path, true);
                }
            }
        }
    }

    /// Import...: a model file, or another sculpt whose objects are wanted.
    pub(super) fn import_action(&mut self) {
        let pick = rfd::FileDialog::new()
            .set_title("Import")
            .add_filter("Models and sculpts", &["obj", "stl", "sclp", unity_import::EXTENSION])
            .add_filter("Models", &["obj", "stl"])
            .add_filter("Sculpt", &["sclp"])
            .add_filter("Unity app sculpt", &[unity_import::EXTENSION])
            .pick_file();
        if let Some(path) = pick {
            self.load(path, false);
        }
    }

    /// Opens a `.sclp` in place of the scene (`native`), or adds a file to
    /// it: a model, or the objects of another `.sclp`. Either is one undo
    /// step. Nothing is asked here: the interface asks first (`open_chosen`),
    /// and the Claude bridge and the command line have already decided.
    pub(super) fn load(&mut self, path: PathBuf, native: bool) {
        if native {
            self.open_replacing(path);
        } else if is_native(&path) {
            self.add_scene(path);
        } else {
            self.add_model(path);
        }
    }

    /// The file takes the scene's place and becomes the document: its look,
    /// mirror settings and view come with it, and Save writes back to it.
    fn open_replacing(&mut self, path: PathBuf) {
        let (count, details) = match self.replace_scene(&path) {
            Ok(opened) => opened,
            Err(e) => return self.status(e, false),
        };
        // A sculpt from the Unity app says so, and what it could not bring.
        let note = unity_import::status_note(&details, true).map_or(String::new(), |note| format!(". {note}"));
        let note = note + &self.opened_note();
        self.status(format!("Opened {} ({}){note}", path.display(), how_many(count)), true);
        // Its picture is the one from its last save; one is only taken now
        // if it has none.
        self.note_recent(&path, false);
    }

    /// What there is to say about damage found in the file that was last
    /// put in place of the scene (an Open, a restored recovery copy): the
    /// points that were mended, the triangles that were left out, and the
    /// subdivision levels or sculpt layers that could not be read. `None`
    /// for a sound file.
    pub(super) fn opened_damage(&self) -> Option<String> {
        let files = &self.files;
        damage_notes(files.repaired_on_open, files.left_out_on_open, files.lost_on_open)
    }

    /// [`Running::opened_damage`] as a sentence to follow what a status line
    /// says; empty for a sound file.
    fn opened_note(&self) -> String {
        self.opened_damage().map_or(String::new(), |note| format!(". {note}"))
    }

    /// What there is to add to a status line about the last save: that
    /// damaged points were written mended. Empty when none were.
    pub(super) fn saved_note(&self) -> String {
        io::note_saved(self.files.mended_on_save).map_or(String::new(), |note| format!(". {note}"))
    }

    /// Opens `path` and puts its scene in place of the one that is open: the
    /// file becomes the scene's own, with nothing unsaved. Nothing has been
    /// touched if the file cannot be read.
    fn replace_scene(&mut self, path: &Path) -> io::Result<(usize, io::Details)> {
        let saved = io::load_scene(path)?;
        Ok(self.put_in_place(saved, FileState::opened(path.to_owned())))
    }

    /// Puts `saved` in place of the scene that is open, as one undo step,
    /// with whatever look, mirror settings and view it holds. `file` is what
    /// the new scene brings: the file that becomes its own, with nothing
    /// unsaved (an Open, `FileState::opened`); no file, all of it unsaved,
    /// and a note of the recovery copy it came from (`FileState::restored`);
    /// or no file and nothing to save (New Scene, `FileState::new_scene`).
    /// This is the one way a whole scene arrives, from a file or from
    /// nowhere, so each treats the scene it replaces alike: the step keeps
    /// that scene's file, whether it was saved and which recovery copy it was
    /// restored from, and Undo gives them back (`file_undo.rs`); the tools
    /// are told their objects went (`apply_details`); and unsaved work in
    /// that scene that is in no recovery copy yet gets one first
    /// (`recovery_before_replacing`). Gives back how many objects came, and
    /// what else `saved` held.
    pub(super) fn put_in_place(&mut self, saved: io::SavedScene, file: FileState) -> (usize, io::Details) {
        // Unsaved work is about to be one Undo away and nowhere else.
        self.recovery_before_replacing();
        let count = saved.objects.len();
        (self.files.repaired_on_open, self.files.left_out_on_open) = (saved.repaired, saved.left_out);
        self.files.lost_on_open = saved.lost;
        let before = self.history_mark();
        let before = self.file().state(before);
        let seen_before = self.seen();
        let mut details = io::Details::default();
        self.edit_scene(|scene, slot| {
            details = install(scene, slot, saved);
            true
        });
        // Straight away: it is the step just recorded that keeps them.
        self.doc.history.keep_file(before);
        self.doc.history.keep_seen(seen_before);
        self.doc.mesh_replaced = true;
        self.sync_view();
        if file.restored.is_some() {
            // Where a scripted check of recovery may ask for a fault.
            self.restore_fault_point();
        }
        self.apply_details(&details);
        let now = self.history_mark();
        self.file().became(file, now);
        (count, details)
    }

    /// The file's objects join the scene where they were saved. Nothing else
    /// of the file is used: the look, the view and the document stay the
    /// scene's own, as in the Unity app.
    fn add_scene(&mut self, path: PathBuf) {
        let saved = match io::load_scene(&path) {
            Ok(saved) => saved,
            Err(e) => return self.status(e, false),
        };
        let count = saved.objects.len();
        let note = unity_import::status_note(&saved.details, false).map_or(String::new(), |note| format!(". {note}"));
        let damage = damage_notes(saved.repaired, saved.left_out, saved.lost);
        let note = note + &damage.map_or(String::new(), |note| format!(". {note}"));
        let (details, mut rows) = (saved.details.clone(), Vec::new());
        self.edit_scene(|scene, slot| {
            rows = append(scene, slot, saved);
            true
        });
        // Only the objects are added: their arrays come with them, and the scene's own are not touched.
        self.doc.tools.restore_from_file(&rows, &details, Arrival::Added);
        self.doc.mesh_replaced = true;
        self.frame_view();
        self.status(format!("Added {} from {}{note}", how_many(count), file_name(&path)), true);
    }

    fn add_model(&mut self, path: PathBuf) {
        // An OBJ is fitted to the size of the scene's first object (as Unity does).
        let first = self.doc.scene.objects.first().and_then(|o| self.doc.scene.mesh(o.mesh, &self.doc.mesh));
        let first_half_size = first.map(|m| m.bounds().size() * 0.5);
        match import_mesh(&path, first_half_size) {
            Ok((mesh, at)) => {
                let (tris, verts) = (mesh.triangle_count(), mesh.vertex_count());
                let size = mesh.bounds().size().to_array().map(|s| sculpt_geom::units::format(s, sculpt_geom::LengthUnit::Millimetres, false));
                let name = stem(&path);
                self.edit_scene(|scene, slot| {
                    scene.add(&name, mesh, slot);
                    scene.active_mut().position = at;
                    true
                });
                self.frame_view();
                let message = format!(
                    "Added {} ({tris} triangles, {verts} vertices, {} x {} x {} mm)",
                    path.display(),
                    size[0],
                    size[1],
                    size[2]
                );
                // Models from elsewhere, scans above all, are often not closed.
                self.status_checked(message, Cure::Remesh);
            }
            Err(e) => self.status(e, false),
        }
    }

    // ---- visual checks

    /// Brings up something of this file's to look at with `--screenshot`:
    /// `recent` fills the File card's recent tiles, `ask-exit`, `ask-open`
    /// and `ask-new` put up the question the close button, choosing a file,
    /// or New Scene would ask of the scene as the other `--demo` options left
    /// it (`ask-open-clean`: of a scene with nothing to lose), `play` runs
    /// `files_play` first, `lost` the check of what closing and opening would
    /// lose (app/unsaved/play.rs), `open-undo` the check of Undo across an
    /// Open (app/file_undo/play.rs), and `new` the check of New Scene and
    /// Exit (app/new_scene/play.rs). `ask-exit:mirror` makes a change first
    /// that a file keeps and no undo step records: `mirror`, `hide` or
    /// `local` (`Running::kept_change`); `ask-exit:saved` takes the scene as
    /// the other options left it for saved, so that only what is in no file
    /// is left to ask about. Everything it writes goes in a scratch folder
    /// of the run's own (`prefs::scratch`).
    pub(super) fn files_demo(&mut self, what: &str) {
        let folder = prefs::scratch("sculpt_files_demo");
        let _ = std::fs::remove_dir_all(&folder);
        if let Err(e) = std::fs::create_dir_all(&folder) {
            log::error!("{e}");
            return;
        }
        // The pictures too: never beside the user's own.
        self.ui.recent.restore(Vec::new(), Some(folder.join("thumbnails")));
        match what {
            "play" => self.files_play(&folder),
            // The same for a sculpt from the Unity app named on the command line (app/unity.rs).
            "unity" => self.unity_play(&folder),
            // Arrays the Array tool holds, through a save, an open, an undo and an add (app/file_arrays.rs).
            "array" => self.arrays_play(&folder),
            // What Undo and Redo of an Open do to the scene's file (app/file_undo.rs).
            "open-undo" | "open-undo:opened" | "open-undo:undone" => self.open_undo_play(&folder, what),
            // What closing and opening would lose, asked the way a person would (app/unsaved/play.rs).
            "lost" => self.lost_play(&folder),
            // New Scene and the Exit button, through the routines the buttons run (app/new_scene/play.rs).
            "new" | "new:fresh" | "new:undone" => self.new_scene_play(&folder, what),
            "ask-open-clean" => {
                let path = folder.join("dragon head.sclp");
                self.ui.graph.ask(Question::Open { path, unsaved: false, lost: Vec::new() });
            }
            _ if ["ask-exit", "ask-open", "ask-new"].iter().any(|ask| what.starts_with(ask)) => {
                let (ask, change) = what.split_once(':').map_or((what, None), |(ask, change)| (ask, Some(change)));
                match change {
                    Some("saved") => self.mark_saved(),
                    Some(change) => self.kept_change(change),
                    None => {}
                }
                self.demo_question(&folder, ask);
            }
            _ => {
                // The Objects card shut, as if it had been left that way, so
                // the File card under it is in view.
                let shut = std::collections::BTreeMap::from([("Objects".to_owned(), false)]);
                prefs::cards::restore(&self.egui_ctx, &shut);
                // The same sculpt saved from four sides, under four names.
                let names = ["base mesh", "crab claw", "dragon head", "lure v3"];
                for (i, name) in names.into_iter().enumerate() {
                    self.doc.camera.yaw = 25.0 + 90.0 * i as f32;
                    self.sync_view();
                    let path = folder.join(format!("{name}.sclp"));
                    match self.save_scene(&path) {
                        Ok(()) => self.doc.path = Some(path),
                        Err(e) => log::error!("{e}"),
                    }
                }
                self.doc.camera.yaw = 25.0;
                self.sync_view();
                self.status("Saved lure v3.sclp", true);
            }
        }
    }

    /// Puts up the question the close button would ask of the scene as it
    /// stands (`ask-exit`), or the one New Scene would (`ask-new`), or the
    /// one choosing a file would (`ask-open`): the app's own decision, so
    /// the picture is of what a person would see. Says so if there would be
    /// no question at all.
    fn demo_question(&mut self, folder: &Path, ask: &str) {
        if ask == "ask-exit" {
            match self.question_before_closing() {
                Some(question) => self.ui.graph.ask(question),
                None => println!("ask-exit: nothing would be lost, so the app would close without asking"),
            }
            return;
        }
        if ask == "ask-new" {
            self.new_scene_action();
            if !self.ui.graph.asking() {
                println!("ask-new: nothing would be lost, so the new scene was started without a question");
            }
            return;
        }
        let path = folder.join("dragon head.sclp");
        if let Err(e) = self.write_copy(&path) {
            return log::error!("{e}");
        }
        let objects = self.doc.scene.objects.len();
        self.open_chosen(path);
        if !self.ui.graph.asking() {
            let opened = self.doc.scene.objects.len();
            println!("ask-open: nothing to keep, so the file opened without a question ({objects} -> {opened} objects)");
        }
    }

    /// For a scripted check: presses Exit as a person's run would (a
    /// `--screenshot` run never asks), then takes the press back, so the app
    /// stays open. Gives whether the app would have closed at once, and the
    /// question that was put up if it asked instead.
    pub(super) fn exit_pressed(&mut self) -> (bool, Option<Question>) {
        let shot = self.args.screenshot.take();
        self.exit_action();
        self.args.screenshot = shot;
        let closing = std::mem::take(&mut self.files.quit);
        let asked = self.ui.graph.question().cloned();
        self.ui.graph.answer(false);
        (closing, asked)
    }

    /// Writes the scene to `path` as a save would, and leaves what the scene
    /// knows of its own file alone: a file for a scripted check to open.
    pub(super) fn write_copy(&self, path: &Path) -> io::Result<()> {
        let (meshes, objects, active) = self.scene_for_file()?;
        io::save_scene_with(&meshes, &objects, active, &self.details(), path).map(|_| ())
    }

    /// Works the file code the way a person would, in the real app, and
    /// prints what came of each step: a check that needs no one at the
    /// tablet. Run it with the demo strokes (`--demo --demo-files play`).
    fn files_play(&mut self, folder: &Path) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let (first, second) = (folder.join("first.sclp"), folder.join("second.sclp"));
        let strokes = self.doc.history.steps().0;
        say("demo strokes are unsaved work", strokes > 0 && self.unsaved() && !self.untouched());

        // Save: clean, in the recent files, with a picture.
        let look = &mut self.doc.opts.look;
        look.finish = crate::look::Finish::Metal;
        look.metal.preset = 2;
        look.lighting = 6;
        look.depth_of_field = true;
        look.focus_distance = 3.5;
        self.doc.opts.matcap_on = false;
        self.doc.opts.base_color = [0.8, 0.3, 0.1];
        let radial = crate::sculpt::Radial { on: true, count: 6, axis: crate::sculpt::RadialAxis::Z, custom_axis: Vec3::X };
        let kept = crate::scene::Symmetry { mirror: [true, false, true], radial, space: crate::scene::Space::World };
        self.doc.edit_symmetry(|symmetry| *symmetry = kept);
        (self.doc.camera.yaw, self.doc.camera.pitch, self.doc.camera.ortho) = (140.0, -20.0, true);
        let saved_view = self.doc.camera.saved_view();
        let saved = self.save_scene(&first);
        self.doc.path = Some(first.clone());
        say("save writes the file", saved.is_ok() && first.is_file());
        say("a saved scene is not unsaved", !self.unsaved());
        say("the save is the newest recent file", self.ui.recent.paths().first().is_some_and(|p| p.ends_with("first.sclp")));
        say("and has a picture", self.ui.recent.has_picture(&first));
        say("nothing is left beside it", !folder.join("first.sclp.tmp").exists());

        // Undo makes it unsaved, redo clean again; a new stroke unsaved.
        self.act(Action::Undo);
        say("undo after a save is unsaved work", self.unsaved());
        self.act(Action::Redo);
        say("redo back to the save is clean", !self.unsaved());
        self.act(Action::New(crate::mesh::primitives::Primitive::Cube));
        say("adding a shape is unsaved work", self.unsaved());
        let count = self.doc.scene.objects.len();
        let second_saved = self.save_scene(&second);
        say("a second file saves", second_saved.is_ok() && !self.unsaved());

        // Change everything the file keeps, then open the first again.
        let look = &mut self.doc.opts.look;
        look.finish = crate::look::Finish::None;
        look.lighting = 0;
        look.depth_of_field = false;
        self.doc.opts.matcap_on = true;
        self.doc.edit_symmetry(|symmetry| *symmetry = Default::default());
        (self.doc.camera.yaw, self.doc.camera.ortho) = (10.0, false);
        self.load(first.clone(), true);
        let opts = &self.doc.opts;
        say("opening puts back the object list", self.doc.scene.objects.len() == count - 1);
        say(
            "and the finish, lighting and depth of field",
            opts.look.finish == crate::look::Finish::Metal
                && opts.look.metal.preset == 2
                && opts.look.lighting == 6
                && opts.look.depth_of_field
                && opts.look.focus_distance == 3.5,
        );
        say("and the material", !opts.matcap_on && opts.base_color == [0.8, 0.3, 0.1]);
        let opened = self.doc.scene.active_symmetry();
        say("and the selected object's mirror axes", opened.mirror == [true, false, true]);
        say("and its radial symmetry, fixed in the world as it was", (opened.radial, opened.space) == (radial, kept.space));
        say("and the view", self.doc.camera.saved_view() == saved_view);
        say("the opened file is the document, and clean", self.doc.path.as_deref() == Some(first.as_path()) && !self.unsaved());
        say("it is the newest recent file again", self.ui.recent.paths().first().is_some_and(|p| p.ends_with("first.sclp")));
        self.act(Action::Undo);
        say("undo brings the scene before it back", self.doc.scene.objects.len() == count);
        self.act(Action::Redo);

        // Add: the second file's objects join, nothing else changes.
        let before = self.doc.scene.objects.len();
        self.load(second.clone(), false);
        let names: Vec<&str> = self.doc.scene.objects.iter().map(|o| o.name.as_str()).collect();
        let mut distinct = names.clone();
        distinct.sort_unstable();
        distinct.dedup();
        say("adding a sculpt brings its objects in", names.len() == before + count);
        say("under names of their own", distinct.len() == names.len());
        // The second file held the ball as it was saved the first time and a new cube.
        let scene = &self.doc.scene;
        let added: Vec<_> = scene.objects[before.min(names.len())..].iter().map(|o| scene.symmetry(o.id)).collect();
        say("each with the symmetry it was saved with", added == [kept, Default::default()]);
        say("the document is still the first file", self.doc.path.as_deref() == Some(first.as_path()));
        say("and it now has unsaved work", self.unsaved());
        say("its look stayed", self.doc.opts.look.finish == crate::look::Finish::Metal);

        // The questions.
        self.ui.graph.answer(false);
        self.open_chosen(second.clone());
        say("opening over unsaved work asks", self.ui.graph.asking() && self.doc.path.as_deref() == Some(first.as_path()));
        self.ui.graph.answer(false);
        say("Esc leaves the scene alone", !self.ui.graph.asking() && self.doc.scene.objects.len() == before + count);
        let gone = folder.join("gone.sclp");
        self.ui.recent.add(&gone);
        self.open_recent(gone.clone());
        say("a recent file that has gone is dropped, not opened", !self.ui.recent.paths().contains(&gone) && !self.ui.graph.asking());
        say("closing would ask", self.unsaved() && !self.quitting());
        self.quit(false);
        say("Exit Without Saving closes", self.quitting());
        self.files.quit = false;

        // The settings: brush tuning and the rest survive a write and a read.
        let home = Home::at(&folder.join("settings"));
        self.doc.opts.ui_scale = 1.25;
        self.doc.sculptor.settings.radius_px = 77.0;
        self.doc.sculptor.settings.per_brush[crate::sculpt::Brush::Crease.index()].strength = 0.66;
        let written = self.prefs_now();
        let wrote = home.save(&written);
        say("the settings file writes", wrote.is_ok() && home.load() == written);
        self.doc.opts.ui_scale = 1.0;
        self.doc.sculptor.settings.radius_px = 20.0;
        let read = home.load();
        self.apply_prefs(&read);
        say(
            "and reading it puts the settings back",
            self.doc.opts.ui_scale == 1.25
                && self.doc.sculptor.settings.radius_px == 77.0
                && self.doc.sculptor.settings.per_brush[crate::sculpt::Brush::Crease.index()].strength == 0.66
                && self.ui.recent.paths() == written.recent,
        );
        self.ui.recent.restore(written.recent, Some(folder.join("thumbnails")));
        self.doc.opts.ui_scale = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.sclp` may name its HDRI on a network share, and whoever opens the
    /// file (the Claude bridge among them) must not be made to reach out to it.
    #[test]
    fn a_saved_hdri_on_a_network_share_is_not_looked_for() {
        let looked = std::cell::Cell::new(0);
        let there = |_: &Path| {
            looked.set(looked.get() + 1);
            true
        };
        let network = [
            r"\\server\share\sky.hdr",
            "//server/share/sky.hdr",
            r"\\?\UNC\server\share\sky.hdr",
            r"\\?\C:\Pictures\sky.hdr",
            r"\\.\COM1",
            r"C:\Pictures\NUL.hdr",
            r"C:\Pictures\sky.hdr:hidden",
        ];
        for path in network {
            assert!(!saved_picture_is_there(Path::new(path), there), "{path} counts as not there");
        }
        assert_eq!(looked.get(), 0, "and the disk, or the network, was never asked about any of them");
        // A picture on one of this computer's own drives is looked for as before.
        assert!(saved_picture_is_there(Path::new(r"C:\Pictures\sky.hdr"), there));
        assert!(!saved_picture_is_there(Path::new(r"C:\Pictures\gone.hdr"), |_| false), "and may be missing");
        assert_eq!(looked.get(), 1);
        // The real look at the disk, for a file that is there and one that is not.
        let folder = std::env::temp_dir().join("sculpt_saved_picture");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("sky.hdr"), b"a picture").unwrap();
        assert!(saved_picture_is_there(&folder.join("sky.hdr"), Path::is_file));
        assert!(!saved_picture_is_there(&folder.join("no such.hdr"), Path::is_file));
    }

    /// Where the Exit button meets the recovery copies: the app is told it
    /// may close in three ways, and only two of them are the user's word.
    #[test]
    fn a_close_over_unsaved_work_is_settled_only_when_the_question_was_answered() {
        // Nothing unsaved: there was nothing to ask, however it closes.
        assert!(settled(false, false, false) && settled(true, false, false) && settled(true, true, false));
        // Unsaved, and the question answered with a way to close.
        assert!(settled(true, false, true));
        // Unsaved, and closed without the question: the bridge's quit, which
        // tells nobody; a scripted run, which is told and never asked.
        assert!(!settled(false, false, true), "this run's recovery copies must stay");
        assert!(!settled(true, true, true), "this run's recovery copies must stay");
    }

    #[test]
    fn a_file_that_will_not_open_at_start_up_is_named_and_explained() {
        let folder = std::env::temp_dir().join("sculpt_start_up_failure");
        std::fs::create_dir_all(&folder).unwrap();

        let missing = folder.join("not_here.sclp");
        let _ = std::fs::remove_file(&missing);
        let said = start_up_failure(&missing, &io::load_scene(&missing).err().unwrap());
        assert!(said.starts_with("not_here.sclp is not there"), "{said}");
        assert!(!said.contains("os error"), "plain words, not an error code: {said}");

        let broken = folder.join("broken.sclp");
        std::fs::write(&broken, b"SCLP0004 and then nothing a scene could be made of").unwrap();
        let said = start_up_failure(&broken, &io::load_scene(&broken).err().unwrap());
        assert!(said.contains("broken.sclp") && said.contains("not opened"), "{said}");
        assert!(said.ends_with("Starting with a new sphere instead."), "{said}");
        assert!(!said.contains(".."), "{said}");
    }
}
