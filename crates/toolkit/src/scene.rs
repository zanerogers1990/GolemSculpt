//! What a tool can read of the scene, and the commands it changes it with.
//!
//! Reading is by reference: an object's vertices, normals, triangles and mask
//! are lent as slices of the app's own arrays, so looking at a ten-million
//! triangle mesh costs nothing. Changing goes through [`Command`]s, which the
//! app carries out as ordinary undo steps; a tool never holds the document
//! itself, so it cannot leave the scene or its history half-edited.

use std::any::Any;
use std::fmt;

use glam::{Mat4, Quat, Vec3};
use sculpt_geom::{Aabb, TriMesh};

use crate::view::Ray;

/// Names an object in the scene. It stays the same across undo and redo: an
/// object that is deleted and brought back by Z has the id it had before.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ObjectId(pub u64);

/// Names the geometry an object shows. Instances share one; an object whose
/// mesh is replaced gets a new one, and gets the old one back when that is
/// undone. Comparing the id an object has now with the one it had is how a
/// tool notices that its own change was undone.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct MeshId(pub u64);

/// Where an object sits: its own space into world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub position: Vec3,
    pub rotation: Quat,
    /// Size along the object's own axes.
    pub scale: Vec3,
}

impl Placement {
    pub const IDENTITY: Placement = Placement { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    pub fn at(position: Vec3) -> Placement {
        Placement { position, ..Placement::IDENTITY }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

impl Default for Placement {
    fn default() -> Placement {
        Placement::IDENTITY
    }
}

/// One row of the object list.
#[derive(Clone, Copy, Debug)]
pub struct ObjectInfo<'a> {
    pub id: ObjectId,
    pub name: &'a str,
    pub visible: bool,
    /// The selected object: the one brushes work on.
    pub active: bool,
    pub mesh: MeshId,
    pub placement: Placement,
    pub vertices: usize,
    pub triangles: usize,
}

impl ObjectInfo<'_> {
    /// The object's own space into world space.
    pub fn matrix(&self) -> Mat4 {
        self.placement.matrix()
    }
}

/// An object's geometry, lent for reading, in the object's own space.
///
/// The app keeps vertices in an order of its own (it sorts them so nearby
/// vertices sit together), so the order here is not the order of the mesh a
/// tool handed over when it made the object. A mask written back with
/// [`Command::Mask`] is indexed like `positions` here.
#[derive(Clone, Copy)]
pub struct MeshView<'a> {
    pub id: MeshId,
    pub positions: &'a [Vec3],
    pub normals: &'a [Vec3],
    /// Three per triangle, counter-clockwise seen from outside.
    pub indices: &'a [u32],
    /// 0 = free, 1 = fully masked, one per vertex.
    pub mask: &'a [f32],
    /// One flag per triangle (true = hidden) in the order of `indices`, or
    /// `None` while nothing of the mesh is hidden. Hidden faces are not
    /// drawn, not hit by the pointer and not touched by brushes, but they
    /// stay in `indices` and every other array here, so vertex and triangle
    /// numbers mean the same before and after a hide. This is what
    /// `sculpt_cut::select::plan_hide` takes. Change it with
    /// [`Command::HideFaces`].
    pub hidden_triangles: Option<&'a [bool]>,
    /// One flag per vertex, or `None` while nothing is hidden. A vertex is
    /// hidden only when every triangle on it is, so the vertices along the
    /// edge of a hidden area are not.
    pub hidden_vertices: Option<&'a [bool]>,
}

impl MeshView<'_> {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// How many triangles are showing: all of them unless some are hidden.
    pub fn visible_triangle_count(&self) -> usize {
        match self.hidden_triangles {
            Some(hidden) => hidden.iter().filter(|&&h| !h).count(),
            None => self.triangle_count(),
        }
    }

    pub fn is_triangle_hidden(&self, triangle: usize) -> bool {
        self.hidden_triangles.is_some_and(|hidden| hidden.get(triangle).copied().unwrap_or(false))
    }

    pub fn is_vertex_hidden(&self, vertex: usize) -> bool {
        self.hidden_vertices.is_some_and(|hidden| hidden.get(vertex).copied().unwrap_or(false))
    }

    /// A copy of the triangles as plain data, for geometry code that needs to
    /// own or change them (and for sending to a background job, which cannot
    /// borrow from the scene). This is the one place a whole mesh is copied;
    /// do it when a job starts, not every frame. Hidden faces are copied
    /// like the rest: hiding changes what is seen, not what the model is.
    pub fn to_tri_mesh(&self) -> TriMesh {
        TriMesh::new(self.positions.to_vec(), self.indices.to_vec())
    }
}

/// Where a ray met the scene, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub object: ObjectId,
    pub position: Vec3,
    /// Surface normal, facing the ray's origin.
    pub normal: Vec3,
    /// Distance from the ray's origin.
    pub distance: f32,
}

/// The scene as a tool reads it.
pub trait Scene {
    /// Every object, in list order (top row first).
    fn objects(&self) -> Vec<ObjectId>;

    fn info(&self, id: ObjectId) -> Option<ObjectInfo<'_>>;

    /// The selected object: the one brushes work on. There always is one.
    /// When several are selected it is the bright row of the list.
    fn active(&self) -> ObjectId;

    /// Every selected object, [`Scene::active`] first, then the others in
    /// the order they were added to the selection (Ctrl + click on a row,
    /// Shift or Ctrl + click in the viewport with the Transform tool up).
    /// Never empty. A tool that takes two operands reads them here: the
    /// active object and the next one.
    fn selected(&self) -> Vec<ObjectId> {
        vec![self.active()]
    }

    /// An object's geometry in its own space.
    fn mesh(&self, id: ObjectId) -> Option<MeshView<'_>>;

    /// The nearest visible surface along a world ray.
    fn raycast(&self, ray: &Ray) -> Option<Hit>;

    /// Where a world ray meets one object, shown or hidden.
    fn raycast_object(&self, id: ObjectId, ray: &Ray) -> Option<Hit>;

    /// A world-space box around an object. Quick (no pass over the
    /// vertices), and exact for an object that is not turned; for a turned
    /// one it is the box around its own box, so loose by up to 40%. Measure
    /// from the vertices when that matters. Empty for an unknown id.
    fn bounds(&self, id: ObjectId) -> Aabb;

    /// The selected object's row.
    fn active_info(&self) -> Option<ObjectInfo<'_>> {
        self.info(self.active())
    }

    /// The selected object's geometry.
    fn active_mesh(&self) -> Option<MeshView<'_>> {
        self.mesh(self.active())
    }

    fn exists(&self, id: ObjectId) -> bool {
        self.info(id).is_some()
    }
}

/// A mesh the app has already turned into its own sculptable form, off the
/// interface thread. Made by `JobCtx::bake`; opaque to tools.
pub struct Baked(Box<dyn Any + Send>);

impl Baked {
    /// For the host: wraps its own mesh type.
    pub fn new<T: Any + Send>(mesh: T) -> Baked {
        Baked(Box::new(mesh))
    }

    /// For the host: takes its own mesh type back out.
    pub fn take<T: Any>(self) -> Option<T> {
        self.0.downcast::<T>().ok().map(|b| *b)
    }
}

impl fmt::Debug for Baked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Baked(..)")
    }
}

/// Geometry for a new or replaced object.
#[derive(Debug)]
pub enum MeshData {
    /// Plain triangles, with an optional mask (one value per vertex of
    /// `mesh`). The app builds its sculpting structures from them when the
    /// command is carried out, on the interface thread: fine for a preview-
    /// sized mesh, a visible pause for millions of triangles.
    Plain { mesh: TriMesh, mask: Option<Vec<f32>> },
    /// Built ahead of time inside a background job; carrying it out is
    /// instant.
    Baked(Baked),
}

impl MeshData {
    pub fn masked(mesh: TriMesh, mask: Vec<f32>) -> MeshData {
        MeshData::Plain { mesh, mask: Some(mask) }
    }
}

impl From<TriMesh> for MeshData {
    fn from(mesh: TriMesh) -> MeshData {
        MeshData::Plain { mesh, mask: None }
    }
}

impl From<Baked> for MeshData {
    fn from(baked: Baked) -> MeshData {
        MeshData::Baked(baked)
    }
}

/// An object to add to the scene.
#[derive(Debug)]
pub struct NewObject {
    /// The app makes it unique ("Tube", "Tube (2)"...).
    pub name: String,
    pub mesh: MeshData,
    pub placement: Placement,
    pub visible: bool,
    /// Whether it becomes the selected object. On by default: something just
    /// made is nearly always what is worked on next.
    pub select: bool,
}

impl NewObject {
    pub fn new(name: impl Into<String>, mesh: impl Into<MeshData>) -> NewObject {
        NewObject { name: name.into(), mesh: mesh.into(), placement: Placement::IDENTITY, visible: true, select: true }
    }

    pub fn placed(mut self, placement: Placement) -> NewObject {
        self.placement = placement;
        self
    }

    pub fn unselected(mut self) -> NewObject {
        self.select = false;
        self
    }

    pub fn hidden(mut self) -> NewObject {
        self.visible = false;
        self
    }
}

/// One change to the scene. Hand a list of them to `Ctx::commit`.
#[derive(Debug)]
pub enum Command {
    /// Add an object at the bottom of the list.
    Add(NewObject),
    /// Give an object new geometry. Its instances follow. The old geometry
    /// is kept for undo.
    Replace { object: ObjectId, mesh: MeshData },
    /// Move, turn or resize an object.
    Place { object: ObjectId, placement: Placement },
    /// Show or hide an object.
    Show { object: ObjectId, visible: bool },
    Rename { object: ObjectId, name: String },
    /// Remove an object. The last one left cannot be removed.
    Delete { object: ObjectId },
    /// Make an object the selected one. Selecting alone is not an undo step,
    /// as when the user clicks a row.
    Select { object: ObjectId },
    /// Set an object's mask, one value per vertex in the order `Scene::mesh`
    /// lends them. Always an undo step of its own (see `Ctx::commit`), and
    /// the object becomes the selected one.
    Mask { object: ObjectId, mask: Vec<f32> },
    /// Hide these triangles of an object's mesh, or with `hide` false show
    /// them again. The numbers are triangle numbers: triangle `t` is
    /// `MeshView::indices[3 * t..3 * t + 3]`; ones past the end are ignored
    /// and a repeat counts once. Instances share a mesh, so they follow.
    ///
    /// Like a mask, it is an undo step of its own, applied after the rest
    /// of the commit, and the object becomes the selected one. It is counted
    /// as failed, and leaves no step, when it changes nothing or when it
    /// would hide every face of the object: an object that has vanished
    /// gives no sign that it is only hidden.
    HideFaces { object: ObjectId, triangles: Vec<u32>, hide: bool },
    /// Show every hidden face of an object again. A step of its own, as
    /// above; counted as failed when nothing was hidden.
    ShowAllFaces { object: ObjectId },
    /// Swap what is hidden and what shows on an object. A step of its own,
    /// as above; counted as failed when nothing is hidden, since the swap
    /// would hide the whole object.
    InvertFaces { object: ObjectId },
}

/// What came of a commit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Receipt {
    /// The objects the `Add` commands made, in command order.
    pub added: Vec<ObjectId>,
    /// How many commands could not be carried out (an id that no longer
    /// exists, a mask of the wrong length, deleting the last object, a hide
    /// that changes nothing or would hide the whole object).
    pub failed: usize,
    /// Whether anything was changed, so whether an undo step was recorded.
    pub changed: bool,
}

/// The app's side of a commit: the handful of things it must be able to do
/// to its document. Tool crates never call these; they go through
/// `Ctx::commit`, which is [`apply`].
///
/// Everything between `begin` and `end` is one edit of the object list and
/// must come back as one undo step. `mask` and the three calls for hidden
/// faces are made outside that pair and are each a step of their own.
pub trait Document: Scene {
    /// An edit of the object list starts. `label` names it for the user.
    fn begin(&mut self, label: &str);
    fn add(&mut self, object: NewObject) -> Option<ObjectId>;
    fn replace(&mut self, object: ObjectId, mesh: MeshData) -> bool;
    fn place(&mut self, object: ObjectId, placement: Placement) -> bool;
    fn show(&mut self, object: ObjectId, visible: bool) -> bool;
    fn rename(&mut self, object: ObjectId, name: String) -> bool;
    fn delete(&mut self, object: ObjectId) -> bool;
    fn select(&mut self, object: ObjectId) -> bool;
    /// The edit is over. Returns whether it left an undo step.
    fn end(&mut self) -> bool;
    /// Sets an object's mask as an undo step of its own. Returns whether the
    /// mask was taken.
    fn mask(&mut self, object: ObjectId, mask: Vec<f32>) -> bool;

    /// Hides (or with `hide` false shows) triangles of an object's mesh as an
    /// undo step of its own. Returns whether anything changed; a hide that
    /// would leave none of the object showing must be refused. A document
    /// that cannot hide faces changes nothing.
    fn hide_faces(&mut self, _object: ObjectId, _triangles: &[u32], _hide: bool) -> bool {
        false
    }

    /// Shows every hidden face of an object again, as an undo step of its
    /// own. Returns whether anything was hidden.
    fn show_all_faces(&mut self, _object: ObjectId) -> bool {
        false
    }

    /// Swaps hidden and showing faces of an object, as an undo step of its
    /// own. Returns false, and does nothing, when nothing is hidden.
    fn invert_faces(&mut self, _object: ObjectId) -> bool {
        false
    }
}

/// Carries out `commands` on a document, in order.
///
/// Everything that changes the object list (add, replace, place, show,
/// rename, delete, select) happens inside one `begin`/`end` pair, so however
/// many objects a tool touches, Z takes it all back at once. Masks and
/// hidden faces are set after that, each as its own step, in the order given:
/// they are stored with the mesh they belong to and undone like a brush
/// stroke, so they cannot ride in the same step. A command that cannot be
/// carried out is counted in `Receipt::failed` and the rest still run.
pub fn apply(doc: &mut dyn Document, label: &str, commands: Vec<Command>) -> Receipt {
    let mut receipt = Receipt::default();
    let mut on_meshes = Vec::new();
    let mut listed = false;
    for command in commands {
        if matches!(
            command,
            Command::Mask { .. } | Command::HideFaces { .. } | Command::ShowAllFaces { .. } | Command::InvertFaces { .. }
        ) {
            on_meshes.push(command);
            continue;
        }
        if !listed {
            doc.begin(label);
            listed = true;
        }
        let ok = match command {
            Command::Add(object) => match doc.add(object) {
                Some(id) => {
                    receipt.added.push(id);
                    true
                }
                None => false,
            },
            Command::Replace { object, mesh } => doc.replace(object, mesh),
            Command::Place { object, placement } => doc.place(object, placement),
            Command::Show { object, visible } => doc.show(object, visible),
            Command::Rename { object, name } => doc.rename(object, name),
            Command::Delete { object } => doc.delete(object),
            Command::Select { object } => doc.select(object),
            Command::Mask { .. }
            | Command::HideFaces { .. }
            | Command::ShowAllFaces { .. }
            | Command::InvertFaces { .. } => unreachable!("commands on a mesh were set aside above"),
        };
        if !ok {
            receipt.failed += 1;
        }
    }
    if listed {
        receipt.changed |= doc.end();
    }
    for command in on_meshes {
        let done = match command {
            Command::Mask { object, mask } => doc.mask(object, mask),
            Command::HideFaces { object, triangles, hide } => doc.hide_faces(object, &triangles, hide),
            Command::ShowAllFaces { object } => doc.show_all_faces(object),
            Command::InvertFaces { object } => doc.invert_faces(object),
            _ => unreachable!("only commands on a mesh were set aside"),
        };
        if done {
            receipt.changed = true;
        } else {
            receipt.failed += 1;
        }
    }
    receipt
}
