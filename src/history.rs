//! Undo / redo. Strokes are stored as sparse per-vertex deltas (a few
//! kilobytes for a typical stroke); operations that replace the whole mesh
//! (remesh, import, new shape) keep the previous mesh itself, so stepping
//! across them is instant. Edits to the object list (add, delete, rename,
//! reorder...) keep the list as it was. A step that put one scene in place
//! of another (an Open, a restored recovery copy, a New Scene) also keeps
//! which file the scene on its other side belongs to (`FileState`), so that
//! the file comes back with the objects.
//!
//! Mesh steps remember which mesh they belong to, so undo can bring that
//! mesh back to hand before stepping.
//!
//! An object-list step holds no mesh. The mesh of an object that was deleted,
//! joined into another or opened over stays parked in the scene, which owns
//! every mesh, for as long as a step's list still shows it (`keeps_mesh`).
//! Those meshes are most of what undo costs, so the scene tells the history
//! how big they are and they count against the cap like the steps themselves
//! (`settle`): each once, however many steps or instances show it, and not at
//! all while an object of the scene still shows it.
//!
//! What the memory cap lets go (`over_bytes`). Steps go oldest first, so a
//! parked mesh is freed with the newest step that shows it, and a step can
//! only go with every step before it.
//! - Normally the fewest of the oldest steps go that bring undo under the
//!   cap, and never one of the last three (`RECENT`): a big step is not
//!   thrown away by the first stroke made after it.
//! - When the last three alone are over the cap nothing gets under it. Older
//!   steps that keep a whole mesh still go, since that is real memory. A
//!   stroke older than the last three stays unless such a step is newer than
//!   it: dropping strokes would free next to nothing.
//! - Twice the cap is the ceiling, and the last three give way to it, oldest
//!   first. Only the newest step is kept whatever it holds (the last thing
//!   done can always be undone); when it is over the cap alone, the steps
//!   before it may hold the cap again.
//!
//! So undo holds more than the cap only while its last three steps make it
//! so, and never more than twice the cap, or than the cap beside its newest
//! step when that alone is over the cap.
//!
//! Recording a step cannot reach the scene, so when a step that goes was the
//! last to show a parked mesh the history says a settle is owed
//! (`owes_settle`), and the app runs it on the next frame.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::io::Details;
use crate::mesh::groups::GroupsStep;
use crate::mesh::layers::LayersStep;
use crate::mesh::paint::PaintStep;
use crate::mesh::levels::MeshLevels;
use crate::mesh::{Mesh, StrokeDelta, VisibilityDelta};
use crate::scene::{Object, Scene, SceneState};

mod cap;

/// The newest steps the memory cap leaves alone until undo holds twice the cap.
const RECENT: usize = 3;

pub enum Edit {
    Stroke(StrokeDelta),
    /// Faces of the mesh hidden, shown again or swapped (see mesh/visibility.rs).
    Visibility(VisibilityDelta),
    /// The mesh that was current on the other side of this step.
    Replace(Box<Mesh>),
    /// The mesh's subdivision levels on the other side of this step: levels
    /// were deleted and the mesh itself is the same (see mesh/levels.rs). A
    /// change of level, which is another mesh, is a `Replace`.
    Levels(Option<Box<MeshLevels>>),
    /// The mesh's sculpt layers on the other side of this step, and its
    /// positions where the two sides differ: a layer made, renamed, turned
    /// up or down, switched, deleted or baked (see mesh/layers.rs). A stroke
    /// on a layer is a `Stroke`.
    Layers(Box<LayersStep>),
    /// The mesh's face groups on the other side of this step, with what the
    /// same command did to the mask or to the hidden faces (see mesh/groups.rs).
    Groups(Box<GroupsStep>),
    /// The mesh's paint layers on the other side of this step: a layer made,
    /// deleted, merged, filled, cleared or changed (see mesh/paint.rs). A
    /// paint stroke is a `Stroke`.
    #[allow(dead_code)] // recorded by the Paint Layers card
    Paint(Box<PaintStep>),
    /// The object list on the other side of this step.
    Scene(Box<SceneState>),
}

/// The file of the scene on the other side of a step that changed which
/// file the scene belongs to. Undo of an Open brings back the objects that
/// were there before it; without this the name of the opened file would stay
/// behind, and Save would write the old scene over the file just opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileState {
    /// That scene's file; `None` if it has none.
    pub path: Option<std::path::PathBuf>,
    /// Whether that scene has nothing to save: it is what its file holds,
    /// or it is a new scene that nothing has been done to yet.
    pub saved: bool,
    /// The recovery copy that scene was restored from, if it was and has not
    /// been saved since.
    pub restored: Option<Restored>,
}

/// Where a scene that was restored from a recovery copy came from. It stays
/// with that scene through Undo and Redo, as its file does: only a save of
/// that very scene may retire the copies it came from (app/recovery.rs), and
/// Undo of the restore puts another scene on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    /// The folder of the run that left the copy.
    pub run: std::path::PathBuf,
    /// The file the copy was a copy of, for Save to offer.
    pub document: Option<std::path::PathBuf>,
}

impl FileState {
    /// A scene opened from `path`: it is what that file holds.
    pub fn opened(path: std::path::PathBuf) -> FileState {
        FileState { path: Some(path), saved: true, restored: None }
    }

    /// A scene put back from a recovery copy, `from` saying which: it has no
    /// file, and all of it is unsaved work until the user says where it
    /// should go.
    pub fn restored(from: Restored) -> FileState {
        FileState { path: None, saved: false, restored: Some(from) }
    }

    /// A scene just begun (New Scene). It has no file, so Save asks where,
    /// and nothing in it is unsaved: there is nothing in it to keep yet. It
    /// is nobody's restored copy either, whatever scene it took the place of.
    pub fn new_scene() -> FileState {
        FileState { path: None, saved: true, restored: None }
    }
}

struct Step {
    edit: Edit,
    /// The mesh a stroke or replace applies to.
    mesh: u64,
    /// Only on a step that changed which file the scene belongs to: the file
    /// on its other side (see `keep_file`).
    file: Option<FileState>,
    /// Only on a step that put another scene in place of the one that was
    /// open: how the scene on its other side was seen (see `keep_seen`).
    seen: Option<Box<Details>>,
}

/// What the next undo or redo step will touch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Next {
    Mesh(u64),
    Scene,
}

impl Edit {
    /// What the step itself holds.
    fn bytes(&self) -> usize {
        match self {
            Edit::Stroke(d) => d.bytes(),
            Edit::Visibility(d) => d.bytes(),
            Edit::Replace(m) => m.memory_bytes(),
            Edit::Levels(levels) => levels.as_ref().map_or(0, |levels| levels.bytes()),
            Edit::Layers(step) => step.bytes(),
            Edit::Groups(step) => step.bytes(),
            Edit::Paint(step) => step.bytes(),
            // The list alone. Its meshes are the scene's: `History::bytes`
            // adds the ones that are kept only for undo.
            Edit::Scene(s) => s.objects.len() * std::mem::size_of::<Object>(),
        }
    }

    /// The objects an object-list step can bring back; none for a mesh step.
    fn listed(&self) -> &[Object] {
        match self {
            Edit::Scene(state) => &state.objects,
            _ => &[],
        }
    }
}

pub struct History {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    /// Id of the mesh being edited; new stroke and replace steps belong to it.
    pub target: u64,
    /// The most steps undo keeps, and the memory it may hold with them (see
    /// `bytes`, and the module header for what goes when it holds more). The
    /// oldest steps go first; the newest always stays.
    pub max_steps: usize,
    pub max_bytes: usize,
    /// The meshes no object of the scene shows, with their sizes, as the
    /// scene last listed them (`settle`). One counts only while a step shows it.
    spare: HashMap<u64, usize>,
    /// Whether a step has gone that was the last to show one of those meshes:
    /// the scene still holds it, and will until `settle`.
    owed: bool,
    /// Steps the memory cap has dropped that `take_let_go` has yet to report.
    let_go: usize,
    /// The edit (a value of `edits`) at which the cap last dropped steps, and
    /// whether that was left out of `let_go` as more of a run already counted.
    let_go_at: Option<(u64, bool)>,
    /// How many steps have ever been recorded (or thrown away by `clear`).
    /// With `steps`, it tells whether anything changed since the scene was
    /// last saved: see `Running::unsaved` in app/files.rs.
    pub edits: u64,
}

impl Default for History {
    fn default() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            target: 0,
            max_steps: 100,
            max_bytes: cap::default_bytes(),
            spare: HashMap::new(),
            owed: false,
            let_go: 0,
            let_go_at: None,
            edits: 0,
        }
    }
}

impl History {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn steps(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    /// The memory undo and redo hold: the steps themselves, and every mesh
    /// that is still in memory only because one of them could bring it back.
    pub fn bytes(&self) -> usize {
        let steps: usize = self.undo.iter().chain(&self.redo).map(|s| s.edit.bytes()).sum();
        steps + self.spare_bytes()
    }

    /// The meshes the kept object lists show, once for each object.
    fn listed_meshes(&self) -> impl Iterator<Item = u64> + '_ {
        self.undo.iter().chain(&self.redo).flat_map(|s| s.edit.listed()).map(|o| o.mesh)
    }

    /// Bytes of the spare meshes a step still shows. A mesh is counted once
    /// however many steps and objects show it, and for as long as one does.
    fn spare_bytes(&self) -> usize {
        if self.spare.is_empty() {
            return 0;
        }
        let kept: HashSet<u64> = self.listed_meshes().filter(|mesh| self.spare.contains_key(mesh)).collect();
        kept.iter().map(|mesh| self.spare[mesh]).sum()
    }

    /// For each undo step, oldest first: what undo and redo would hold with
    /// every step before it gone, and whether the step keeps a whole mesh in
    /// memory (a replaced one, or a spare one no newer step shows).
    fn loads(&self) -> Vec<(usize, bool)> {
        let mut counted = HashSet::new();
        let mut spare = |step: &Step| -> usize {
            let meshes = step.edit.listed().iter().map(|o| o.mesh);
            meshes.filter_map(|mesh| self.spare.get(&mesh).filter(|_| counted.insert(mesh))).sum()
        };
        let mut held: usize = self.redo.iter().map(|step| step.edit.bytes() + spare(step)).sum();
        let newest_first = self.undo.iter().rev().map(|step| {
            let kept = spare(step);
            held += step.edit.bytes() + kept;
            (held, kept > 0 || matches!(step.edit, Edit::Replace(_)))
        });
        let mut loads: Vec<(usize, bool)> = newest_first.collect();
        loads.reverse();
        loads
    }

    /// How many of the oldest steps the memory cap lets go: the rules in the
    /// module header. Never the newest.
    fn over_bytes(&self) -> usize {
        let loads = self.loads();
        let Some(newest) = loads.len().checked_sub(1) else { return 0 };
        let cap = self.max_bytes;
        // The fewest steps that bring undo under `limit`, of the `most` oldest.
        let fewest = |limit: usize, most: usize| (0..=most).find(|&n| loads[n].0 <= limit);
        let older = loads.len().saturating_sub(RECENT);
        if let Some(steps) = fewest(cap, older) {
            return steps;
        }
        // The last few are over the cap alone. Of the older ones, as far as
        // the newest that keeps a mesh; and whatever the ceiling asks for.
        let meshes = loads[..older].iter().rposition(|&(_, mesh)| mesh).map_or(0, |last| last + 1);
        let ceiling = cap.saturating_add(cap.max(loads[newest].0));
        meshes.max(fewest(ceiling, newest).unwrap_or(newest))
    }

    /// Drops the oldest steps while there are too many of them, then those
    /// the memory cap lets go.
    fn trim(&mut self) {
        let extra = self.undo.len().saturating_sub(self.max_steps.max(1));
        self.drop_oldest(extra);
        let heavy = self.over_bytes();
        self.drop_oldest(heavy);
        if heavy > 0 {
            // Edit after edit that each push a step out is one event, told
            // once. An edit recorded and then settled is one edit, so what
            // its settling drops is told, or not, with what recording it did.
            let edit = self.edits;
            let quiet = self.let_go_at.is_some_and(|(at, quiet)| at + 1 == edit || (at == edit && quiet));
            if !quiet {
                self.let_go += heavy;
            }
            self.let_go_at = Some((edit, quiet));
        }
    }

    /// Drops the `count` oldest steps.
    fn drop_oldest(&mut self, count: usize) {
        let gone: Vec<Step> = self.undo.drain(..count).collect();
        self.gone(&gone);
    }

    /// Call with steps that have just left the history. If one of them was
    /// the last to show a spare mesh, the scene is holding that mesh for
    /// nothing, and a settle is owed.
    fn gone(&mut self, steps: &[Step]) {
        let shown = steps.iter().flat_map(|s| s.edit.listed()).map(|o| o.mesh);
        let spare: Vec<u64> = shown.filter(|mesh| self.spare.contains_key(mesh)).collect();
        self.owed |= spare.iter().any(|&mesh| !self.keeps_mesh(mesh));
    }

    /// Whether the scene holds a mesh that no step shows any more. Recording
    /// a step can leave one (its own trimming, or the end of redo) with no
    /// scene at hand to free it: the app asks this once a frame and settles.
    /// It costs nothing to ask, and an ordinary stroke never owes one.
    pub fn owes_settle(&self) -> bool {
        self.owed
    }

    /// How many steps the memory cap has dropped that the user has not been
    /// told about, if any. A run of edits that each drop steps counts as its
    /// first: the app says it once, not at every stroke.
    pub fn take_let_go(&mut self) -> Option<usize> {
        let steps = std::mem::take(&mut self.let_go);
        (steps > 0).then_some(steps)
    }

    /// Call after anything that may have changed the object list or which
    /// object is active, and when `owes_settle` says so. The scene says which
    /// meshes it now holds for undo's sake alone; the oldest steps go if that
    /// puts the history over its cap, and the scene then frees every mesh
    /// that neither an object nor a remaining step shows.
    pub fn settle(&mut self, scene: &mut Scene) {
        self.spare = scene.spare_meshes();
        // Stepping back moves meshes out of the scene and into the redo steps
        // without using any more memory, and the next edit ends those steps.
        // Dropping undo steps to make up for them would lose work for nothing.
        if self.redo.is_empty() {
            self.trim();
        }
        scene.collect(|id| self.keeps_mesh(id));
        self.owed = false;
    }

    fn push(&mut self, edit: Edit) {
        // Every step recorded here is the model changing: the timelapse's cue.
        crate::timelapse::activity::edited();
        self.edits += 1;
        let ended = std::mem::take(&mut self.redo);
        self.undo.push_back(Step { edit, mesh: self.target, file: None, seen: None });
        self.gone(&ended);
        self.trim();
    }

    pub fn push_stroke(&mut self, delta: StrokeDelta) {
        self.push(Edit::Stroke(delta));
    }

    /// Records a hide, show or invert of the mesh's faces (what
    /// `Mesh::set_triangles_hidden`, `show_all` and `invert_visible` return).
    pub fn push_visibility(&mut self, delta: VisibilityDelta) {
        self.push(Edit::Visibility(delta));
    }

    /// Records an edit to the object list; `previous` is the list before it.
    pub fn push_scene(&mut self, previous: SceneState) {
        self.push(Edit::Scene(Box::new(previous)));
    }

    fn next(step: Option<&Step>) -> Option<Next> {
        step.map(|s| if matches!(s.edit, Edit::Scene(_)) { Next::Scene } else { Next::Mesh(s.mesh) })
    }

    pub fn next_undo(&self) -> Option<Next> {
        Self::next(self.undo.back())
    }

    pub fn next_redo(&self) -> Option<Next> {
        Self::next(self.redo.last())
    }

    /// Whether any kept object list still shows this mesh, so undo or redo
    /// could bring it back.
    pub fn keeps_mesh(&self, mesh: u64) -> bool {
        self.listed_meshes().any(|listed| listed == mesh)
    }

    /// Steps an object-list edit back (`undo`) or forward. Does nothing
    /// unless that is the next step.
    pub fn step_scene(&mut self, undo: bool, scene: &mut Scene, slot: &mut Mesh) -> bool {
        let next = if undo { self.next_undo() } else { self.next_redo() };
        if next != Some(Next::Scene) {
            return false;
        }
        let step = if undo { self.undo.pop_back() } else { self.redo.pop() };
        let Some(Step { edit: Edit::Scene(state), mesh, file, seen }) = step else { return false };
        let step = Step { edit: Edit::Scene(Box::new(scene.restore(*state, slot))), mesh, file, seen };
        if undo { self.redo.push(step) } else { self.undo.push_back(step) }
        true
    }

    /// Says that the step just recorded changed which file the scene belongs
    /// to; `before` is the file of the scene it replaced.
    pub fn keep_file(&mut self, before: FileState) {
        if let Some(step) = self.undo.back_mut() {
            step.file = Some(before);
        }
    }

    /// Call after a step back (`undo`) or forward. If the step just taken
    /// changed which file the scene belongs to, gives back the file of the
    /// scene now arrived at, and keeps `here`, the file of the scene just
    /// left, for the way back.
    pub fn cross_file(&mut self, undo: bool, here: impl FnOnce() -> FileState) -> Option<FileState> {
        let taken = if undo { self.redo.last_mut() } else { self.undo.back_mut() };
        let kept = taken?.file.as_mut()?;
        Some(std::mem::replace(kept, here()))
    }

    /// Says that the step just recorded put another scene in place of the
    /// one that was open; `before` is how that one was seen: its look and
    /// its view, as a file keeps them. An opened file brings its own, and
    /// Undo of the Open must not leave the scene from before under them.
    pub fn keep_seen(&mut self, before: Details) {
        if let Some(step) = self.undo.back_mut() {
            step.seen = Some(Box::new(before));
        }
    }

    /// Whether the step just taken, back (`undo`) or forward, keeps how the
    /// scene now arrived at was seen.
    pub fn keeps_seen(&self, undo: bool) -> bool {
        let taken = if undo { self.redo.last() } else { self.undo.back() };
        taken.is_some_and(|step| step.seen.is_some())
    }

    /// Call after a step back (`undo`) or forward that [`keeps_seen`]
    /// (Self::keeps_seen). Gives back how the scene now arrived at was seen,
    /// and keeps `here`, how the scene just left was, for the way back.
    pub fn cross_seen(&mut self, undo: bool, here: Details) -> Option<Details> {
        let taken = if undo { self.redo.last_mut() } else { self.undo.back_mut() };
        let kept = taken?.seen.as_mut()?;
        Some(std::mem::replace(&mut **kept, here))
    }

    /// The files kept for the scenes that undo and redo can still reach
    /// across such steps. A save looks through them: a file that has just
    /// been written no longer holds any scene but the one that was saved.
    pub fn kept_files(&mut self) -> impl Iterator<Item = &mut FileState> {
        self.undo.iter_mut().chain(&mut self.redo).filter_map(|step| step.file.as_mut())
    }

    /// Records that the whole mesh was replaced; `previous` is the old one.
    pub fn push_replace(&mut self, previous: Mesh) {
        self.push(Edit::Replace(Box::new(previous)));
    }

    /// Records that the mesh's subdivision levels were changed and nothing
    /// else of it (levels deleted); `previous` is what they were.
    pub fn push_levels(&mut self, previous: Option<Box<MeshLevels>>) {
        self.push(Edit::Levels(previous));
    }

    /// Records a change to the mesh's sculpt layers; `previous` is what the
    /// change itself hands back, the layers and positions as they were.
    pub fn push_layers(&mut self, previous: LayersStep) {
        self.push(Edit::Layers(Box::new(previous)));
    }

    /// Records a change to the mesh's face groups; `previous` is what the
    /// change itself hands back.
    pub fn push_groups(&mut self, previous: GroupsStep) {
        self.push(Edit::Groups(Box::new(previous)));
    }

    /// Records a change to the mesh's paint layers; `previous` is what the
    /// change itself hands back.
    #[allow(dead_code)] // for the Paint Layers card
    pub fn push_paint(&mut self, previous: PaintStep) {
        self.push(Edit::Paint(Box::new(previous)));
    }

    pub fn clear(&mut self) {
        self.edits += 1;
        let gone: Vec<Step> = self.undo.drain(..).chain(self.redo.drain(..)).collect();
        self.gone(&gone);
    }

    /// Steps a mesh edit back. Returns true if the mesh object itself was
    /// swapped (the renderer must then rebuild its buffers rather than patch
    /// them). `mesh` must be the one `next_undo` names; object-list steps go
    /// through `step_scene` instead.
    pub fn undo(&mut self, mesh: &mut Mesh) -> Option<bool> {
        if !matches!(self.next_undo(), Some(Next::Mesh(_))) {
            return None;
        }
        let step = self.undo.pop_back()?;
        let (edit, replaced) = Self::flip(step.edit, mesh, true);
        self.redo.push(Step { edit, mesh: step.mesh, file: step.file, seen: step.seen });
        Some(replaced)
    }

    pub fn redo(&mut self, mesh: &mut Mesh) -> Option<bool> {
        if !matches!(self.next_redo(), Some(Next::Mesh(_))) {
            return None;
        }
        let step = self.redo.pop()?;
        let (edit, replaced) = Self::flip(step.edit, mesh, false);
        self.undo.push_back(Step { edit, mesh: step.mesh, file: step.file, seen: step.seen });
        Some(replaced)
    }

    fn flip(edit: Edit, mesh: &mut Mesh, undo: bool) -> (Edit, bool) {
        match edit {
            Edit::Stroke(delta) => {
                mesh.apply_delta(&delta, undo);
                (Edit::Stroke(delta), false)
            }
            Edit::Visibility(delta) => {
                // The mesh itself stays: the renderer sees its visibility
                // version move and sends the faces that now show.
                mesh.apply_visibility(&delta, undo);
                (Edit::Visibility(delta), false)
            }
            Edit::Replace(mut other) => {
                std::mem::swap(mesh, &mut *other);
                (Edit::Replace(other), true)
            }
            Edit::Levels(mut other) => {
                // The mesh stays, and nothing drawn changes with its levels.
                std::mem::swap(&mut mesh.levels, &mut other);
                (Edit::Levels(other), false)
            }
            Edit::Layers(mut step) => {
                // The mesh stays; the vertices that move are patched like a stroke's.
                step.swap(mesh);
                (Edit::Layers(step), false)
            }
            Edit::Groups(mut step) => {
                // The mesh stays; the renderer sees the versions that moved.
                step.swap(mesh);
                (Edit::Groups(step), false)
            }
            Edit::Paint(mut step) => {
                // The mesh stays; its paint composite is worked out again.
                step.swap(mesh);
                (Edit::Paint(step), false)
            }
            Edit::Scene(state) => (Edit::Scene(state), false),
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::{Quat, Vec3};

    use super::*;
    use crate::mesh::Polys;
    use crate::mesh::primitives::{self, Primitive};
    use crate::scene::Object;

    fn ball() -> Mesh {
        primitives::build(Primitive::Sphere, 2_000, 0.5)
    }

    /// The mask value step `step` of a test paints.
    fn level(step: u32) -> f32 {
        step as f32 / 10.0
    }

    /// One mesh step: the whole mask set to `level(step)`.
    fn paint(mesh: &mut Mesh, history: &mut History, step: u32) {
        let delta = mesh.remap_mask(|_, _, _| level(step)).expect("the mask changes");
        history.push_stroke(delta);
    }

    /// What the steps themselves hold, without the meshes kept for them.
    fn own(history: &History) -> usize {
        history.undo.iter().chain(&history.redo).map(|s| s.edit.bytes()).sum()
    }

    /// Bytes of those of `meshes` that are still in memory.
    fn held(scene: &Scene, slot: &Mesh, meshes: &[u64]) -> usize {
        meshes.iter().filter_map(|&m| scene.mesh(m, slot)).map(Mesh::memory_bytes).sum()
    }

    /// An edit to the object list as the app makes one: the edit, its undo
    /// step, then the settling (`Running::edit_scene`).
    fn edit(scene: &mut Scene, slot: &mut Mesh, history: &mut History, change: impl FnOnce(&mut Scene, &mut Mesh)) {
        let before = scene.state();
        change(scene, slot);
        history.push_scene(before);
        history.settle(scene);
        // What the steps are charged, one by one, is what undo holds in all.
        assert_eq!(history.loads().first().map(|&(held, _)| held), Some(history.bytes()));
    }

    fn rename(scene: &mut Scene, slot: &mut Mesh, history: &mut History, name: &str) {
        edit(scene, slot, history, |scene, _| scene.objects[0].name = name.into());
    }

    fn delete(id: u64, scene: &mut Scene, slot: &mut Mesh, history: &mut History) {
        edit(scene, slot, history, |scene, slot| assert!(scene.remove(id, slot)));
    }

    /// Opens a file of two balls in place of the scene.
    fn open(scene: &mut Scene, slot: &mut Mesh, history: &mut History) {
        let (name, position, rotation, scale) = ("Ball".to_owned(), Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
        let object = Object { id: 0, name, visible: true, mesh: 0, position, rotation, scale, mirror: None };
        let objects = vec![object.clone(), Object { mesh: 1, ..object }];
        edit(scene, slot, history, |scene, slot| scene.load(vec![ball(), ball()], objects, 0, slot));
    }

    fn step(history: &mut History, undo: bool, scene: &mut Scene, slot: &mut Mesh) {
        assert!(history.step_scene(undo, scene, slot));
        history.settle(scene);
    }

    fn meshes(scene: &Scene) -> Vec<u64> {
        scene.objects.iter().map(|o| o.mesh).collect()
    }

    /// A cheap mesh step: one vertex's mask flipped, a dozen bytes of undo.
    fn dab(mesh: &mut Mesh, history: &mut History) {
        let flip = |_: &Mesh, v: u32, old: f32| if v == 0 { 1.0 - old.round() } else { old };
        history.push_stroke(mesh.remap_mask(flip).expect("the mask changes"));
    }

    /// A stroke as the app makes one: its step, then what the next frame
    /// does about it (`Running::undo_upkeep`).
    fn stroke(scene: &mut Scene, slot: &mut Mesh, history: &mut History) {
        dab(slot, history);
        if history.owes_settle() {
            history.settle(scene);
        }
    }

    /// A remesh as the app records one: the old mesh kept by the step.
    fn remesh(mesh: &mut Mesh, history: &mut History) {
        history.push_replace(std::mem::replace(mesh, ball()));
    }

    /// A scene of the first ball and `count` more, the last of them in hand:
    /// their ids and their meshes.
    fn balls(count: usize, slot: &mut Mesh) -> (Scene, Vec<u64>, Vec<u64>) {
        let mut scene = Scene::new("Ball");
        let ids: Vec<u64> = (0..count).map(|_| scene.add("Ball", ball(), slot)).collect();
        let meshes = ids.iter().map(|&id| scene.get(id).unwrap().mesh).collect();
        (scene, ids, meshes)
    }

    /// Which of `meshes` are still in memory.
    fn parked(scene: &Scene, slot: &Mesh, meshes: &[u64]) -> Vec<bool> {
        meshes.iter().map(|&m| scene.mesh(m, slot).is_some()).collect()
    }

    /// A view from `yaw` degrees round, as a file keeps one.
    fn seen_from(yaw: f32) -> Details {
        let mut seen = Details::default();
        seen.set(crate::io::slot::CAMERA, &crate::camera::SavedView { yaw, ..Default::default() });
        seen
    }

    fn yaw_of(seen: Option<Details>) -> Option<f32> {
        seen?.get::<crate::camera::SavedView>(crate::io::slot::CAMERA).map(|view| view.yaw)
    }

    /// An Open keeps how the scene it replaced was seen, and each time Undo
    /// or Redo crosses it the two change places, however often: the scene
    /// arrived at gets its own look and view back, and the step keeps those
    /// of the scene just left. Steps on either side of it keep nothing.
    #[test]
    fn a_scene_put_in_place_of_another_keeps_how_that_one_was_seen() {
        let (mut scene, mut slot, mut history) = (Scene::new("Sphere"), ball(), History::default());
        rename(&mut scene, &mut slot, &mut history, "Head");
        open(&mut scene, &mut slot, &mut history);
        history.keep_seen(seen_from(10.0));
        rename(&mut scene, &mut slot, &mut history, "Opened");

        // The rename after it is an ordinary step.
        assert!(history.step_scene(true, &mut scene, &mut slot));
        assert!(!history.keeps_seen(true));
        assert_eq!(yaw_of(history.cross_seen(true, seen_from(99.0))), None);

        // Back across the Open: the old scene's view, and the opened one's kept.
        for round in 0..3 {
            assert!(history.step_scene(true, &mut scene, &mut slot));
            assert!(history.keeps_seen(true), "round {round}");
            assert_eq!(yaw_of(history.cross_seen(true, seen_from(20.0))), Some(10.0), "round {round}");
            assert_eq!(scene.objects[0].name, "Head");
            // Forward again: the opened scene's, and the old one's kept as it was left.
            assert!(history.step_scene(false, &mut scene, &mut slot));
            assert!(history.keeps_seen(false), "round {round}");
            assert_eq!(yaw_of(history.cross_seen(false, seen_from(10.0))), Some(20.0), "round {round}");
            assert_eq!(scene.objects.len(), 2);
        }

        // The rename before it is an ordinary step too.
        assert!(history.step_scene(true, &mut scene, &mut slot));
        assert!(history.step_scene(true, &mut scene, &mut slot));
        assert!(!history.keeps_seen(true));
        assert_eq!(scene.objects[0].name, "Sphere");
    }

    #[test]
    fn the_step_cap_drops_the_oldest_steps() {
        let mut mesh = ball();
        let mut history = History { max_steps: 3, ..History::default() };
        for step in 1..=5 {
            paint(&mut mesh, &mut history, step);
        }
        assert_eq!(history.steps(), (3, 0));
        // The three left are the last three: undoing them all leaves the second.
        while history.undo(&mut mesh).is_some() {}
        assert_eq!(history.steps(), (0, 3));
        assert!(mesh.mask.iter().all(|&m| m == level(2)));
    }

    #[test]
    fn the_byte_cap_drops_the_oldest_steps() {
        let mut mesh = ball();
        let mut history = History::default();
        paint(&mut mesh, &mut history, 1);
        let one = history.bytes();
        assert!(one > 0);
        // Room for three such steps, not four.
        history.max_bytes = one * 7 / 2;
        for step in 2..=6 {
            paint(&mut mesh, &mut history, step);
            let kept = (step as usize).min(3);
            assert_eq!((history.steps(), history.bytes()), ((kept, 0), one * kept));
        }
        while history.undo(&mut mesh).is_some() {}
        assert!(mesh.mask.iter().all(|&m| m == level(3)));
    }

    /// Strokes so big that three of them are over the cap: the last three
    /// stay, and since a stroke is not a mesh the older ones stay too, up to
    /// twice the cap. That is the most undo ever holds beside its last step.
    #[test]
    fn strokes_too_big_for_three_to_fit_are_held_to_twice_the_cap() {
        let mut mesh = ball();
        let mut history = History::default();
        paint(&mut mesh, &mut history, 1);
        let one = history.bytes();
        history.max_bytes = one * 5 / 2;
        for step in 2..=8 {
            paint(&mut mesh, &mut history, step);
            let kept = (step as usize).min(5);
            assert_eq!((history.steps(), history.bytes()), ((kept, 0), one * kept));
        }
        assert_eq!(history.take_let_go(), Some(1));
    }

    #[test]
    fn the_last_step_stays_however_low_the_caps_are() {
        for (max_steps, max_bytes) in [(0, usize::MAX), (1, usize::MAX), (100, 0), (100, 1), (0, 0)] {
            let mut mesh = ball();
            let mut history = History { max_steps, max_bytes, ..History::default() };
            for step in 1..=3 {
                paint(&mut mesh, &mut history, step);
                assert_eq!(history.steps(), (1, 0), "caps of {max_steps} steps and {max_bytes} bytes");
            }
            assert!(history.undo(&mut mesh).is_some());
            assert!(mesh.mask.iter().all(|&m| m == level(2)));
        }
    }

    #[test]
    fn a_replaced_mesh_is_charged_to_its_step() {
        let mut mesh = ball();
        let one = mesh.memory_bytes();
        // Room for three old meshes and a bit.
        let mut history = History { max_bytes: one * 7 / 2, ..History::default() };
        for _ in 0..5 {
            let old = std::mem::replace(&mut mesh, ball());
            history.push_replace(old);
        }
        assert_eq!(history.steps(), (3, 0));
        assert!(history.bytes() >= one * 3 && history.bytes() <= history.max_bytes);
    }

    #[test]
    fn a_mesh_is_counted_with_everything_big_it_holds() {
        let mut mesh = ball();
        let (verts, corners) = (mesh.vertex_count(), mesh.indices.len());
        // Positions, normals and mask; the triangles, the triangles on each
        // vertex and each vertex's neighbours (a closed mesh has as many of
        // each as it has corners); and the tree's leaf for each vertex.
        let plain = mesh.memory_bytes();
        assert!(plain >= verts * (12 + 12 + 4) + corners * 4 * 3 + verts * 4, "{plain} bytes for {verts} vertices");
        mesh.track_curvature(true);
        let curved = mesh.memory_bytes();
        assert!(curved >= plain + verts * 4);
        mesh.polys = Some(Polys { start: vec![0; 10], indices: vec![0; 40] });
        assert_eq!(mesh.memory_bytes(), curved + 200);
    }

    /// The case the review found: a few deletes of big objects. Every deleted
    /// mesh stays in memory for undo, so the history has to own up to it and
    /// let the oldest go once they are over the cap.
    #[test]
    fn deleting_big_objects_one_after_another_stays_under_the_byte_cap() {
        let mut slot = ball();
        let mut scene = Scene::new("Ball");
        let ids: Vec<u64> = (0..5).map(|_| scene.add("Ball", ball(), &mut slot)).collect();
        let meshes: Vec<u64> = ids.iter().map(|&id| scene.get(id).unwrap().mesh).collect();
        // Room for three balls and a bit, as the app's cap has room for a few
        // multi-million-triangle models.
        let mut history = History { max_bytes: slot.memory_bytes() * 7 / 2, ..History::default() };
        for &id in &ids {
            delete(id, &mut scene, &mut slot, &mut history);
        }
        let kept = held(&scene, &slot, &meshes);
        assert!(
            history.bytes() >= kept,
            "the history says it holds {} bytes while {kept} bytes of deleted meshes are kept for it",
            history.bytes()
        );
        let cap = history.max_bytes;
        assert!(kept <= cap, "{kept} bytes of deleted meshes are kept, over the cap of {cap}");

        // The three newest deletes are the ones that fit. The meshes of the
        // other two are gone from memory, not only from the count.
        assert_eq!(history.steps(), (3, 0));
        assert_eq!(history.bytes(), own(&history) + kept);
        assert_eq!(parked(&scene, &slot, &meshes), [false, false, true, true, true]);
        // The user was told once, when the fourth delete pushed the first
        // out, and not again when the fifth pushed the second.
        assert_eq!(history.take_let_go(), Some(1));
        // And those three still undo.
        for _ in 0..3 {
            step(&mut history, true, &mut scene, &mut slot);
        }
        assert!(!history.can_undo());
        assert!(ids[2..].iter().all(|&id| scene.get(id).is_some()) && scene.get(ids[1]).is_none());
        assert!(scene.mesh(meshes[4], &slot).is_some_and(|m| m.triangle_count() > 0));
    }

    #[test]
    fn a_mesh_is_charged_once_and_only_while_no_object_shows_it() {
        let mut slot = ball();
        let mut scene = Scene::new("Ball");
        let mut history = History::default();
        let first = scene.add("Ball", ball(), &mut slot);
        // Sculpted, so its resting shape is kept with it.
        scene.keep_baseline(&slot);
        let shared = scene.active_mesh();
        let size = slot.memory_bytes() + slot.vertex_count() * 12;
        let twin = scene.instance_active();

        // Steps whose meshes are all still in the scene cost only their lists.
        rename(&mut scene, &mut slot, &mut history, "Head");
        assert_eq!(history.bytes(), own(&history));
        // One of the two instances deleted: the other still shows the mesh.
        delete(first, &mut scene, &mut slot, &mut history);
        assert_eq!(history.bytes(), own(&history));
        // Both deleted: two steps show the mesh, one of them twice. Charged once.
        delete(twin, &mut scene, &mut slot, &mut history);
        assert_eq!(history.bytes(), own(&history) + size);

        // The older of those steps goes; the newer still shows the mesh.
        history.max_steps = 1;
        history.settle(&mut scene);
        assert_eq!(history.steps(), (1, 0));
        assert_eq!(history.bytes(), own(&history) + size);
        assert!(scene.mesh(shared, &slot).is_some());
        // The last step that showed it goes: it is freed and no longer counted.
        rename(&mut scene, &mut slot, &mut history, "Body");
        assert_eq!(history.steps(), (1, 0));
        assert_eq!(history.bytes(), own(&history));
        assert!(scene.mesh(shared, &slot).is_none());
        assert!(scene.spare_meshes().is_empty());
    }

    #[test]
    fn the_newest_step_is_kept_even_when_its_mesh_alone_is_over_the_cap() {
        let mut slot = ball();
        let mut scene = Scene::new("Ball");
        let added = scene.add("Ball", ball(), &mut slot);
        let mesh = scene.active_mesh();
        let mut history = History { max_bytes: slot.memory_bytes() / 2, ..History::default() };
        rename(&mut scene, &mut slot, &mut history, "Head");
        rename(&mut scene, &mut slot, &mut history, "Body");
        assert_eq!(history.steps(), (2, 0));

        // Nothing goes for it: dropping the renames would free two short
        // lists and leave undo as far over the cap as it was.
        delete(added, &mut scene, &mut slot, &mut history);
        assert_eq!(history.steps(), (3, 0));
        assert!(history.bytes() > history.max_bytes);
        assert_eq!(history.take_let_go(), None);
        assert!(scene.mesh(mesh, &slot).is_some());
        // Undo brings the object back, and the history holds next to nothing.
        step(&mut history, true, &mut scene, &mut slot);
        assert!(scene.get(added).is_some() && scene.mesh(mesh, &slot).is_some());
        assert_eq!(history.steps(), (2, 1));
        assert_eq!(history.bytes(), own(&history));
    }

    #[test]
    fn a_mesh_only_redo_can_bring_back_is_charged_and_costs_no_undo_steps() {
        let mut slot = ball();
        let mut scene = Scene::new("Ball");
        let mut history = History::default();
        rename(&mut scene, &mut slot, &mut history, "Head");
        rename(&mut scene, &mut slot, &mut history, "Body");
        let mut added = 0;
        edit(&mut scene, &mut slot, &mut history, |scene, slot| added = scene.add("Ball", ball(), slot));
        let (mesh, size) = (scene.active_mesh(), slot.memory_bytes());
        assert_eq!(history.bytes(), own(&history));

        // The add undone: the ball is in memory for the redo step alone. That
        // is over the cap, but stepping back used no memory, so nothing goes.
        history.max_bytes = size / 2;
        step(&mut history, true, &mut scene, &mut slot);
        assert_eq!(history.steps(), (2, 1));
        assert_eq!(history.bytes(), own(&history) + size);
        assert!(scene.get(added).is_none() && scene.mesh(mesh, &slot).is_some());
        // Redo puts it back in the scene, where it costs undo nothing.
        step(&mut history, false, &mut scene, &mut slot);
        assert_eq!(history.steps(), (3, 0));
        assert!(scene.get(added).is_some());
        assert_eq!(history.bytes(), own(&history));

        // Undone again, and then a new edit: redo ends and the ball is freed.
        step(&mut history, true, &mut scene, &mut slot);
        rename(&mut scene, &mut slot, &mut history, "Torso");
        assert_eq!(history.steps(), (3, 0));
        assert_eq!(history.bytes(), own(&history));
        assert!(scene.mesh(mesh, &slot).is_none());
    }

    #[test]
    fn opened_over_and_joined_meshes_are_charged_and_let_go_like_deleted_ones() {
        let mut slot = ball();
        let mut scene = Scene::new("Ball");
        scene.add("Ball", ball(), &mut slot);
        // Room for six balls and a bit: three scenes of two.
        let mut history = History { max_bytes: slot.memory_bytes() * 13 / 2, ..History::default() };
        let mut opened_over = Vec::new();
        for kept_scenes in [1, 2, 3, 3] {
            opened_over.push(meshes(&scene));
            open(&mut scene, &mut slot, &mut history);
            let kept = held(&scene, &slot, &opened_over.concat());
            assert_eq!(history.steps(), (kept_scenes, 0));
            assert_eq!(history.bytes(), own(&history) + kept);
            assert!(kept > 0 && history.bytes() <= history.max_bytes);
        }
        // The fourth Open pushed the first scene's two meshes out of memory.
        assert_eq!(held(&scene, &slot, &opened_over[0]), 0);
        assert!(opened_over[1..].iter().all(|scene_meshes| held(&scene, &slot, scene_meshes) > 0));

        // A Join: both of its meshes are kept for undo, and the oldest Open goes.
        let (ids, joined) = (scene.objects.iter().map(|o| o.id).collect::<Vec<_>>(), meshes(&scene));
        scene.select(ids[0], &mut slot);
        scene.select_also(ids[1]);
        edit(&mut scene, &mut slot, &mut history, |scene, slot| {
            let mesh = scene.joined_mesh(slot).expect("two are selected");
            scene.install_joined(mesh, slot);
        });
        assert_eq!(history.steps(), (3, 0));
        assert_eq!(held(&scene, &slot, &opened_over[1]), 0);
        let kept = held(&scene, &slot, &opened_over[2..].concat()) + held(&scene, &slot, &joined);
        assert_eq!(history.bytes(), own(&history) + kept);
        assert_eq!(scene.spare_meshes().len(), 6);
    }

    // ---- what the memory cap lets go, and when ------------------------------

    /// The second review's case. Six cheap strokes, then the delete of an
    /// object too big for the cap: dropping the strokes frees a few bytes
    /// and leaves undo as far over as it was.
    #[test]
    fn strokes_are_not_dropped_for_a_step_that_is_over_the_cap_alone() {
        let mut slot = ball();
        let (mut scene, ids, meshes) = balls(1, &mut slot);
        let mut history = History { max_bytes: slot.memory_bytes() / 2, ..History::default() };
        for _ in 0..6 {
            stroke(&mut scene, &mut slot, &mut history);
        }
        delete(ids[0], &mut scene, &mut slot, &mut history);
        assert_eq!(history.steps(), (7, 0));
        assert!(history.bytes() > history.max_bytes * 2);
        assert_eq!(history.take_let_go(), None);
        assert!(scene.mesh(meshes[0], &slot).is_some());

        // It is more than twice the cap, so it stays only while it is the
        // last step. The next stroke lets it go, with the six before it, and
        // the mesh leaves memory there and then.
        stroke(&mut scene, &mut slot, &mut history);
        assert_eq!(history.steps(), (1, 0));
        assert!(!history.keeps_mesh(meshes[0]));
        assert!(scene.mesh(meshes[0], &slot).is_none(), "the mesh no step can bring back is still in memory");
        assert_eq!(history.bytes(), own(&history));
        assert_eq!(history.take_let_go(), Some(7));
        assert!(!history.owes_settle());
    }

    #[test]
    fn a_step_over_the_cap_stays_while_it_is_one_of_the_last_three() {
        let mut slot = ball();
        let (mut scene, ids, meshes) = balls(1, &mut slot);
        // The ball is over the cap and under twice the cap.
        let mut history = History { max_bytes: slot.memory_bytes() * 3 / 4, ..History::default() };
        for _ in 0..6 {
            stroke(&mut scene, &mut slot, &mut history);
        }
        delete(ids[0], &mut scene, &mut slot, &mut history);
        for kept in [8, 9] {
            stroke(&mut scene, &mut slot, &mut history);
            assert_eq!(history.steps(), (kept, 0));
            assert!(history.bytes() > history.max_bytes && history.bytes() <= history.max_bytes * 2);
            assert!(scene.mesh(meshes[0], &slot).is_some());
        }
        assert_eq!(history.take_let_go(), None);

        // The third stroke after it: it is the fourth step back, and goes.
        stroke(&mut scene, &mut slot, &mut history);
        assert_eq!(history.steps(), (3, 0));
        assert!(scene.mesh(meshes[0], &slot).is_none());
        assert!(history.bytes() <= history.max_bytes);
        assert_eq!(history.take_let_go(), Some(7));
    }

    #[test]
    fn a_second_big_delete_lets_the_first_go_and_keeps_the_strokes_between() {
        // Either ball fits the cap; the two together do not.
        let mut slot = ball();
        let (mut scene, ids, meshes) = balls(2, &mut slot);
        let mut history = History { max_bytes: slot.memory_bytes() * 3 / 2, ..History::default() };
        delete(ids[0], &mut scene, &mut slot, &mut history);
        for _ in 0..4 {
            stroke(&mut scene, &mut slot, &mut history);
        }
        assert_eq!(history.take_let_go(), None);
        delete(ids[1], &mut scene, &mut slot, &mut history);
        assert_eq!(history.steps(), (5, 0));
        assert_eq!(parked(&scene, &slot, &meshes), [false, true]);
        assert!(history.bytes() <= history.max_bytes);
        assert_eq!(history.take_let_go(), Some(1));

        // Each ball alone is over the cap: the second delete stays because it
        // is the last step, the first has no such claim.
        let mut slot = ball();
        let (mut scene, ids, meshes) = balls(2, &mut slot);
        let mut history = History { max_bytes: slot.memory_bytes() * 3 / 4, ..History::default() };
        delete(ids[0], &mut scene, &mut slot, &mut history);
        stroke(&mut scene, &mut slot, &mut history);
        stroke(&mut scene, &mut slot, &mut history);
        delete(ids[1], &mut scene, &mut slot, &mut history);
        assert_eq!(history.steps(), (3, 0));
        assert_eq!(parked(&scene, &slot, &meshes), [false, true]);
        assert_eq!(history.bytes(), own(&history) + held(&scene, &slot, &meshes[1..]));
        assert_eq!(history.take_let_go(), Some(1));
    }

    #[test]
    fn two_big_deletes_close_together_both_stay_until_the_first_is_four_steps_back() {
        let mut slot = ball();
        let (mut scene, ids, meshes) = balls(2, &mut slot);
        let mut history = History { max_bytes: slot.memory_bytes() * 3 / 2, ..History::default() };
        delete(ids[0], &mut scene, &mut slot, &mut history);
        stroke(&mut scene, &mut slot, &mut history);
        delete(ids[1], &mut scene, &mut slot, &mut history);
        // Over the cap, and all of it in the last three steps.
        assert_eq!(history.steps(), (3, 0));
        assert!(history.bytes() > history.max_bytes && history.bytes() <= history.max_bytes * 2);
        assert_eq!(parked(&scene, &slot, &meshes), [true, true]);
        assert_eq!(history.take_let_go(), None);

        stroke(&mut scene, &mut slot, &mut history);
        assert_eq!(history.steps(), (3, 0));
        assert_eq!(parked(&scene, &slot, &meshes), [false, true]);
        assert!(history.bytes() <= history.max_bytes);
        assert_eq!(history.take_let_go(), Some(1));
    }

    #[test]
    fn replaced_meshes_are_let_go_by_the_same_rules_as_deleted_ones() {
        let size = ball().memory_bytes();
        // The cap, strokes and remeshes in order, and the steps left after each.
        let over_twice = (size / 2, "ssssssRs", vec![1, 2, 3, 4, 5, 6, 7, 1]);
        let over_once = (size * 3 / 4, "ssssssRsss", vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 3]);
        let two_that_do_not_fit = (size * 3 / 2, "RssssR", vec![1, 2, 3, 4, 5, 5]);
        let two_over_the_cap = (size * 3 / 4, "RssR", vec![1, 2, 3, 3]);
        let two_close_together = (size * 3 / 2, "RsRs", vec![1, 2, 3, 3]);
        for (cap, edits, kept) in [over_twice, over_once, two_that_do_not_fit, two_over_the_cap, two_close_together] {
            let mut mesh = ball();
            let mut history = History { max_bytes: cap, ..History::default() };
            for (edit, kept) in edits.chars().zip(kept) {
                if edit == 'R' { remesh(&mut mesh, &mut history) } else { dab(&mut mesh, &mut history) }
                assert_eq!(history.steps(), (kept, 0), "after {edit} of {edits} with a cap of {cap}");
            }
            // A replaced mesh is the step's own: nothing waits on the scene.
            assert!(!history.owes_settle());
        }
    }

    #[test]
    fn older_steps_that_keep_a_mesh_go_when_the_last_three_are_over_the_cap() {
        let mut mesh = ball();
        let one = mesh.memory_bytes();
        // Room for two remeshed balls and a bit. Three are over the cap and
        // under twice it: the last three steps stay, and nothing older that
        // keeps a mesh does, whatever room there is up to twice the cap.
        let mut history = History { max_bytes: one * 5 / 2, ..History::default() };
        dab(&mut mesh, &mut history);
        for kept in [2, 3, 4, 3, 3] {
            remesh(&mut mesh, &mut history);
            assert_eq!(history.steps(), (kept, 0));
        }
        assert_eq!(history.bytes(), one * 3);
        assert_eq!(history.take_let_go(), Some(2));
    }

    #[test]
    fn the_last_three_steps_give_way_over_twice_the_cap() {
        let mut mesh = ball();
        let one = mesh.memory_bytes();
        // Three remeshed balls are 3, twice the cap is 2.5.
        let mut history = History { max_bytes: one * 5 / 4, ..History::default() };
        for kept in [1, 2, 2, 2] {
            remesh(&mut mesh, &mut history);
            assert_eq!(history.steps(), (kept, 0));
            assert!(history.bytes() <= history.max_bytes * 2);
        }
        // Two and a stroke are under twice the cap; then the older of the two
        // is four steps back, and the rest fit the cap without it.
        for _ in 0..2 {
            dab(&mut mesh, &mut history);
            assert_eq!(history.steps(), (3, 0));
        }
        assert!(history.bytes() <= history.max_bytes);
    }

    // ---- meshes the scene holds for steps that have gone ---------------------

    #[test]
    fn a_mesh_is_freed_when_the_step_cap_drops_its_delete() {
        let mut slot = ball();
        let (mut scene, ids, meshes) = balls(1, &mut slot);
        let mut history = History { max_steps: 3, ..History::default() };
        delete(ids[0], &mut scene, &mut slot, &mut history);
        for kept in [true, true, false] {
            stroke(&mut scene, &mut slot, &mut history);
            assert_eq!(scene.mesh(meshes[0], &slot).is_some(), kept);
        }
        assert_eq!(history.steps(), (3, 0));
        // The step cap is not the memory cap: nothing to tell the user.
        assert_eq!(history.take_let_go(), None);
    }

    #[test]
    fn a_mesh_only_redo_kept_is_freed_when_a_stroke_ends_redo() {
        let mut slot = ball();
        let mut scene = Scene::new("Ball");
        let mut history = History::default();
        let mut added = 0;
        edit(&mut scene, &mut slot, &mut history, |scene, slot| added = scene.add("Ball", ball(), slot));
        let mesh = scene.active_mesh();
        step(&mut history, true, &mut scene, &mut slot);
        assert!(scene.get(added).is_none() && scene.mesh(mesh, &slot).is_some() && history.keeps_mesh(mesh));

        stroke(&mut scene, &mut slot, &mut history);
        assert_eq!(history.steps(), (1, 0));
        assert!(scene.mesh(mesh, &slot).is_none(), "the mesh redo kept is still in memory with redo gone");
    }

    #[test]
    fn clearing_the_history_owes_the_scene_a_settle_for_the_meshes_it_kept() {
        let mut slot = ball();
        let (mut scene, ids, meshes) = balls(1, &mut slot);
        let mut history = History::default();
        delete(ids[0], &mut scene, &mut slot, &mut history);
        history.clear();
        assert!(history.owes_settle());
        history.settle(&mut scene);
        assert!(scene.mesh(meshes[0], &slot).is_none() && !history.owes_settle());
    }

    #[test]
    fn no_settle_is_owed_for_steps_that_kept_no_mesh() {
        let mut slot = ball();
        let (mut scene, ids, _) = balls(1, &mut slot);
        let mut history = History { max_steps: 3, ..History::default() };
        rename(&mut scene, &mut slot, &mut history, "Head");
        delete(ids[0], &mut scene, &mut slot, &mut history);
        // The rename's list shows the ball, and so does the delete's after
        // it: dropping the rename frees nothing. Then a stroke drops a stroke.
        for _ in 0..2 {
            dab(&mut slot, &mut history);
            assert!(!history.owes_settle());
        }
        assert_eq!(history.steps(), (3, 0));
        // The delete goes: now the scene holds a mesh for nothing.
        dab(&mut slot, &mut history);
        assert!(history.owes_settle());
    }

    // ---- telling the user ----------------------------------------------------

    #[test]
    fn steps_the_memory_cap_drops_are_reported_once_for_each_run_of_drops() {
        let mut mesh = ball();
        // Room for three remeshed balls and a bit.
        let mut history = History { max_bytes: mesh.memory_bytes() * 7 / 2, ..History::default() };
        for _ in 0..3 {
            remesh(&mut mesh, &mut history);
        }
        assert_eq!(history.take_let_go(), None);
        remesh(&mut mesh, &mut history);
        assert_eq!(history.steps(), (3, 0));
        assert_eq!(history.take_let_go(), Some(1));
        assert_eq!(history.take_let_go(), None);
        // The next edits each drop a step too: the user has been told.
        for _ in 0..3 {
            remesh(&mut mesh, &mut history);
            assert_eq!((history.steps(), history.take_let_go()), ((3, 0), None));
        }
        // An edit that drops nothing ends the run; the next drop is news.
        dab(&mut mesh, &mut history);
        assert_eq!((history.steps(), history.take_let_go()), ((4, 0), None));
        remesh(&mut mesh, &mut history);
        assert_eq!((history.steps(), history.take_let_go()), ((4, 0), Some(1)));
    }

    #[test]
    fn what_one_edit_drops_in_two_goes_is_told_or_not_together() {
        let mut mesh = ball();
        let mut scene = Scene::new("Ball");
        let one = mesh.memory_bytes();
        let mut history = History { max_bytes: one * 13 / 2, ..History::default() };
        for _ in 0..7 {
            remesh(&mut mesh, &mut history);
        }
        // The seventh remesh pushed the first out; settling the same edit
        // with less room drops another. Both are news.
        history.max_bytes = one * 11 / 2;
        history.settle(&mut scene);
        assert_eq!((history.steps(), history.take_let_go()), ((5, 0), Some(2)));
        // The eighth pushes one out as well, which is not news; nor is what
        // settling that edit drops.
        remesh(&mut mesh, &mut history);
        history.max_bytes = one * 9 / 2;
        history.settle(&mut scene);
        assert_eq!((history.steps(), history.take_let_go()), ((4, 0), None));
    }

    #[test]
    fn a_delete_reports_what_its_settling_dropped() {
        let mut slot = ball();
        let (mut scene, ids, _) = balls(4, &mut slot);
        let mut history = History { max_bytes: slot.memory_bytes() * 7 / 2, ..History::default() };
        for &id in &ids[..3] {
            dab(&mut slot, &mut history);
            delete(id, &mut scene, &mut slot, &mut history);
        }
        assert_eq!((history.steps(), history.take_let_go()), ((6, 0), None));
        // The fourth ball does not fit: the first delete goes, and the stroke
        // before it. Recording the step drops nothing (the scene has yet to
        // say what it parked); the settling after it does.
        delete(ids[3], &mut scene, &mut slot, &mut history);
        assert_eq!((history.steps(), history.take_let_go()), ((5, 0), Some(2)));
    }
}
