//! Undo / redo. Strokes are stored as sparse per-vertex deltas (a few
//! kilobytes for a typical stroke); operations that replace the whole mesh
//! (remesh, import, new shape) keep the previous mesh itself, so stepping
//! across them is instant. Edits to the object list (add, delete, rename,
//! reorder...) keep the list as it was. A step that put one file's scene in
//! place of another's (an Open, a restored recovery copy) also keeps which
//! file the scene on its other side belongs to (`FileState`), so that the
//! file comes back with the objects.
//!
//! Mesh steps remember which mesh they belong to, so undo can bring that
//! mesh back to hand before stepping.

use std::collections::VecDeque;

use crate::mesh::{Mesh, StrokeDelta, VisibilityDelta};
use crate::scene::{Scene, SceneState};

pub enum Edit {
    Stroke(StrokeDelta),
    /// Faces of the mesh hidden, shown again or swapped (see mesh/visibility.rs).
    Visibility(VisibilityDelta),
    /// The mesh that was current on the other side of this step.
    Replace(Box<Mesh>),
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
    /// Whether that scene is what its file holds.
    pub saved: bool,
}

struct Step {
    edit: Edit,
    /// The mesh a stroke or replace applies to.
    mesh: u64,
    /// Only on a step that changed which file the scene belongs to: the file
    /// on its other side (see `keep_file`).
    file: Option<FileState>,
}

/// What the next undo or redo step will touch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Next {
    Mesh(u64),
    Scene,
}

impl Edit {
    fn bytes(&self) -> usize {
        match self {
            Edit::Stroke(d) => d.bytes(),
            Edit::Visibility(d) => d.bytes(),
            Edit::Replace(m) => m.memory_bytes(),
            Edit::Scene(s) => s.objects.len() * std::mem::size_of::<crate::scene::Object>(),
        }
    }
}

pub struct History {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    /// Id of the mesh being edited; new stroke and replace steps belong to it.
    pub target: u64,
    pub max_steps: usize,
    pub max_bytes: usize,
    /// How many steps have ever been recorded (or thrown away by `clear`).
    /// With `steps`, it tells whether anything changed since the scene was
    /// last saved: see `Running::unsaved` in app/files.rs.
    pub edits: u64,
}

impl Default for History {
    fn default() -> Self {
        Self { undo: VecDeque::new(), redo: Vec::new(), target: 0, max_steps: 100, max_bytes: 2 << 30, edits: 0 }
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

    pub fn bytes(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(|s| s.edit.bytes()).sum()
    }

    fn trim(&mut self) {
        while self.undo.len() > self.max_steps
            || (self.undo.len() > 1 && self.bytes() > self.max_bytes)
        {
            self.undo.pop_front();
        }
    }

    fn push(&mut self, edit: Edit) {
        // Every step recorded here is the model changing: the timelapse's cue.
        crate::timelapse::activity::edited();
        self.edits += 1;
        self.redo.clear();
        self.undo.push_back(Step { edit, mesh: self.target, file: None });
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
        self.undo.iter().chain(&self.redo).any(|s| match &s.edit {
            Edit::Scene(state) => state.objects.iter().any(|o| o.mesh == mesh),
            _ => false,
        })
    }

    /// Steps an object-list edit back (`undo`) or forward. Does nothing
    /// unless that is the next step.
    pub fn step_scene(&mut self, undo: bool, scene: &mut Scene, slot: &mut Mesh) -> bool {
        let next = if undo { self.next_undo() } else { self.next_redo() };
        if next != Some(Next::Scene) {
            return false;
        }
        let step = if undo { self.undo.pop_back() } else { self.redo.pop() };
        let Some(Step { edit: Edit::Scene(state), mesh, file }) = step else { return false };
        let step = Step { edit: Edit::Scene(Box::new(scene.restore(*state, slot))), mesh, file };
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

    pub fn clear(&mut self) {
        self.edits += 1;
        self.undo.clear();
        self.redo.clear();
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
        self.redo.push(Step { edit, mesh: step.mesh, file: step.file });
        Some(replaced)
    }

    pub fn redo(&mut self, mesh: &mut Mesh) -> Option<bool> {
        if !matches!(self.next_redo(), Some(Next::Mesh(_))) {
            return None;
        }
        let step = self.redo.pop()?;
        let (edit, replaced) = Self::flip(step.edit, mesh, false);
        self.undo.push_back(Step { edit, mesh: step.mesh, file: step.file });
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
            Edit::Scene(state) => (Edit::Scene(state), false),
        }
    }
}
