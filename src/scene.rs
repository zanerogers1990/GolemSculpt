//! The scene: an ordered list of named objects. Each object is a placement of
//! a mesh; instances are simply objects that share one mesh, so sculpting any
//! of them changes all. An object can also have live mirror copies of itself:
//! not objects, but its own mesh drawn again (see `mirror.rs`).
//!
//! The mesh of the active object does not live here. It is `Doc::mesh`, so
//! the sculpting code always has the mesh it works on to hand. Every other
//! mesh is parked in this struct, and the methods that change which object
//! is active take that slot and swap the meshes over.
//!
//! Several objects can be selected at once. One of them is the active one:
//! the object brushes work on, the one the gizmo sits on and the one a Join
//! keeps. The others are along for whatever acts on the whole selection
//! (Join, Frame, the hide keys, the gizmo). `selected()` lists them all.

use std::collections::HashMap;

use glam::{Mat3, Mat4, Quat, Vec3};
use sculpt_geom::TriMesh;
use sculpt_symmetry::MirrorRepeater;

use crate::mesh::bvh::Aabb;
use crate::mesh::{Mesh, Polys, StrokeDelta};

mod mirror;
mod places;
mod symmetry;

pub use mirror::{MirrorCommand, saved_mirror};
pub use places::TOOL_COPY;
pub use symmetry::{Space, Symmetry, saved_symmetry};

/// New shapes are this fraction of the top object's average half-size, as in
/// the Unity app: on a par with what is there without being its twin.
const NEW_SHAPE_FRACTION: f32 = 0.9;
/// Half-size of a new shape when there is nothing to measure it against.
const NEW_SHAPE_FALLBACK: f32 = 0.15;

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    /// Which mesh this object shows. Shared by its instances.
    pub mesh: u64,
    pub position: Vec3,
    pub rotation: Quat,
    /// Size along the object's own axes.
    pub scale: Vec3,
    /// Live mirror copies of the object across the world's planes, drawn from
    /// its own mesh (see `scene/mirror.rs`). Not shared with instances.
    pub mirror: Option<MirrorRepeater>,
}

impl Object {
    /// The object's own space into world space.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

/// The object list at one moment; what an undo step of a scene edit keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneState {
    pub objects: Vec<Object>,
    pub active: u64,
    /// Every selected object, the active one among them.
    pub selected: Vec<u64>,
}

/// Where a ray met the scene, in world space.
#[derive(Clone, Copy, Debug)]
pub struct Picked {
    /// The object that was hit.
    pub id: u64,
    pub pos: Vec3,
    /// Surface normal, facing the ray origin.
    #[allow(dead_code)]
    pub normal: Vec3,
    /// Which place of the object the ray met: 0 for the object itself, the
    /// flip mask (X = 1, Y = 2, Z = 4) of one of its live mirror copies, or
    /// `TOOL_COPY` and up for a copy a tool keeps (see `scene/places.rs`).
    pub copy: u16,
}

/// What becomes of the other selected objects once the active one has taken
/// a mesh made from all of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Others {
    /// A join: they are part of the result now.
    Deleted,
    /// A boolean: out of the viewport, where they would sit exactly in the
    /// hole they cut, but kept. The eye in the list shows one again.
    Hidden,
}

/// Local View while it is on.
struct LocalView {
    /// What it hid on the way in, so leaving shows exactly those again.
    hidden: Vec<u64>,
    /// Camera pivot and distance on the way in, for the glide back.
    back: (Vec3, f32),
}

/// The shape a mesh had before it was sculpted, for Reset Shape.
struct Baseline {
    /// `Mesh::uid` of the mesh these belong to. A remesh builds a new mesh,
    /// whose vertices these say nothing about.
    uid: u64,
    positions: Vec<Vec3>,
}

pub struct Scene {
    /// In list order, top row first.
    pub objects: Vec<Object>,
    active: u64,
    /// The selected objects in the order they were picked. The active one is
    /// always among them.
    selected: Vec<u64>,
    /// Every mesh except the active object's.
    parked: HashMap<u64, Mesh>,
    next_id: u64,
    /// The placement of the active mesh a stroke is going through, when that
    /// is not the active object itself: an instance of it, or a live mirror
    /// copy or a copy a tool keeps (the second number names the place: 0 for
    /// the object, a flip mask, or `TOOL_COPY` and up).
    brush: Option<(u64, u16)>,
    /// The same for the last stroke, which the orbit pivot is measured from.
    stroked: Option<(u64, u16)>,
    /// The copies tools keep of objects, by object, as they last said (see
    /// `scene/places.rs`). Not saved and not an undo step.
    tool_places: HashMap<u64, Vec<Mat4>>,
    /// Which side of the active object is selected when it is mirrored: the
    /// flip mask of the live copy the gizmo is on, with the object it was
    /// chosen for. Not an undo step; see `selected_side`.
    side: Option<(u64, u8)>,
    local_view: Option<LocalView>,
    /// Visibility set with the hide keys or Local View. Those are not undo
    /// steps, so stepping across an older edit must not bring the old
    /// visibility back: `restore` puts these on top of what it restores.
    shown: HashMap<u64, bool>,
    /// How many shapes of each name have been added, for "Cube (1)".
    spawned: HashMap<String, u32>,
    /// By mesh. Only meshes that have been sculpted since they were built
    /// have one: until its first stroke a mesh is its own resting shape.
    baselines: HashMap<u64, Baseline>,
    /// Each object's own symmetry settings, by object (see `symmetry.rs`,
    /// which also says why they are here and not in `Object`). An object
    /// without an entry has none set. An entry outlives its object, because
    /// undo can bring the object back and it is a few bytes.
    symmetry: HashMap<u64, Symmetry>,
}

/// An independent copy of a mesh with everything the user can see of it: its
/// mask, the faces hidden with the box or lasso, and the quads a quad remesh
/// left. (Building a mesh reorders its vertices and triangles, so the hidden
/// flags and the quads' corners are carried over through the same renumbering.)
fn duplicate(mesh: &Mesh) -> Mesh {
    let hidden = mesh.hidden_triangles().unwrap_or(&[]);
    let (mut copy, moved) = Mesh::new_mapped_with_hidden(
        mesh.positions.clone(),
        mesh.indices.clone(),
        Some(mesh.mask.clone()),
        hidden,
    );
    if let Some(polys) = &mesh.polys {
        let corners: Vec<u32> =
            polys.indices.iter().map(|&i| moved.get(i as usize).copied().unwrap_or(u32::MAX)).collect();
        if !corners.contains(&u32::MAX) {
            copy.polys = Some(Polys { start: polys.start.clone(), indices: corners });
        }
    }
    copy
}

impl Scene {
    /// A scene holding one object, whose mesh is the one in `Doc::mesh`.
    pub fn new(name: &str) -> Scene {
        let first = Object {
            id: 1,
            name: name.to_owned(),
            visible: true,
            mesh: 2,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            mirror: None,
        };
        Scene {
            objects: vec![first],
            active: 1,
            selected: vec![1],
            parked: HashMap::new(),
            next_id: 3,
            brush: None,
            stroked: None,
            tool_places: HashMap::new(),
            side: None,
            local_view: None,
            shown: HashMap::new(),
            // The object the scene starts with counts as the first of its name.
            spawned: HashMap::from([(name.to_owned(), 1)]),
            baselines: HashMap::new(),
            symmetry: HashMap::new(),
        }
    }

    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id - 1
    }

    pub fn active_id(&self) -> u64 {
        self.active
    }

    pub fn active(&self) -> &Object {
        self.get(self.active).unwrap_or(&self.objects[0])
    }

    pub fn active_mut(&mut self) -> &mut Object {
        let i = self.index_of(self.active).unwrap_or(0);
        &mut self.objects[i]
    }

    pub fn active_mesh(&self) -> u64 {
        self.active().mesh
    }

    pub fn get(&self, id: u64) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.objects.iter().position(|o| o.id == id)
    }

    /// How many objects show this mesh. More than one means instances.
    pub fn users(&self, mesh: u64) -> usize {
        self.objects.iter().filter(|o| o.mesh == mesh).count()
    }

    pub fn state(&self) -> SceneState {
        SceneState { objects: self.objects.clone(), active: self.active, selected: self.selected.clone() }
    }

    /// The mesh with this id; `slot` is `Doc::mesh`.
    pub fn mesh<'a>(&'a self, id: u64, slot: &'a Mesh) -> Option<&'a Mesh> {
        if id == self.active_mesh() { Some(slot) } else { self.parked.get(&id) }
    }

    /// The parked meshes, for the renderer to keep its copies current.
    pub fn parked_mut(&mut self) -> impl Iterator<Item = (u64, &mut Mesh)> {
        self.parked.iter_mut().map(|(&id, mesh)| (id, mesh))
    }

    /// `base`, or `base (2)`, `base (3)`... if that name is taken.
    pub fn unique_name(&self, base: &str) -> String {
        let taken = |name: &str| self.objects.iter().any(|o| o.name == name);
        if !taken(base) {
            return base.to_owned();
        }
        (2..).map(|n| format!("{base} ({n})")).find(|name| !taken(name)).unwrap()
    }

    /// The name for the next new shape called `base`, counting up the way the
    /// Unity app does: "Cube", then "Cube (1)", "Cube (2)"... A number is
    /// never handed out twice, even after the shape that had it is deleted,
    /// and one already in use (through a rename, say) is skipped.
    pub fn spawn_name(&mut self, base: &str) -> String {
        let count = self.spawned.entry(base.to_owned()).or_insert(0);
        loop {
            let name = if *count == 0 { base.to_owned() } else { format!("{base} ({count})") };
            *count += 1;
            if !self.objects.iter().any(|o| o.name == name) {
                return name;
            }
        }
    }

    /// Half-size for a new shape: a little under the top object's, so it
    /// arrives at a usable size beside what is already there instead of
    /// dwarfing it or vanishing next to it. The Unity app's rule.
    pub fn new_shape_size(&self, slot: &Mesh) -> f32 {
        let size = self.objects.first().and_then(|main| {
            let bounds = self.mesh(main.mesh, slot)?.bounds();
            // The mesh's own box, at the size the object is shown.
            let half = bounds.size() * 0.5 * main.scale.abs();
            (!bounds.is_empty()).then_some((half.x + half.y + half.z) / 3.0 * NEW_SHAPE_FRACTION)
        });
        size.filter(|s| s.is_finite() && *s > 1e-5).unwrap_or(NEW_SHAPE_FALLBACK)
    }

    /// Makes the object using mesh `to` the one in the slot.
    fn swap_in(&mut self, from: u64, to: u64, slot: &mut Mesh) {
        if from == to {
            return;
        }
        if let Some(mesh) = self.parked.remove(&to) {
            self.parked.insert(from, std::mem::replace(slot, mesh));
        }
    }

    /// Makes `id` the active object and brings its mesh to hand. Leaves the
    /// selection to the caller.
    fn activate(&mut self, id: u64, slot: &mut Mesh) {
        let Some(to) = self.get(id).map(|o| o.mesh) else { return };
        let from = self.active_mesh();
        self.active = id;
        (self.brush, self.stroked) = (None, None);
        self.swap_in(from, to, slot);
    }

    /// Selects one object, dropping any others from the selection.
    pub fn select(&mut self, id: u64, slot: &mut Mesh) {
        if self.get(id).is_none() {
            return;
        }
        self.activate(id, slot);
        self.selected = vec![id];
        self.side = None;
    }

    /// Every selected object, in the order they were picked. Never empty: the
    /// active object is always one of them.
    pub fn selected(&self) -> &[u64] {
        &self.selected
    }

    pub fn is_selected(&self, id: u64) -> bool {
        self.selected.contains(&id)
    }

    /// Adds an object to the selection without changing which one is active
    /// (Ctrl + click on a row). It only ever adds: the active object is the
    /// one a Join keeps, so a second click must not quietly take an object
    /// back out of the merge, nor move the survivor. Returns whether it added.
    pub fn select_also(&mut self, id: u64) -> bool {
        if self.get(id).is_none() || self.is_selected(id) {
            return false;
        }
        self.selected.push(id);
        true
    }

    /// Adds an object to the selection, or drops it if it was in there (Shift
    /// + click in the viewport). Dropping the active object promotes the
    /// earliest-picked of the rest; the last one selected cannot be dropped.
    pub fn toggle_selected(&mut self, id: u64, slot: &mut Mesh) -> bool {
        if self.get(id).is_none() {
            return false;
        }
        match self.selected.iter().position(|&s| s == id) {
            Some(_) if self.selected.len() < 2 => return false,
            Some(at) => {
                self.selected.remove(at);
                if id == self.active {
                    self.activate(self.selected[0], slot);
                }
            }
            None => self.selected.push(id),
        }
        true
    }

    /// Puts the selection right after the object list or the active object
    /// changed under it: only objects that exist, and the active one always.
    fn mend_selection(&mut self) {
        let objects = &self.objects;
        self.selected.retain(|id| objects.iter().any(|o| o.id == *id));
        if !self.selected.contains(&self.active) {
            self.selected.insert(0, self.active);
        }
    }

    /// World bounds of everything selected, live mirror copies included.
    pub fn selection_bounds(&self, slot: &Mesh) -> Aabb {
        self.selected
            .iter()
            .filter_map(|&id| self.get(id))
            .fold(Aabb::EMPTY, |all, o| Aabb::union(all, self.shown_bounds_of(o, slot)))
    }

    /// Makes sure `mesh` is the one in the slot, selecting an object that
    /// shows it if the active one does not.
    pub fn focus_mesh(&mut self, mesh: u64, slot: &mut Mesh) {
        if self.active_mesh() != mesh {
            if let Some(id) = self.objects.iter().find(|o| o.mesh == mesh).map(|o| o.id) {
                self.select(id, slot);
            }
        }
    }

    /// Adds an object with its own mesh at the bottom of the list and
    /// selects it.
    pub fn add(&mut self, name: &str, mesh: Mesh, slot: &mut Mesh) -> u64 {
        let (id, mesh_id) = (self.fresh_id(), self.fresh_id());
        let name = self.unique_name(name);
        self.parked.insert(mesh_id, mesh);
        self.objects.push(Object {
            id,
            name,
            visible: true,
            mesh: mesh_id,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            mirror: None,
        });
        self.select(id, slot);
        id
    }

    /// An independent copy of the active object, on top of it, sculpted
    /// under the same symmetry.
    pub fn clone_active(&mut self, slot: &mut Mesh) -> u64 {
        let source = self.active().clone();
        let id = self.add(&format!("{} Copy", source.name), duplicate(slot), slot);
        self.place_after(id, &source);
        self.inherit_symmetry(source.id, id);
        id
    }

    /// A linked copy of the active object: same mesh, its own place, name
    /// and visibility.
    pub fn instance_active(&mut self) -> u64 {
        let source = self.active().clone();
        let id = self.fresh_id();
        // An instance of an instance is named after the original.
        let base = source.name.split(" Instance").next().unwrap_or(&source.name);
        let name = self.unique_name(&format!("{base} Instance"));
        // The live mirror belongs to the object it was made for, not to its instances.
        self.objects.push(Object { id, name, visible: true, mirror: None, ..source.clone() });
        self.active = id;
        self.selected = vec![id];
        (self.brush, self.stroked) = (None, None);
        self.place_after(id, &source);
        // The symmetry is the mesh's, whichever of its objects is selected.
        self.inherit_symmetry(source.id, id);
        id
    }

    /// Puts a new copy where its source is, and directly under it in the list.
    fn place_after(&mut self, id: u64, source: &Object) {
        if let Some(o) = self.get_mut(id) {
            (o.position, o.rotation, o.scale) = (source.position, source.rotation, source.scale);
        }
        if let Some(row) = self.index_of(source.id) {
            self.move_to(id, row + 1);
        }
    }

    /// Gives the active object its own copy of a mesh it shares.
    pub fn make_unique(&mut self, slot: &mut Mesh) -> bool {
        let old = self.active_mesh();
        if self.users(old) < 2 {
            return false;
        }
        let new = self.fresh_id();
        let copy = duplicate(slot);
        self.parked.insert(old, std::mem::replace(slot, copy));
        self.active_mut().mesh = new;
        true
    }

    /// Removes an object. The last one cannot be removed. Its mesh stays
    /// parked until `collect` finds nothing refers to it.
    pub fn remove(&mut self, id: u64, slot: &mut Mesh) -> bool {
        let Some(row) = self.index_of(id) else { return false };
        if self.objects.len() < 2 {
            return false;
        }
        self.selected.retain(|&s| s != id);
        if id == self.active {
            // Another selected object takes over; failing that the nearest
            // row that is shown (below first), so a hidden object is not left
            // as the one brushes work on; failing that, simply a neighbour.
            let shown_below = self.objects[row + 1..].iter().find(|o| o.visible);
            let shown_above = self.objects[..row].iter().rev().find(|o| o.visible);
            let neighbour = self.objects.get(row + 1).or_else(|| self.objects[..row].last());
            let next = self.selected.first().copied().or_else(|| shown_below.or(shown_above).or(neighbour).map(|o| o.id));
            if let Some(next) = next {
                self.activate(next, slot);
            }
        }
        self.objects.remove(row);
        self.mend_selection();
        true
    }

    /// Moves an object to row `to` of the list.
    pub fn move_to(&mut self, id: u64, to: usize) -> bool {
        let Some(from) = self.index_of(id) else { return false };
        let to = to.min(self.objects.len() - 1);
        if from == to {
            return false;
        }
        let object = self.objects.remove(from);
        self.objects.insert(to, object);
        true
    }

    /// Replaces the object list and the selection with `state` and returns
    /// the old ones.
    pub fn restore(&mut self, state: SceneState, slot: &mut Mesh) -> SceneState {
        self.forget_stale_visibility();
        let from = self.active_mesh();
        let old = SceneState {
            objects: std::mem::replace(&mut self.objects, state.objects),
            active: self.active,
            selected: std::mem::replace(&mut self.selected, state.selected),
        };
        self.active = state.active;
        if self.get(self.active).is_none() {
            self.active = self.objects[0].id;
        }
        self.mend_selection();
        (self.brush, self.stroked) = (None, None);
        // What the hide keys last set stays as the user left it.
        for o in &mut self.objects {
            if let Some(&visible) = self.shown.get(&o.id) {
                o.visible = visible;
            }
        }
        let to = self.active_mesh();
        self.swap_in(from, to, slot);
        old
    }

    /// Replaces the whole scene with loaded objects; `objects[i].mesh` indexes
    /// `meshes`. The old meshes stay parked for undo.
    pub fn load(&mut self, meshes: Vec<Mesh>, mut objects: Vec<Object>, active: usize, slot: &mut Mesh) {
        if meshes.is_empty() || objects.is_empty() {
            return;
        }
        let from = self.active_mesh();
        let mut ids = Vec::with_capacity(meshes.len());
        for mesh in meshes {
            let id = self.fresh_id();
            self.parked.insert(id, mesh);
            ids.push(id);
        }
        for o in &mut objects {
            o.id = self.fresh_id();
            o.mesh = ids[(o.mesh as usize).min(ids.len() - 1)];
        }
        self.active = objects[active.min(objects.len() - 1)].id;
        self.selected = vec![self.active];
        self.objects = objects;
        // Local View and the hide keys belonged to the scene that was open.
        self.local_view = None;
        self.shown.clear();
        (self.brush, self.stroked) = (None, None);
        let to = self.active_mesh();
        self.swap_in(from, to, slot);
    }

    /// Drops parked meshes that no object shows and `keep` does not want.
    pub fn collect(&mut self, keep: impl Fn(u64) -> bool) {
        let objects = &self.objects;
        let wanted = |id: u64| objects.iter().any(|o| o.mesh == id) || keep(id);
        self.parked.retain(|&id, _| wanted(id));
        self.baselines.retain(|&id, _| wanted(id));
        self.forget_stale_visibility();
    }

    /// The meshes no object shows, each with the bytes it holds (its resting
    /// shape for Reset Shape included). They are parked here only because an
    /// undo step may still bring one of their objects back, so they are what
    /// undo is charged for (`History::settle`). The mesh of an object in the
    /// list is not among them, however many steps also show it, and a mesh
    /// several objects shared is here once.
    pub fn spare_meshes(&self) -> HashMap<u64, usize> {
        let resting = |id: &u64| self.baselines.get(id).map_or(0, |b| std::mem::size_of_val(b.positions.as_slice()));
        let spare = self.parked.iter().filter(|(id, _)| self.users(**id) == 0);
        spare.map(|(id, mesh)| (*id, mesh.memory_bytes() + resting(id))).collect()
    }

    // ---- join and booleans -------------------------------------------------

    /// The selected objects and their meshes: the active one first, then the
    /// rest in the order they were picked.
    fn selected_sources<'a>(&'a self, slot: &'a Mesh) -> Vec<(&'a Object, &'a Mesh)> {
        let others = self.selected.iter().filter(|&&id| id != self.active).filter_map(|&id| self.get(id));
        std::iter::once(self.active()).chain(others).filter_map(|o| Some((o, self.mesh(o.mesh, slot)?))).collect()
    }

    /// The selection as the solids of a boolean, all in the active object's
    /// own space: the active object's mesh as it is (the target), and each
    /// other selected object's with its placement baked into its vertices
    /// (the cutters). A cutter that is mirrored comes through with its
    /// triangles turned back the right way out, or it would add where it
    /// should cut. `None` unless two or more are selected.
    pub fn boolean_solids(&self, slot: &Mesh) -> Option<(TriMesh, Vec<TriMesh>)> {
        let sources = self.selected_sources(slot);
        let ((target, mesh), others) = sources.split_first().filter(|(_, others)| !others.is_empty())?;
        let into = target.matrix().inverse();
        let cutters = others.iter().map(|(o, m)| {
            let mut solid = TriMesh::new(m.positions.clone(), m.indices.clone());
            solid.transform(&(into * o.matrix()));
            solid
        });
        Some((TriMesh::new(mesh.positions.clone(), mesh.indices.clone()), cutters.collect()))
    }

    /// The selected objects as one mesh in the active object's own space, so
    /// it can take the active object's place without anything moving: plain
    /// concatenation, every shell kept as it is, each vertex keeping its
    /// mask. `None` unless two or more are selected.
    pub fn joined_mesh(&self, slot: &Mesh) -> Option<Mesh> {
        if self.selected.len() < 2 {
            return None;
        }
        let survivor = self.active();
        let into = survivor.matrix().inverse();
        let sources = self.selected_sources(slot);
        let shells: Vec<TriMesh> =
            sources.iter().map(|(_, m)| TriMesh::new(m.positions.clone(), m.indices.clone())).collect();
        // The survivor's own vertices are not moved at all, so a mesh that was
        // mirror-exact stays so.
        let place = |o: &Object| if o.id == survivor.id { Mat4::IDENTITY } else { into * o.matrix() };
        let parts: Vec<(&TriMesh, Mat4)> =
            shells.iter().zip(&sources).map(|(shell, (o, _))| (shell, place(o))).collect();
        let joined = sculpt_geom::join::join(&parts);
        // Each part's vertices arrive in a block, in their own order, so the
        // masks carry across the same way.
        let mut mask = Vec::with_capacity(joined.mesh.positions.len());
        for (_, m) in &sources {
            mask.extend_from_slice(&m.mask);
        }
        debug_assert_eq!(mask.len(), joined.mesh.positions.len());
        Some(Mesh::new(joined.mesh.positions, joined.mesh.indices, Some(mask)))
    }

    /// Puts `mesh` (see `joined_mesh`) in place of the selection: the active
    /// object keeps its row, name and placement and shows the new mesh, and
    /// the other selected objects go. It gets a mesh of its own, so instances
    /// that were not part of the join keep the shape they had. Returns how
    /// many objects went into it.
    pub fn install_joined(&mut self, mesh: Mesh, slot: &mut Mesh) -> usize {
        self.install_combined(mesh, Others::Deleted, slot)
    }

    /// Puts `mesh`, made from the whole selection, in place of the active
    /// object's: the active object keeps its row, name and placement, and the
    /// other selected objects are deleted or hidden. The new mesh is the
    /// active object's own, whatever it shared before, and is at rest as it
    /// is for Reset Shape. Only the active object stays selected. Returns how
    /// many objects were selected.
    ///
    /// Hiding the others here is part of the caller's undo step, unlike the
    /// hide keys: undoing a boolean has to bring its cutters back into view.
    pub fn install_combined(&mut self, mesh: Mesh, others: Others, slot: &mut Mesh) -> usize {
        let count = self.selected.len();
        let (old, new) = (self.active_mesh(), self.fresh_id());
        self.parked.insert(new, mesh);
        self.active_mut().mesh = new;
        self.swap_in(old, new, slot);
        let (active, selected) = (self.active, std::mem::take(&mut self.selected));
        let spent = |o: &Object| o.id != active && selected.contains(&o.id);
        match others {
            Others::Deleted => self.objects.retain(|o| !spent(o)),
            Others::Hidden => {
                for o in self.objects.iter_mut().filter(|o| spent(o)) {
                    o.visible = false;
                    self.shown.remove(&o.id);
                }
            }
        }
        self.selected = vec![active];
        (self.brush, self.stroked) = (None, None);
        count
    }

    // ---- showing and hiding ------------------------------------------------

    /// Sets visibility the way the hide keys do: for good, whatever undo
    /// later steps across.
    fn show(&mut self, id: u64, visible: bool) {
        if let Some(o) = self.get_mut(id) {
            o.visible = visible;
            self.shown.insert(id, visible);
        }
    }

    /// Drops what the hide keys set for any object that something else has
    /// shown or hidden since (an eye button, a tool): that change was an undo
    /// step, and undo has to be free to take it back.
    fn forget_stale_visibility(&mut self) {
        let objects = &self.objects;
        self.shown.retain(|id, visible| objects.iter().find(|o| o.id == *id).is_none_or(|o| o.visible == *visible));
    }

    /// After something was hidden: hidden objects leave the selection, and
    /// if the active one went, the brushes move to an object that can be
    /// seen (a selected one first, else the first shown row). Sculpting an
    /// object you cannot see would be confusing. With nothing left to see,
    /// the selection stays where it was.
    fn back_off(&mut self, slot: &mut Mesh) {
        let shown = |scene: &Scene, id: u64| scene.get(id).is_some_and(|o| o.visible);
        let kept: Vec<u64> = self.selected.iter().copied().filter(|&id| shown(self, id)).collect();
        if !shown(self, self.active) {
            let next = kept.first().copied().or_else(|| self.objects.iter().find(|o| o.visible).map(|o| o.id));
            let Some(next) = next else {
                self.selected = vec![self.active];
                return;
            };
            self.activate(next, slot);
        }
        self.selected = kept;
        self.mend_selection();
    }

    /// Shows or hides one object, as its eye button does.
    pub fn toggle_visible(&mut self, id: u64, slot: &mut Mesh) -> bool {
        let Some(o) = self.get_mut(id) else { return false };
        o.visible = !o.visible;
        let hidden = !o.visible;
        // The eye is an undo step, so from here on the step speaks for it.
        self.shown.remove(&id);
        if hidden {
            self.back_off(slot);
        }
        true
    }

    /// H: hides every selected object. Returns how many it hid.
    pub fn hide_selected(&mut self, slot: &mut Mesh) -> usize {
        let ids: Vec<u64> =
            self.selected.iter().copied().filter(|&id| self.get(id).is_some_and(|o| o.visible)).collect();
        for &id in &ids {
            self.show(id, false);
        }
        if !ids.is_empty() {
            self.back_off(slot);
        }
        ids.len()
    }

    fn hide_others(&mut self) -> Vec<u64> {
        let ids: Vec<u64> =
            self.objects.iter().filter(|o| o.visible && !self.selected.contains(&o.id)).map(|o| o.id).collect();
        for &id in &ids {
            self.show(id, false);
        }
        ids
    }

    /// Shift + H: hides every shown object that is not selected. Returns how
    /// many it hid.
    pub fn hide_unselected(&mut self) -> usize {
        self.hide_others().len()
    }

    /// Alt + H: shows every hidden object, except what Local View set aside
    /// while it is on (leaving it brings those back). Returns how many it
    /// showed.
    pub fn unhide_all(&mut self) -> usize {
        let aside: &[u64] = self.local_view.as_ref().map_or(&[][..], |local| &local.hidden[..]);
        let ids: Vec<u64> =
            self.objects.iter().filter(|o| !o.visible && !aside.contains(&o.id)).map(|o| o.id).collect();
        for &id in &ids {
            self.show(id, true);
        }
        ids.len()
    }

    pub fn in_local_view(&self) -> bool {
        self.local_view.is_some()
    }

    /// Local View: solos the selection by hiding everything else, and
    /// remembers what it hid and where the camera was (`back`: its pivot and
    /// distance). False, and nothing changes, when it is already on or when
    /// nothing selected can be seen: there would be nothing to solo.
    pub fn enter_local_view(&mut self, back: (Vec3, f32)) -> bool {
        let any_shown = self.selected.iter().any(|&id| self.get(id).is_some_and(|o| o.visible));
        if self.local_view.is_some() || !any_shown {
            return false;
        }
        let hidden = self.hide_others();
        self.local_view = Some(LocalView { hidden, back });
        true
    }

    /// Leaves Local View: shows again what it hid that still exists and is
    /// still hidden, and returns where the camera was on the way in.
    /// Anything hidden before it, or inside it, stays hidden.
    pub fn exit_local_view(&mut self) -> Option<(Vec3, f32)> {
        let local = self.local_view.take()?;
        for id in local.hidden {
            if self.get(id).is_some_and(|o| !o.visible) {
                self.show(id, true);
            }
        }
        Some(local.back)
    }

    // ---- reset shape -------------------------------------------------------

    /// Call before a stroke moves the active mesh. The first time for a mesh
    /// it keeps the vertex positions as its resting shape, which is what
    /// Reset Shape goes back to; after that it does nothing. A mesh built
    /// since (a remesh, say) is a different mesh and rests where it was built.
    pub fn keep_baseline(&mut self, slot: &Mesh) {
        let id = self.active_mesh();
        if self.baselines.get(&id).is_none_or(|b| b.uid != slot.uid) {
            self.baselines.insert(id, Baseline { uid: slot.uid, positions: slot.positions.clone() });
        }
    }

    fn baseline(&self, slot: &Mesh) -> Option<&Baseline> {
        // Dynamic topology adds vertices after the ones the mesh was built
        // with, which are the ones kept here (see `Mesh::rest_shape`).
        let fits = |b: &&Baseline| b.uid == slot.uid && b.positions.len() <= slot.positions.len();
        self.baselines.get(&self.active_mesh()).filter(fits)
    }

    /// Whether the active mesh has been sculpted since it was built, so
    /// there may be something for Reset Shape to put back.
    pub fn can_reset(&self, slot: &Mesh) -> bool {
        self.baseline(slot).is_some()
    }

    /// Puts the active mesh back to its resting shape: how it was when it
    /// was made, loaded or last rebuilt. Returns the undo record, or `None`
    /// if it is at rest already.
    pub fn reset_shape(&self, slot: &mut Mesh) -> Option<StrokeDelta> {
        let rest = slot.rest_shape(&self.baseline(slot)?.positions)?;
        let verts: Vec<u32> =
            (0..rest.len() as u32).filter(|&v| slot.positions[v as usize] != rest[v as usize]).collect();
        if verts.is_empty() {
            return None;
        }
        let delta = StrokeDelta::Positions {
            before: verts.iter().map(|&v| slot.positions[v as usize]).collect(),
            after: verts.iter().map(|&v| rest[v as usize]).collect(),
            verts,
            topology: None,
        };
        slot.apply_delta(&delta, false);
        Some(delta)
    }

    // ---- which placement the brush works through ---------------------------

    /// Whether this object is the active one or an instance of it.
    pub fn shows_active_mesh(&self, id: u64) -> bool {
        self.get(id).is_some_and(|o| o.mesh == self.active_mesh())
    }

    /// Where a stroke is aimed, made safe: the object and which of its
    /// placements (0, the flip mask of one of its live mirror copies, or a
    /// copy a tool keeps). An aim that is no longer a placement of the active
    /// mesh is the active object's own.
    fn placement(&self, at: Option<(u64, u16)>) -> (&Object, u16) {
        let active = self.active();
        let found = at.and_then(|(id, copy)| Some((self.get(id)?, copy)));
        match found {
            Some((o, copy)) if o.mesh == active.mesh && self.has_place(o, copy) => (o, copy),
            _ => (active, 0),
        }
    }

    /// The object a stroke under way maps the pointer through: the active
    /// object, or the instance of it the stroke began on. (A live mirror copy
    /// is a placement of its object: see `brush_matrix`.) An instance draws
    /// the very same mesh through its own placement, so sculpting through
    /// that placement edits exactly the vertices seen under the pointer.
    pub fn brush_object(&self) -> &Object {
        self.placement(self.brush).0
    }

    /// The object the last stroke went through.
    pub fn stroke_object(&self) -> &Object {
        self.placement(self.stroked).0
    }

    /// The matrix the stroke under way maps the pointer through: the
    /// placement of the object it began on, mirror copies and the copies a
    /// tool keeps included.
    pub fn brush_matrix(&self) -> Mat4 {
        let (object, copy) = self.placement(self.brush);
        self.place_matrix(object, copy)
    }

    /// Which placement of `brush_object` the stroke under way is on: 0 for
    /// the object itself, else the flip mask of its live mirror copy or
    /// `TOOL_COPY` and up for a copy a tool keeps.
    pub fn brush_copy(&self) -> u16 {
        self.placement(self.brush).1
    }

    /// The same for the last stroke, which the orbit pivot is measured from.
    pub fn stroke_matrix(&self) -> Mat4 {
        let (object, copy) = self.placement(self.stroked);
        self.place_matrix(object, copy)
    }

    /// Sends the stroke about to begin through `id`, a placement of the
    /// active mesh. It stays there until `release_brush`: an instance and its
    /// original can touch, and a stroke that crossed from one to the other
    /// must not change placement half way.
    /// `copy` is 0 for the object itself, the flip mask of one of its live
    /// mirror copies, or `TOOL_COPY` and up for a copy a tool keeps (an
    /// array's), as `Picked::copy` says.
    pub fn aim_brush(&mut self, id: u64, copy: u16) {
        let own = id == self.active && copy == 0;
        self.brush = (!own && self.shows_active_mesh(id)).then_some((id, copy));
        self.stroked = self.brush;
    }

    /// The stroke is over: back to the active object's own placement. True
    /// if it had been going through an instance or a mirror copy.
    pub fn release_brush(&mut self) -> bool {
        self.brush.take().is_some()
    }

    // ---- bounds and picking ------------------------------------------------

    /// World bounds of one object.
    pub fn bounds_of(&self, object: &Object, slot: &Mesh) -> Aabb {
        let Some(mesh) = self.mesh(object.mesh, slot) else { return Aabb::EMPTY };
        let b = mesh.bounds();
        if b.is_empty() {
            return b;
        }
        // The box around the mesh's own box, once that is turned and scaled.
        mirror::box_through(b, &object.matrix())
    }

    /// World bounds of everything shown, live mirror copies included.
    pub fn bounds(&self, slot: &Mesh) -> Aabb {
        self.objects
            .iter()
            .filter(|o| o.visible)
            .fold(Aabb::EMPTY, |all, o| Aabb::union(all, self.shown_bounds_of(o, slot)))
    }

    /// The nearest shown object along a world ray, and where the ray meets it.
    pub fn pick(&self, origin: Vec3, dir: Vec3, slot: &Mesh) -> Option<Picked> {
        self.pick_among(origin, dir, slot, |_| true)
    }

    /// The nearest shown placement of one mesh along a world ray: the object
    /// that has it or any of its instances, whatever else is in front.
    pub fn pick_mesh(&self, origin: Vec3, dir: Vec3, mesh: u64, slot: &Mesh) -> Option<Picked> {
        self.pick_among(origin, dir, slot, |o| o.mesh == mesh)
    }

    fn pick_among(&self, origin: Vec3, dir: Vec3, slot: &Mesh, wanted: impl Fn(&Object) -> bool) -> Option<Picked> {
        let mut best: Option<(Picked, f32)> = None;
        for o in self.objects.iter().filter(|o| o.visible && wanted(o)) {
            let Some(mesh) = self.mesh(o.mesh, slot) else { continue };
            // The object itself, then each of its live mirror copies and the
            // copies a tool keeps.
            for (copy, model) in self.places_of(o) {
                // The ray in the placement's own space, and the hit back out of it.
                let inverse = model.inverse();
                let local_dir = inverse.transform_vector3(dir).normalize_or(dir);
                if let Some(hit) = mesh.raycast(inverse.transform_point3(origin), local_dir) {
                    let pos = model.transform_point3(hit.pos);
                    let along = (pos - origin).dot(dir);
                    if best.is_none_or(|(_, d)| along < d) {
                        let normal = (Mat3::from_mat4(inverse).transpose() * hit.normal).normalize_or(hit.normal);
                        best = Some((Picked { id: o.id, pos, normal, copy }, along));
                    }
                }
            }
        }
        best.map(|(picked, _)| picked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use crate::mesh::primitives::{self, Primitive};

    fn cube() -> Mesh {
        primitives::build(Primitive::Cube, 600, 0.5)
    }

    fn volume(mesh: &Mesh) -> f64 {
        mesh.indices
            .chunks_exact(3)
            .map(|t| {
                let p = [0, 1, 2].map(|c| mesh.positions[t[c] as usize].as_dvec3());
                p[0].dot(p[1].cross(p[2])) / 6.0
            })
            .sum()
    }

    /// Three cubes in a row along X; returns their ids, top row first.
    fn three(mesh: &mut Mesh) -> (Scene, [u64; 3]) {
        let mut scene = Scene::new("Cube");
        let a = scene.active_id();
        let b = scene.add("Cube", cube(), mesh);
        let c = scene.add("Cube", cube(), mesh);
        for (i, id) in [a, b, c].into_iter().enumerate() {
            scene.get_mut(id).unwrap().position.x = i as f32 * 3.0;
        }
        scene.select(a, mesh);
        (scene, [a, b, c])
    }

    fn shown(scene: &Scene) -> Vec<bool> {
        scene.objects.iter().map(|o| o.visible).collect()
    }

    #[test]
    fn selection_adds_toggles_and_always_holds_the_active_object() {
        let mut mesh = cube();
        let (mut scene, [a, b, c]) = three(&mut mesh);
        assert_eq!(scene.selected(), [a]);

        // Ctrl + click only ever adds, and leaves the active object alone.
        assert!(scene.select_also(c));
        assert!(scene.select_also(b));
        assert!(!scene.select_also(c));
        assert_eq!((scene.selected(), scene.active_id()), (&[a, c, b][..], a));
        assert!(scene.is_selected(b) && !scene.is_selected(99));

        // Toggling the active one out promotes the earliest-picked of the rest.
        assert!(scene.toggle_selected(a, &mut mesh));
        assert_eq!((scene.selected(), scene.active_id()), (&[c, b][..], c));
        assert!(scene.toggle_selected(a, &mut mesh));
        assert!(scene.toggle_selected(b, &mut mesh));
        assert_eq!(scene.selected(), [c, a]);
        // The last one cannot be toggled out.
        assert!(scene.toggle_selected(a, &mut mesh));
        assert!(!scene.toggle_selected(c, &mut mesh));
        assert_eq!((scene.selected(), scene.active_id()), (&[c][..], c));

        // A plain click drops the rest.
        scene.select_also(a);
        scene.select(b, &mut mesh);
        assert_eq!((scene.selected(), scene.active_id()), (&[b][..], b));

        // The bounds Frame fits are those of everything selected.
        scene.select_also(c);
        let bounds = scene.selection_bounds(&mesh);
        assert!((bounds.min.x - 2.5).abs() < 1e-4 && (bounds.max.x - 6.5).abs() < 1e-4);
    }

    #[test]
    fn selection_survives_undo_and_redo() {
        let mut mesh = cube();
        let (mut scene, [a, b, c]) = three(&mut mesh);
        let mut history = History::default();
        scene.select_also(b);
        scene.select_also(c);

        // Deleting a selected object is an undo step that holds the selection.
        let before = scene.state();
        assert!(scene.remove(c, &mut mesh));
        history.push_scene(before);
        assert_eq!((scene.selected(), scene.active_id()), (&[a, b][..], a));
        // So is deleting the active one: another selected object takes over.
        let before = scene.state();
        assert!(scene.remove(a, &mut mesh));
        history.push_scene(before);
        assert_eq!((scene.selected(), scene.active_id()), (&[b][..], b));

        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!((scene.selected(), scene.active_id()), (&[a, b][..], a));
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!((scene.selected(), scene.active_id()), (&[a, b, c][..], a));
        assert_eq!(scene.objects.len(), 3);

        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert_eq!((scene.selected(), scene.active_id()), (&[a, b][..], a));
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert_eq!((scene.selected(), scene.active_id()), (&[b][..], b));
    }

    #[test]
    fn joining_two_cubes_keeps_both_volumes_and_masks_and_undoes_in_one_step() {
        let mut mesh = cube();
        let mut scene = Scene::new("Cube");
        let mut history = History::default();
        let first = scene.active_id();
        // The survivor is turned and twice the size; its top half is masked.
        let object = scene.active_mut();
        (object.position, object.scale) = (Vec3::new(1.0, 2.0, 3.0), Vec3::splat(2.0));
        object.rotation = Quat::from_rotation_y(0.4);
        for v in 0..mesh.vertex_count() {
            mesh.mask[v] = if mesh.positions[v].y > 0.06 { 1.0 } else { 0.0 };
        }
        let masked = mesh.mask.iter().filter(|&&m| m == 1.0).count();
        let first_verts = mesh.vertex_count();

        let second = scene.add("Cube", cube(), &mut mesh);
        scene.get_mut(second).unwrap().position = Vec3::new(9.0, 0.0, 0.0);
        mesh.mask.fill(0.5);
        let second_verts = mesh.vertex_count();
        let second_mesh = scene.active_mesh();

        // One object selected is nothing to join.
        scene.select(first, &mut mesh);
        assert!(scene.joined_mesh(&mesh).is_none());
        scene.select_also(second);
        let world = scene.active().matrix();

        let before = scene.state();
        let joined = scene.joined_mesh(&mesh).expect("two are selected");
        assert_eq!(scene.install_joined(joined, &mut mesh), 2);
        history.push_scene(before);

        // One object, where the survivor was, holding both shells.
        assert_eq!(scene.objects.len(), 1);
        assert_eq!((scene.active_id(), scene.selected()), (first, &[first][..]));
        assert_eq!(scene.active().matrix(), world);
        assert_eq!(mesh.vertex_count(), first_verts + second_verts);
        // In the survivor's space its own cube is 1 and the other, an eighth
        // of the survivor's size in the world, is 1/8.
        assert!((volume(&mesh) - 1.125).abs() < 1e-4, "volume {}", volume(&mesh));
        // The second cube is still where it was in the world.
        let far = mesh.positions.iter().map(|&p| world.transform_point3(p)).filter(|p| p.x > 8.0).count();
        assert_eq!(far, second_verts);
        // Both masks came across, each on its own vertices.
        assert_eq!(mesh.mask.iter().filter(|&&m| m == 1.0).count(), masked);
        assert_eq!(mesh.mask.iter().filter(|&&m| m == 0.5).count(), second_verts);
        for (p, &m) in mesh.positions.iter().zip(&mesh.mask) {
            let own = p.abs().max_element() < 0.5001;
            let expect = if !own { 0.5 } else if p.y > 0.06 { 1.0 } else { 0.0 };
            assert_eq!(m, expect, "mask at {p}");
        }

        // One undo brings both back, meshes and masks as they were.
        scene.collect(|id| history.keeps_mesh(id));
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert!(!history.can_undo());
        assert_eq!(scene.objects.len(), 2);
        assert_eq!((scene.active_id(), scene.selected()), (first, &[first, second][..]));
        assert_eq!(mesh.vertex_count(), first_verts);
        assert!((volume(&mesh) - 1.0).abs() < 1e-5);
        assert_eq!(mesh.mask.iter().filter(|&&m| m == 1.0).count(), masked);
        let other = scene.mesh(second_mesh, &mesh).expect("the second cube's mesh is back");
        assert!((volume(other) - 1.0).abs() < 1e-5);
        assert!(other.mask.iter().all(|&m| m == 0.5));

        // And redo joins them again.
        scene.collect(|id| history.keeps_mesh(id));
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert_eq!(scene.objects.len(), 1);
        assert!((volume(&mesh) - 1.125).abs() < 1e-4);
    }

    #[test]
    fn joining_leaves_instances_outside_the_selection_as_they_were() {
        let mut mesh = cube();
        let (mut scene, [a, b, _]) = three(&mut mesh);
        // An instance of `a` that is not part of the join.
        let twin = scene.instance_active();
        let shared = scene.active_mesh();
        scene.select(a, &mut mesh);
        scene.select_also(b);
        let joined = scene.joined_mesh(&mesh).unwrap();
        scene.install_joined(joined, &mut mesh);
        assert_ne!(scene.active_mesh(), shared);
        assert_eq!(scene.get(twin).unwrap().mesh, shared);
        let kept = scene.mesh(shared, &mesh).expect("the instance still has its mesh");
        assert!((volume(kept) - 1.0).abs() < 1e-5);
        assert!((volume(&mesh) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn hiding_the_active_object_moves_the_brushes_to_one_that_shows() {
        let mut mesh = cube();
        let (mut scene, [a, b, c]) = three(&mut mesh);
        // The eye on the active object: the first shown row takes over.
        scene.select(b, &mut mesh);
        assert!(scene.toggle_visible(b, &mut mesh));
        assert_eq!((scene.active_id(), scene.selected()), (a, &[a][..]));
        assert!(scene.active().visible);
        // Hiding one that is not active leaves the active one alone.
        assert!(scene.toggle_visible(c, &mut mesh));
        assert_eq!(scene.active_id(), a);
        // With nothing left to see, the selection stays put.
        assert!(scene.toggle_visible(a, &mut mesh));
        assert_eq!((scene.active_id(), scene.selected()), (a, &[a][..]));
        assert_eq!(shown(&scene), [false, false, false]);

        // In a selection of several, another selected object is preferred.
        for id in [a, b, c] {
            scene.toggle_visible(id, &mut mesh);
        }
        scene.select(a, &mut mesh);
        scene.select_also(c);
        scene.toggle_visible(a, &mut mesh);
        assert_eq!((scene.active_id(), scene.selected()), (c, &[c][..]));

        // Deleting the active object passes over a hidden neighbour too.
        scene.toggle_visible(a, &mut mesh);
        scene.toggle_visible(b, &mut mesh);
        scene.select(a, &mut mesh);
        assert!(scene.remove(a, &mut mesh));
        assert_eq!(scene.active_id(), c);
    }

    #[test]
    fn hide_others_then_show_all_is_a_round_trip() {
        let mut mesh = cube();
        let (mut scene, [a, b, c]) = three(&mut mesh);
        scene.select(b, &mut mesh);
        assert_eq!(scene.hide_unselected(), 2);
        assert_eq!(shown(&scene), [false, true, false]);
        assert_eq!(scene.hide_unselected(), 0);
        assert_eq!(scene.unhide_all(), 2);
        assert_eq!(shown(&scene), [true, true, true]);
        assert_eq!(scene.unhide_all(), 0);
        assert_eq!((scene.active_id(), scene.selected()), (b, &[b][..]));

        // H hides the whole selection and hands the brushes on.
        scene.select_also(c);
        assert_eq!(scene.hide_selected(&mut mesh), 2);
        assert_eq!(shown(&scene), [true, false, false]);
        assert_eq!((scene.active_id(), scene.selected()), (a, &[a][..]));
        assert_eq!(scene.unhide_all(), 2);
    }

    #[test]
    fn local_view_solos_the_selection_and_puts_back_only_what_it_hid() {
        let mut mesh = cube();
        let (mut scene, [a, _, c]) = three(&mut mesh);
        // `c` was hidden before, and stays so afterwards.
        scene.select(c, &mut mesh);
        scene.hide_selected(&mut mesh);
        scene.select(a, &mut mesh);
        let back = (Vec3::new(1.0, 2.0, 3.0), 7.0);
        assert!(scene.enter_local_view(back));
        assert!(scene.in_local_view());
        assert_eq!(shown(&scene), [true, false, false]);
        // Entering again does nothing, and keeps the first way back.
        assert!(!scene.enter_local_view((Vec3::ZERO, 1.0)));

        // Alt + H inside it shows what was hidden before, not what it set aside.
        assert_eq!(scene.unhide_all(), 1);
        assert_eq!(shown(&scene), [true, false, true]);
        scene.select(c, &mut mesh);
        scene.hide_selected(&mut mesh);

        assert_eq!(scene.exit_local_view(), Some(back));
        assert!(!scene.in_local_view());
        assert_eq!(shown(&scene), [true, true, false]);
        assert_eq!(scene.exit_local_view(), None);

        // Nothing shown is selected: nothing to solo.
        scene.select(c, &mut mesh);
        assert!(!scene.enter_local_view(back));
        assert_eq!(shown(&scene), [true, true, false]);
    }

    #[test]
    fn the_hide_keys_are_not_undone_by_stepping_across_older_edits() {
        let mut mesh = cube();
        let (mut scene, [a, b, c]) = three(&mut mesh);
        let mut history = History::default();

        // A rename while everything shows, then Shift + H.
        let before = scene.state();
        scene.get_mut(a).unwrap().name = "Head".into();
        history.push_scene(before);
        scene.collect(|id| history.keeps_mesh(id));
        assert_eq!(scene.hide_unselected(), 2);

        // Undoing the rename does not bring the others back...
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(scene.get(a).unwrap().name, "Cube");
        assert_eq!(shown(&scene), [true, false, false]);
        // ...and redoing it does not either.
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert_eq!(shown(&scene), [true, false, false]);

        // The eye is an undo step, and from then on undo decides for that object.
        let before = scene.state();
        scene.toggle_visible(b, &mut mesh);
        history.push_scene(before);
        scene.collect(|id| history.keeps_mesh(id));
        assert_eq!(shown(&scene), [true, true, false]);
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(shown(&scene), [true, false, false]);
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert_eq!(shown(&scene), [true, true, false]);

        // So is a tool hiding something inside its own step.
        assert_eq!(scene.unhide_all(), 1);
        let before = scene.state();
        scene.get_mut(c).unwrap().visible = false;
        history.push_scene(before);
        scene.collect(|id| history.keeps_mesh(id));
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(shown(&scene), [true, true, true]);
    }

    #[test]
    fn new_shapes_are_named_and_sized_as_in_the_unity_app() {
        let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let mut scene = Scene::new("Sphere");
        // The sphere the scene starts with counts as the first of its name.
        assert_eq!(scene.spawn_name("Sphere"), "Sphere (1)");
        assert_eq!(scene.spawn_name("Cube"), "Cube");
        let name = scene.spawn_name("Cube");
        assert_eq!(name, "Cube (1)");
        let one = scene.add(&name, cube(), &mut mesh);
        assert_eq!(scene.spawn_name("Cube"), "Cube (2)");
        // A number is not handed out again once its shape is gone...
        scene.remove(one, &mut mesh);
        assert_eq!(scene.spawn_name("Cube"), "Cube (3)");
        // ...and one a rename has taken is skipped.
        scene.active_mut().name = "Cube (4)".into();
        assert_eq!(scene.spawn_name("Cube"), "Cube (5)");
        assert_eq!(scene.spawn_name("Cone"), "Cone");

        // Nine tenths of the top object: the 1-unit sphere gives 0.9 across.
        assert!((scene.new_shape_size(&mesh) - 0.45).abs() < 1e-4);
        scene.active_mut().scale = Vec3::new(2.0, 4.0, 6.0);
        assert!((scene.new_shape_size(&mesh) - 0.45 * 4.0).abs() < 1e-3);
    }

    #[test]
    fn a_ray_finds_the_instance_of_the_active_mesh_behind_another_object() {
        let mut mesh = cube();
        let (mut scene, [a, b, _]) = three(&mut mesh);
        // An instance of `a` straight behind `b`.
        let twin = scene.instance_active();
        scene.get_mut(twin).unwrap().position = Vec3::new(3.0, 0.0, -4.0);
        scene.select(a, &mut mesh);
        let (origin, dir) = (Vec3::new(3.0, 0.0, 10.0), Vec3::NEG_Z);
        assert_eq!(scene.pick(origin, dir, &mesh).unwrap().id, b);
        let own = scene.pick_mesh(origin, dir, scene.active_mesh(), &mesh).unwrap();
        assert_eq!(own.id, twin);
        assert!((own.pos.z + 3.5).abs() < 1e-4);
        assert!(scene.shows_active_mesh(twin) && !scene.shows_active_mesh(b));

        // A stroke that begins on the instance goes through its placement,
        // and so does the pivot afterwards; the next press starts afresh.
        assert_eq!(scene.brush_object().id, a);
        scene.aim_brush(twin, 0);
        assert_eq!((scene.brush_object().id, scene.stroke_object().id), (twin, twin));
        scene.release_brush();
        assert_eq!((scene.brush_object().id, scene.stroke_object().id), (a, twin));
        // Another object is never a placement of this mesh.
        scene.aim_brush(b, 0);
        assert_eq!((scene.brush_object().id, scene.stroke_object().id), (a, a));
    }

    #[test]
    fn reset_shape_goes_back_to_the_last_built_shape() {
        use crate::camera::Camera;
        use crate::sculpt::{Brush, PointerSample, Sculptor};
        use glam::Vec2;

        const SIZE: Vec2 = Vec2::new(1600.0, 900.0);
        let mut mesh = primitives::build(Primitive::Sphere, 40_000, 0.5);
        let mut scene = Scene::new("Sphere");
        let mut history = History::default();
        history.target = scene.active_mesh();
        let mut camera = Camera::default();
        camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
        let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
        let mut sculptor = Sculptor::default();
        sculptor.settings.brush = Brush::Inflate;
        sculptor.settings.symmetry = [false; 3];
        sculptor.settings.radius_px = 50.0;

        // What the app does around a stroke: keep the resting shape first.
        let mut stroke = |scene: &mut Scene, mesh: &mut Mesh, history: &mut History, y: f32| {
            scene.keep_baseline(mesh);
            let sample = |i: usize| PointerSample {
                pos: SIZE * 0.5 + Vec2::new(i as f32 * 3.0 - 60.0, y),
                pressure: None,
                time: i as f64 / 240.0,
            };
            assert!(sculptor.begin(mesh, &view, sample(0), false, false), "stroke missed the mesh");
            for i in 1..=40 {
                sculptor.sample(mesh, &view, sample(i));
                if i % 4 == 0 {
                    sculptor.frame(mesh, &view, 1.0 / 60.0);
                }
            }
            history.push_stroke(sculptor.end(mesh, &view).expect("the stroke moved something"));
        };

        // Untouched, there is nothing to reset.
        let made = mesh.positions.clone();
        assert!(!scene.can_reset(&mesh));
        assert!(scene.reset_shape(&mut mesh).is_none());

        stroke(&mut scene, &mut mesh, &mut history, -30.0);
        stroke(&mut scene, &mut mesh, &mut history, 40.0);
        let sculpted = mesh.positions.clone();
        assert_ne!(sculpted, made);
        assert!(scene.can_reset(&mesh));

        // Reset is one undo step, back to the shape as it was made.
        let delta = scene.reset_shape(&mut mesh).expect("there is a sculpt to put back");
        history.push_stroke(delta);
        assert_eq!(mesh.positions, made);
        assert!(scene.reset_shape(&mut mesh).is_none(), "already at rest");
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(mesh.positions, sculpted);
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert_eq!(mesh.positions, made);
        assert_eq!(history.undo(&mut mesh), Some(false));

        // A remesh builds a new mesh, which rests where it was built: the
        // sculpt under it is part of that shape now.
        let rebuilt = crate::remesh::voxel_remesh(&mesh, 48, [false; 3]).unwrap();
        history.push_replace(std::mem::replace(&mut mesh, rebuilt));
        let remeshed = mesh.positions.clone();
        assert!(!scene.can_reset(&mesh));
        assert!(scene.reset_shape(&mut mesh).is_none());
        stroke(&mut scene, &mut mesh, &mut history, 0.0);
        assert_ne!(mesh.positions, remeshed);
        let delta = scene.reset_shape(&mut mesh).expect("the stroke on the remesh");
        history.push_stroke(delta);
        assert_eq!(mesh.positions, remeshed);

        // Instances share the mesh, and so the one resting shape; an object
        // with its own mesh keeps its own.
        let twin = scene.instance_active();
        assert!(scene.can_reset(&mesh));
        scene.add("Cube", cube(), &mut mesh);
        assert!(!scene.can_reset(&mesh));
        scene.select(twin, &mut mesh);
        assert!(scene.can_reset(&mesh));
        // A copy made unique is a mesh of its own, at rest as it is.
        scene.make_unique(&mut mesh);
        assert!(!scene.can_reset(&mesh));
    }

    /// A flat grid of quads, each also given as its two triangles, so a mesh
    /// can carry both quads and hidden faces.
    fn quad_grid() -> Mesh {
        let side = 5u32;
        let positions: Vec<Vec3> =
            (0..side * side).map(|i| Vec3::new((i % side) as f32, (i / side) as f32, 0.0)).collect();
        let (mut indices, mut start, mut corners) = (Vec::new(), vec![0u32], Vec::new());
        for y in 0..side - 1 {
            for x in 0..side - 1 {
                let a = y * side + x;
                let quad = [a, a + 1, a + side + 1, a + side];
                indices.extend_from_slice(&[quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
                corners.extend_from_slice(&quad);
                start.push(corners.len() as u32);
            }
        }
        let (mut mesh, moved) = Mesh::new_mapped(positions, indices, None);
        let corners = corners.iter().map(|&c| moved[c as usize]).collect();
        mesh.polys = Some(Polys { start, indices: corners });
        mesh.hide(&[0, 1, 2, 9, 10]);
        for v in 0..mesh.mask.len() {
            mesh.mask[v] = (v % 3) as f32 * 0.25;
        }
        mesh
    }

    /// Each face's corners as exact positions, so two meshes can be compared
    /// whatever order they number their vertices in.
    fn faces_by_position<'a>(mesh: &Mesh, faces: impl Iterator<Item = &'a [u32]>) -> Vec<Vec<[u32; 3]>> {
        let mut all: Vec<Vec<[u32; 3]>> = faces
            .map(|corners| {
                let mut face: Vec<[u32; 3]> =
                    corners.iter().map(|&i| mesh.positions[i as usize].to_array().map(f32::to_bits)).collect();
                face.sort_unstable();
                face
            })
            .collect();
        all.sort_unstable();
        all
    }

    fn quads(mesh: &Mesh) -> Vec<Vec<[u32; 3]>> {
        let polys = mesh.polys.as_ref().expect("the quads are there");
        faces_by_position(mesh, polys.start.windows(2).map(|f| &polys.indices[f[0] as usize..f[1] as usize]))
    }

    fn hidden(mesh: &Mesh) -> Vec<Vec<[u32; 3]>> {
        let flags = mesh.hidden_triangles().expect("the hidden faces are there");
        faces_by_position(mesh, mesh.indices.chunks_exact(3).zip(flags).filter(|(_, h)| **h).map(|(t, _)| t))
    }

    fn masked(mesh: &Mesh) -> Vec<([u32; 3], u32)> {
        let mut all: Vec<_> =
            mesh.positions.iter().zip(&mesh.mask).map(|(p, m)| (p.to_array().map(f32::to_bits), m.to_bits())).collect();
        all.sort_unstable();
        all
    }

    fn assert_same_as(copy: &Mesh, original: &Mesh, what: &str) {
        assert_eq!(copy.positions.len(), original.positions.len(), "{what}: vertices");
        assert_eq!(copy.hidden_triangle_count(), original.hidden_triangle_count(), "{what}: hidden faces");
        assert!(original.hidden_triangle_count() > 0);
        assert_eq!(hidden(copy), hidden(original), "{what}: the same faces hidden");
        assert_eq!(quads(copy), quads(original), "{what}: the same quads");
        assert_eq!(masked(copy), masked(original), "{what}: the mask");
    }

    #[test]
    fn a_clone_keeps_hidden_faces_quads_and_mask() {
        let original = quad_grid();
        let reference = duplicate(&original);
        assert_same_as(&reference, &original, "duplicate");

        let mut mesh = quad_grid();
        let mut scene = Scene::new("Grid");
        let mut history = History::default();
        history.target = scene.active_mesh();
        let first = scene.active_id();
        let before = scene.state();
        let copy = scene.clone_active(&mut mesh);
        history.push_scene(before);
        // The copy is the active object and its mesh is in the slot.
        assert_eq!(scene.active_id(), copy);
        assert_same_as(&mesh, &original, "the clone");
        // The original, parked, is as it was.
        let parked = scene.mesh(scene.get(first).unwrap().mesh, &mesh).unwrap();
        assert_same_as(parked, &original, "the object cloned");

        // Undo removes the copy, redo brings it back with the same parts.
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(scene.objects.len(), 1);
        assert_same_as(&mesh, &original, "after undo");
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert_eq!(scene.objects.len(), 2);
        assert_same_as(&mesh, &original, "after redo");
    }

    #[test]
    fn make_unique_keeps_hidden_faces_quads_and_mask() {
        let original = quad_grid();
        let mut mesh = quad_grid();
        let mut scene = Scene::new("Grid");
        let mut history = History::default();
        history.target = scene.active_mesh();
        let twin = scene.instance_active();
        // An instance shares the very mesh: nothing is copied, nothing lost.
        assert_eq!(scene.get(twin).unwrap().mesh, scene.active().mesh);
        assert_same_as(&mesh, &original, "the instance");

        let before = scene.state();
        let shared = scene.active_mesh();
        assert!(scene.make_unique(&mut mesh));
        history.push_scene(before);
        assert_ne!(scene.active_mesh(), shared);
        assert_same_as(&mesh, &original, "the made-unique mesh");
        let other = scene.objects.iter().find(|o| o.mesh == shared).unwrap().mesh;
        assert_same_as(scene.mesh(other, &mesh).unwrap(), &original, "the mesh left to the other object");

        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(scene.active_mesh(), shared);
        assert_same_as(&mesh, &original, "after undo");
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert_same_as(&mesh, &original, "after redo");
    }
}
