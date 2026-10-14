//! The app's side of the hosted tools: the tools written against
//! `sculpt_toolkit` (Measure, and each one after it).
//!
//! This file is the whole host. It keeps the list of tools, lends each one a
//! view of the document while it is called, carries out what it asks for as
//! ordinary undo steps, and routes the pointer, wheel and keys to whichever
//! is up, with the same precedence the SSphere tool gets. A tool never sees
//! an app type, so adding one changes nothing here but [`registry`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Mat3, Mat4, Vec2, Vec3};
use sculpt_geom::{LengthUnit, TriMesh};
use sculpt_toolkit as kit;
use sculpt_toolkit::{Ctx, Draw, Env, Event, Jobs, MeshData, ObjectId, Outbox};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

use super::{DOUBLE_CLICK, DOUBLE_CLICK_PX, Drag, Running};
use crate::camera;
use crate::doc::{Action, Doc, Tool};
use crate::history::History;
use crate::mesh::Mesh;
use crate::printable::{self, Cure};
use crate::render::DrawItem;
use crate::scene::{Scene, SceneState};
use crate::sculpt::{Radial, RadialAxis, Sculptor};
use crate::ui::assets::Assets;

mod files;

/// Every hosted tool, in the order of their buttons. Adding a tool is one
/// line here (and its crate in Cargo.toml).
fn registry() -> Vec<Box<dyn kit::Tool>> {
    vec![
        sculpt_measure::tool(),
        sculpt_tube::tool(),
        sculpt_tube::muscle(),
        sculpt_array::tool(),
        sculpt_lathe::tool(),
        sculpt_region::mask(),
        sculpt_region::hide(),
        sculpt_region::trim(),
        sculpt_region::split(),
        sculpt_keys::tool(),
        sculpt_region::extract(),
    ]
}

/// How far (interface points) a press may wander and still be a click.
const CLICK_SLOP: f32 = 4.0;
/// How often a frame is drawn while a background job runs, to move its
/// progress text and to land its result.
const JOB_POLL: Duration = Duration::from_millis(50);

/// A press the tool passed on, watched to see whether it ends as a click.
#[derive(Clone, Copy)]
struct Watch {
    button: kit::Button,
    at: Vec2,
    moved: bool,
}

/// Things a tool or the interface asked for that only the app can do, done
/// once the call that asked has returned.
#[derive(Default)]
struct Pending {
    start: Option<usize>,
    finish: bool,
    repaint: bool,
    /// A line for the status area, and whether it is good news.
    status: Option<(String, bool)>,
    toast: Option<String>,
    /// What to add to that line: objects a tool has just made or rebuilt
    /// that are not closed surfaces, in words (`printable::warning`).
    print_warning: Option<String>,
}

/// The hosted tools and what the app keeps for them.
pub struct Tools {
    /// `None` only for the moment that tool is being called.
    slots: Vec<Option<Box<dyn kit::Tool>>>,
    infos: Vec<kit::Info>,
    /// Which one is up, while `Doc::tool` is `Tool::Plugin`.
    active: Option<usize>,
    /// The moment as the tools are shown it, refreshed by the app before it
    /// calls them. The interface pass builds its contexts from this.
    env: Env,
    /// When the tools last had their frame, for `Env::dt`.
    frame_time: f64,
    pub jobs: Jobs,
    /// The unit lengths are shown and typed in.
    pub unit: LengthUnit,
    /// What the tools asked to have drawn this frame.
    pub draw: Draw,
    /// Where the hold-Space wheel of the tool that is up is open (points).
    pub radial: Option<egui::Pos2>,
    /// The tool that is up took the press that is down.
    captured: bool,
    watch: Option<Watch>,
    pending: Pending,
}

impl Default for Tools {
    fn default() -> Tools {
        let tools = registry();
        // Sculpting structures (the spatial index, adjacency, normals) are
        // the slow part of making a large object; a job builds them on its
        // own thread.
        let baker: kit::Baker = Arc::new(|mesh: TriMesh, mask| {
            MeshData::Baked(kit::Baked::new(super::groups::Handed::built(mesh.positions, mesh.indices, mask)))
        });
        let view = kit::View::perspective(glam::Vec3::Z * 3.0, glam::Vec3::ZERO, 60.0, Vec2::new(1600.0, 960.0));
        Tools {
            infos: tools.iter().map(|t| t.info()).collect(),
            slots: tools.into_iter().map(Some).collect(),
            active: None,
            env: Env { hovering: false, active: false, ..Env::at_rest(view) },
            frame_time: 0.0,
            jobs: Jobs::new(baker),
            unit: LengthUnit::default(),
            draw: Draw::default(),
            radial: None,
            captured: false,
            watch: None,
            pending: Pending::default(),
        }
    }
}

impl Tools {
    pub fn len(&self) -> usize {
        self.infos.len()
    }

    pub fn info(&self, index: usize) -> Option<kit::Info> {
        self.infos.get(index).copied()
    }

    /// Which tool has this `Info::id`.
    pub fn info_index(&self, id: &str) -> Option<usize> {
        self.infos.iter().position(|info| info.id == id)
    }

    /// Which tool is up, given the app's own tool setting.
    pub fn active(&self, tool: Tool) -> Option<usize> {
        self.active.filter(|_| tool == Tool::Plugin)
    }

    /// The interface asks for a tool: brought up if it is not, put away if
    /// it is.
    pub fn toggle(&mut self, index: usize, tool: Tool) {
        if self.active(tool) == Some(index) {
            self.pending.finish = true;
        } else {
            self.pending.start = Some(index);
        }
    }

    fn can_step(&self, redo: bool) -> bool {
        self.active.and_then(|i| self.slots.get(i)?.as_ref()).is_some_and(|tool| tool.can_undo(redo))
    }

    /// Whether Z would step the history of the tool that is up.
    pub fn can_undo(&self) -> bool {
        self.can_step(false)
    }

    pub fn can_redo(&self) -> bool {
        self.can_step(true)
    }

    /// What the tool that is up wants the transform gizmo put on, if
    /// anything, and which tool that is (see app/transform.rs).
    pub fn gizmo(&self, tool: Tool) -> Option<(usize, kit::GizmoTarget)> {
        let index = self.active(tool)?;
        let target = self.slots.get(index)?.as_ref()?.gizmo()?;
        Some((index, target))
    }

    /// The centre of object `id` was moved onto its mesh (Set Origin to
    /// Mesh, app/origin.rs): tells every tool, so that one which keeps
    /// something measured from the object's position (the Array tool's
    /// path) puts it right. `from` and `to` are where the object stood and
    /// stands; nothing moved in the world.
    pub fn origin_moved(&mut self, id: u64, from: kit::Placement, to: kit::Placement) {
        for tool in self.slots.iter_mut().flatten() {
            tool.origin_moved(ObjectId(id), from, to);
        }
    }

    /// The wheel tiles of the tool that is up.
    pub fn radial_items(&self, tool: Tool) -> Vec<kit::RadialItem> {
        let slot = self.active(tool).and_then(|i| self.slots.get(i)?.as_ref());
        slot.map_or(Vec::new(), |tool| tool.radial())
    }

    /// Calls tool `index` with a context over the document. What the tool
    /// asked for through the context is carried out before this returns,
    /// except what needs the whole app (see [`Pending`]).
    pub fn call<R>(
        doc: &mut Doc,
        assets: &Assets,
        index: usize,
        f: impl FnOnce(&mut dyn kit::Tool, &mut Ctx) -> R,
    ) -> Option<R> {
        let mut tool = doc.tools.slots.get_mut(index)?.take()?;
        let mut env = doc.tools.env;
        env.active = doc.tools.active(doc.tool) == Some(index);
        // The selected object's own symmetry is the one the tool follows.
        env.set_symmetry(symmetry_of(&doc.scene, doc.scene.active_id()));
        env.unit = doc.tools.unit;
        env.tips = doc.opts.tooltips;
        env.clean_view = doc.opts.clean_view;

        let mut out = Outbox::default();
        let (result, rebuilt) = {
            let Doc { mesh, scene, history, sculptor, tools, .. } = doc;
            let mut access = Access { scene, mesh, history, sculptor, before: None, rebuilt: Vec::new() };
            let icons = |name: &str| assets.icon(name);
            let result = f(tool.as_mut(), &mut Ctx::new(env, &mut access, &tools.jobs, &icons, &mut out));
            (result, access.rebuilt)
        };
        doc.tools.slots[index] = Some(tool);
        // Whatever the tool made or rebuilt (a trim, a split, a merge, built keys) is
        // looked over for holes, and the status line says if it may not print.
        if let Some(warning) = print_warning(&doc.scene, &doc.mesh, &rebuilt) {
            doc.tools.pending.print_warning = Some(warning);
        }

        if let Some(unit) = out.unit {
            doc.tools.unit = unit;
        }
        take_symmetry(&mut doc.scene, &out);
        if let Some(bounds) = out.frame.filter(|b| !b.is_empty()) {
            let aspect = env.view.size.x / env.view.size.y.max(1.0);
            doc.camera.frame(crate::mesh::bvh::Aabb { min: bounds.min, max: bounds.max }, aspect, true);
        }
        let pending = &mut doc.tools.pending;
        pending.repaint |= out.repaint || out.changed || out.frame.is_some() || out.status.is_some();
        pending.status = out.status.or(pending.status.take());
        pending.toast = out.toast.or(pending.toast.take());
        if out.start {
            pending.start = Some(index);
        }
        pending.finish |= out.finish && env.active;
        Some(result)
    }
}

/// The warning for the objects a tool's commit made or gave a new mesh, if
/// any of them is not a closed surface. Objects sharing a mesh are named
/// once, and one that has gone again (a commit can add and delete) is passed
/// over.
fn print_warning(scene: &Scene, slot: &Mesh, rebuilt: &[u64]) -> Option<String> {
    let mut seen = Vec::new();
    let mut found = Vec::new();
    for object in rebuilt.iter().filter_map(|&id| scene.get(id)) {
        if seen.contains(&object.mesh) {
            continue;
        }
        seen.push(object.mesh);
        if let Some(mesh) = scene.mesh(object.mesh, slot) {
            found.push((object.name.as_str(), printable::census(mesh)));
        }
    }
    printable::warning(&found, Cure::Remesh)
}

/// An object's own symmetry as tools read it: plain numbers, with where the
/// object stands when the symmetry is fixed in the world. None for an object
/// that is not there.
fn symmetry_of(scene: &Scene, id: u64) -> kit::Symmetry {
    let Some(object) = scene.get(id) else { return kit::Symmetry::NONE };
    let symmetry = scene.symmetry(id);
    let placed =
        |p: sculpt_symmetry::Placement| kit::Placement { position: p.position, rotation: p.rotation, scale: p.scale };
    kit::Symmetry {
        mirror: symmetry.mirror,
        radial_count: symmetry.radial.repeats().clamp(1, Radial::MAX_COUNT),
        radial_axis: symmetry.radial.axis_vector(),
        world: symmetry.world(object).map(placed),
    }
}

/// Carries out what a tool asked for of an object's symmetry. Symmetry
/// belongs to objects: a change goes to the one that was selected when the
/// tool asked, which is the object it made if it had just made one (the
/// Lathe's Radial Symmetry on Create), and never to the one that was
/// selected before that. Not an undo step, like the Symmetry card's ticks.
fn take_symmetry(scene: &mut Scene, out: &Outbox) {
    let target = out.symmetry_of.map_or(scene.active_id(), |object| object.0);
    if let Some(mirror) = out.mirror {
        scene.edit_symmetry(target, |symmetry| symmetry.mirror = mirror);
    }
    if let Some(change) = out.radial {
        scene.edit_symmetry(target, |symmetry| change_radial(&mut symmetry.radial, change));
    }
}

/// A tool's request for radial symmetry, taken into an object's symmetry
/// settings, which the Symmetry card shows while it is selected. A direction
/// along X, Y or Z is that axis; any other is the custom one.
fn change_radial(radial: &mut Radial, change: kit::RadialChange) {
    if let Some(on) = change.on {
        radial.on = on;
    }
    if let Some(count) = change.count {
        radial.count = count.clamp(Radial::MIN_COUNT, Radial::MAX_COUNT);
    }
    let Some(axis) = change.axis.filter(|axis| axis.is_finite() && axis.length_squared() > 1e-12) else { return };
    let axis = axis.normalize();
    let named = [(Vec3::X, RadialAxis::X), (Vec3::Y, RadialAxis::Y), (Vec3::Z, RadialAxis::Z)];
    match named.iter().find(|(unit, _)| unit.abs_diff_eq(axis, 1e-6)) {
        Some(&(_, choice)) => radial.axis = choice,
        None => radial.set_custom_axis(axis),
    }
}

/// The document as a tool's context reaches it: the scene to read, and the
/// few edits a commit is made of.
struct Access<'a> {
    scene: &'a mut Scene,
    /// The active object's mesh (`Doc::mesh`).
    mesh: &'a mut Mesh,
    history: &'a mut History,
    sculptor: &'a mut Sculptor,
    /// The object list when the edit in progress began.
    before: Option<SceneState>,
    /// The objects this call added or gave a new mesh, for the printing
    /// check that follows it (`print_warning`).
    rebuilt: Vec<u64>,
}

/// The app's sculptable mesh from what a command carries, with the order its
/// triangles were handed over in (see `groups::Handed`).
fn build(data: MeshData) -> Option<super::groups::Handed> {
    let handed = match data {
        MeshData::Plain { mesh, mask } => super::groups::Handed::built(mesh.positions, mesh.indices, mask),
        MeshData::Baked(baked) => baked.take::<super::groups::Handed>()?,
    };
    (handed.mesh.triangle_count() > 0).then_some(handed)
}

impl Access<'_> {
    /// Call after anything that may have changed the object list or which
    /// object is active; `Running::scene_changed` for edits made in here.
    fn settle(&mut self) {
        let mesh = self.scene.active_mesh();
        if self.history.target != mesh {
            self.history.target = mesh;
            self.sculptor.last_hit = None;
        }
        // Meshes of deleted objects are kept only while undo can bring them
        // back, and count against what undo may hold.
        self.history.settle(self.scene);
    }

    /// Where the ray meets the object's mesh drawn through `model`: the
    /// object's own matrix, or one of its copies'.
    fn hit(&self, object: &crate::scene::Object, model: Mat4, ray: &kit::Ray) -> Option<kit::Hit> {
        let mesh = self.scene.mesh(object.mesh, self.mesh)?;
        // The ray in the placement's own space, and the hit back out of it.
        let inverse = model.inverse();
        let local = ray.transformed(&inverse);
        let hit = mesh.raycast(local.origin, local.dir)?;
        let position = model.transform_point3(hit.pos);
        let normal = (Mat3::from_mat4(inverse).transpose() * hit.normal).normalize_or(hit.normal);
        Some(kit::Hit { object: ObjectId(object.id), position, normal, distance: (position - ray.origin).dot(ray.dir) })
    }
}

impl kit::Scene for Access<'_> {
    fn objects(&self) -> Vec<ObjectId> {
        self.scene.objects.iter().map(|o| ObjectId(o.id)).collect()
    }

    fn info(&self, id: ObjectId) -> Option<kit::ObjectInfo<'_>> {
        let o = self.scene.get(id.0)?;
        let mesh = self.scene.mesh(o.mesh, self.mesh);
        Some(kit::ObjectInfo {
            id,
            name: &o.name,
            visible: o.visible,
            active: o.id == self.scene.active_id(),
            mesh: kit::MeshId(o.mesh),
            placement: kit::Placement { position: o.position, rotation: o.rotation, scale: o.scale },
            mirror: o.mirror,
            live_copies: self.scene.tool_places(o.id),
            vertices: mesh.map_or(0, Mesh::vertex_count),
            triangles: mesh.map_or(0, Mesh::triangle_count),
        })
    }

    fn active(&self) -> ObjectId {
        ObjectId(self.scene.active_id())
    }

    fn selected(&self) -> Vec<ObjectId> {
        // The active object first, whatever order the rest were picked in.
        let active = self.scene.active_id();
        let others = self.scene.selected().iter().copied().filter(|&id| id != active);
        std::iter::once(active).chain(others).map(ObjectId).collect()
    }

    fn mesh(&self, id: ObjectId) -> Option<kit::MeshView<'_>> {
        let o = self.scene.get(id.0)?;
        let mesh = self.scene.mesh(o.mesh, self.mesh)?;
        Some(kit::MeshView {
            id: kit::MeshId(o.mesh),
            positions: &mesh.positions,
            normals: &mesh.normals,
            indices: &mesh.indices,
            mask: &mesh.mask,
            hidden_triangles: mesh.hidden_triangles(),
            hidden_vertices: mesh.hidden_vertices(),
        })
    }

    fn raycast(&self, ray: &kit::Ray) -> Option<kit::Hit> {
        let picked = self.scene.pick(ray.origin, ray.dir, self.mesh)?;
        Some(kit::Hit {
            object: ObjectId(picked.id),
            position: picked.pos,
            normal: picked.normal,
            distance: (picked.pos - ray.origin).dot(ray.dir),
        })
    }

    fn raycast_object(&self, id: ObjectId, ray: &kit::Ray) -> Option<kit::Hit> {
        let object = self.scene.get(id.0)?;
        self.hit(object, object.matrix(), ray)
    }

    fn raycast_placed(&self, id: ObjectId, model: &Mat4, ray: &kit::Ray) -> Option<kit::Hit> {
        self.hit(self.scene.get(id.0)?, *model, ray)
    }

    fn bounds(&self, id: ObjectId) -> sculpt_geom::Aabb {
        let Some(o) = self.scene.get(id.0) else { return sculpt_geom::Aabb::EMPTY };
        let b = self.scene.bounds_of(o, self.mesh);
        if b.is_empty() { sculpt_geom::Aabb::EMPTY } else { sculpt_geom::Aabb::new(b.min, b.max) }
    }

    fn symmetry(&self, id: ObjectId) -> kit::Symmetry {
        symmetry_of(self.scene, id.0)
    }
}

impl kit::Document for Access<'_> {
    fn begin(&mut self, _label: &str) {
        self.before = Some(self.scene.state());
    }

    fn add(&mut self, object: kit::NewObject) -> Option<ObjectId> {
        let mut mesh = build(object.mesh)?.mesh;
        // A piece of an object is in that object's face groups where it is its surface.
        if let Some(from) = object.symmetry_of {
            let place = &object.placement;
            let model = Mat4::from_scale_rotation_translation(place.scale, place.rotation, place.position);
            super::groups::piece_of(self.scene, self.mesh, from.0, model, &mut mesh);
        }
        let previous = self.scene.active_id();
        let id = self.scene.add(&object.name, mesh, self.mesh);
        if let Some(o) = self.scene.get_mut(id) {
            (o.position, o.rotation, o.scale) = (object.placement.position, object.placement.rotation, object.placement.scale);
            o.visible = object.visible;
            o.mirror = object.mirror;
        }
        // A piece of an object, or something grown on it, is sculpted the
        // way that object is.
        if let Some(from) = object.symmetry_of {
            self.scene.inherit_symmetry(from.0, id);
        }
        if !object.select {
            self.scene.select(previous, self.mesh);
        }
        self.rebuilt.push(id);
        Some(ObjectId(id))
    }

    /// The object is pointed at a new mesh rather than having its mesh
    /// swapped in place. That makes a replace an edit of the object list
    /// like any other, so it shares one undo step with whatever else the
    /// commit does, and the old mesh is kept for as long as undo can ask for
    /// it, exactly as a deleted object's is.
    fn replace(&mut self, object: ObjectId, mesh: MeshData) -> bool {
        let Some(old) = self.scene.get(object.0).map(|o| o.mesh) else { return false };
        let Some(handed) = build(mesh) else { return false };
        // What is left of the object's surface stays in its face groups, and so does every copy of it.
        let mesh = super::groups::replacing(self.scene, self.mesh, object.0, handed);
        let previous = self.scene.active_id();
        // The scene only takes in a mesh along with an object, so one is
        // added for a moment to carry it.
        let carrier = self.scene.add("mesh", mesh, self.mesh);
        let Some(new) = self.scene.get(carrier).map(|o| o.mesh) else { return false };
        // Instances share a mesh, so they all follow.
        for o in self.scene.objects.iter_mut().filter(|o| o.mesh == old) {
            o.mesh = new;
        }
        self.scene.select(object.0, self.mesh);
        self.scene.remove(carrier, self.mesh);
        self.scene.select(previous, self.mesh);
        self.rebuilt.push(object.0);
        true
    }

    fn place(&mut self, object: ObjectId, placement: kit::Placement) -> bool {
        let Some(o) = self.scene.get_mut(object.0) else { return false };
        (o.position, o.rotation, o.scale) = (placement.position, placement.rotation, placement.scale);
        true
    }

    fn set_mirror(&mut self, object: ObjectId, mirror: Option<kit::MirrorRepeater>) -> bool {
        let (axes, center) = mirror.map_or((0, glam::Vec3::ZERO), |m| (m.axis_mask, m.center));
        self.scene.set_mirror(object.0, axes, center)
    }

    fn show(&mut self, object: ObjectId, visible: bool) -> bool {
        self.scene.get_mut(object.0).map(|o| o.visible = visible).is_some()
    }

    fn rename(&mut self, object: ObjectId, name: String) -> bool {
        self.scene.get_mut(object.0).map(|o| o.name = name).is_some()
    }

    fn delete(&mut self, object: ObjectId) -> bool {
        self.scene.remove(object.0, self.mesh)
    }

    fn select(&mut self, object: ObjectId) -> bool {
        if self.scene.get(object.0).is_none() {
            return false;
        }
        self.scene.select(object.0, self.mesh);
        true
    }

    fn end(&mut self) -> bool {
        let Some(before) = self.before.take() else { return false };
        // A change of selection alone is not an undo step, as when a row of
        // the object list is clicked.
        let changed = before.objects != self.scene.objects;
        if changed {
            self.history.push_scene(before);
        }
        self.settle();
        changed
    }

    fn mask(&mut self, object: ObjectId, mask: Vec<f32>) -> bool {
        if self.scene.get(object.0).is_none() {
            return false;
        }
        // A mask step is recorded against the mesh in hand, like a stroke.
        if self.scene.active_id() != object.0 {
            self.scene.select(object.0, self.mesh);
            self.settle();
        }
        if mask.len() != self.mesh.vertex_count() {
            return false;
        }
        if let Some(delta) = self.mesh.remap_mask(|_, v, _| mask[v as usize]) {
            self.history.push_stroke(delta);
        }
        true
    }

    fn hide_faces(&mut self, object: ObjectId, triangles: &[u32], hide: bool) -> bool {
        if !self.in_hand(object) {
            return false;
        }
        let Some(delta) = self.mesh.set_triangles_hidden(triangles, hide) else { return false };
        // An object that has vanished gives no sign that it is only hidden:
        // the rule the app's own hide keeps (app/faces.rs).
        if self.mesh.visible_triangle_count() == 0 {
            self.mesh.apply_visibility(&delta, true);
            return false;
        }
        self.history.push_visibility(delta);
        true
    }

    fn show_all_faces(&mut self, object: ObjectId) -> bool {
        if !self.in_hand(object) {
            return false;
        }
        let Some(delta) = self.mesh.show_all() else { return false };
        self.history.push_visibility(delta);
        true
    }

    fn invert_faces(&mut self, object: ObjectId) -> bool {
        if !self.in_hand(object) {
            return false;
        }
        let Some(delta) = self.mesh.invert_visible() else { return false };
        self.history.push_visibility(delta);
        true
    }
}

impl Access<'_> {
    /// Makes `object` the selected one, so that its mesh is the one in hand:
    /// a step on a mesh (hidden faces, like a mask or a stroke) is recorded
    /// against that mesh. False if there is no such object.
    fn in_hand(&mut self, object: ObjectId) -> bool {
        if self.scene.get(object.0).is_none() {
            return false;
        }
        if self.scene.active_id() != object.0 {
            self.scene.select(object.0, self.mesh);
            self.settle();
        }
        true
    }
}

/// The key as tools are told it.
fn key_of(code: KeyCode) -> Option<kit::Key> {
    use kit::Key as K;
    Some(match code {
        KeyCode::KeyA => K::A,
        KeyCode::KeyB => K::B,
        KeyCode::KeyC => K::C,
        KeyCode::KeyD => K::D,
        KeyCode::KeyE => K::E,
        KeyCode::KeyF => K::F,
        KeyCode::KeyG => K::G,
        KeyCode::KeyH => K::H,
        KeyCode::KeyI => K::I,
        KeyCode::KeyJ => K::J,
        KeyCode::KeyK => K::K,
        KeyCode::KeyL => K::L,
        KeyCode::KeyM => K::M,
        KeyCode::KeyN => K::N,
        KeyCode::KeyO => K::O,
        KeyCode::KeyP => K::P,
        KeyCode::KeyQ => K::Q,
        KeyCode::KeyR => K::R,
        KeyCode::KeyS => K::S,
        KeyCode::KeyT => K::T,
        KeyCode::KeyU => K::U,
        KeyCode::KeyV => K::V,
        KeyCode::KeyW => K::W,
        KeyCode::KeyX => K::X,
        KeyCode::KeyY => K::Y,
        KeyCode::KeyZ => K::Z,
        KeyCode::Digit0 | KeyCode::Numpad0 => K::Num0,
        KeyCode::Digit1 | KeyCode::Numpad1 => K::Num1,
        KeyCode::Digit2 | KeyCode::Numpad2 => K::Num2,
        KeyCode::Digit3 | KeyCode::Numpad3 => K::Num3,
        KeyCode::Digit4 | KeyCode::Numpad4 => K::Num4,
        KeyCode::Digit5 | KeyCode::Numpad5 => K::Num5,
        KeyCode::Digit6 | KeyCode::Numpad6 => K::Num6,
        KeyCode::Digit7 | KeyCode::Numpad7 => K::Num7,
        KeyCode::Digit8 | KeyCode::Numpad8 => K::Num8,
        KeyCode::Digit9 | KeyCode::Numpad9 => K::Num9,
        KeyCode::Escape => K::Escape,
        KeyCode::Tab => K::Tab,
        KeyCode::Backspace => K::Backspace,
        KeyCode::Enter | KeyCode::NumpadEnter => K::Enter,
        KeyCode::Space => K::Space,
        KeyCode::Insert => K::Insert,
        KeyCode::Delete => K::Delete,
        KeyCode::Home => K::Home,
        KeyCode::End => K::End,
        KeyCode::PageUp => K::PageUp,
        KeyCode::PageDown => K::PageDown,
        KeyCode::ArrowDown => K::ArrowDown,
        KeyCode::ArrowLeft => K::ArrowLeft,
        KeyCode::ArrowRight => K::ArrowRight,
        KeyCode::ArrowUp => K::ArrowUp,
        KeyCode::Minus | KeyCode::NumpadSubtract => K::Minus,
        KeyCode::Equal => K::Equals,
        KeyCode::NumpadAdd => K::Plus,
        KeyCode::BracketLeft => K::OpenBracket,
        KeyCode::BracketRight => K::CloseBracket,
        KeyCode::Semicolon => K::Semicolon,
        KeyCode::Quote => K::Quote,
        KeyCode::Backquote => K::Backtick,
        KeyCode::Backslash => K::Backslash,
        KeyCode::Comma => K::Comma,
        KeyCode::Period => K::Period,
        KeyCode::Slash => K::Slash,
        _ => return None,
    })
}

/// Keys that stay the app's while a hosted tool is up, if the tool does not
/// want them: undo and redo, the transform gizmo, hide the panels,
/// fullscreen.
fn stays_with_app(code: KeyCode) -> bool {
    matches!(code, KeyCode::KeyZ | KeyCode::KeyG | KeyCode::Tab | KeyCode::F11)
}

impl Running {
    /// Whether a hosted tool has the viewport.
    pub(super) fn tools_active(&self) -> bool {
        self.doc.tools.active(self.doc.tool).is_some()
    }

    /// The moment as a tool is shown it.
    fn tools_env(&self, pressure: Option<f32>) -> Env {
        let v = &self.view;
        let input = &self.input;
        let mods = kit::Mods {
            shift: input.mods.shift_key(),
            ctrl: input.mods.control_key(),
            alt: input.mods.alt_key(),
        };
        let tools = &self.doc.tools;
        let hovering = input.inside
            && v.contains(input.pointer)
            && !self.over_ui()
            && self.ui.radial.is_none()
            && tools.radial.is_none()
            && !mods.alt
            // On a handle of the tool's gizmo a press is the gizmo's.
            && !self.transform.on_tool_handle
            && (input.drag == Drag::None || tools.captured);
        Env {
            view: kit::View {
                eye: v.pos,
                right: v.right,
                up: v.up,
                forward: v.forward,
                view_proj: v.view_proj,
                size: v.size,
                tan_half_fov: v.tan_half_fov,
                ortho_half_height: v.ortho_half_height,
                pixels_per_point: self.egui_ctx.pixels_per_point(),
            },
            pointer: input.pointer - v.origin,
            hovering,
            pressure: pressure.or(input.pen_pressure.filter(|_| input.pen_down)),
            mods,
            time: self.time(),
            dt: tools.env.dt,
            // The rest are filled in per tool by `Tools::call`.
            active: false,
            mirror: [false; 3],
            radial_count: 1,
            radial_axis: glam::Vec3::Y,
            symmetry_world: None,
            unit: tools.unit,
            tips: true,
            clean_view: false,
        }
    }

    /// Calls tool `index` with the moment as it is now, then does what it
    /// asked of the app.
    pub(super) fn tools_call<R>(
        &mut self,
        index: usize,
        pressure: Option<f32>,
        f: impl FnOnce(&mut dyn kit::Tool, &mut Ctx) -> R,
    ) -> Option<R> {
        self.doc.tools.env = self.tools_env(pressure);
        let result = Tools::call(&mut self.doc, &self.ui.assets, index, f);
        self.tools_settle();
        result
    }

    /// Sends an event to the tool that is up. False if none is, or it did
    /// not want the event.
    fn tools_event(&mut self, event: Event, pressure: Option<f32>) -> bool {
        let Some(index) = self.doc.tools.active(self.doc.tool) else { return false };
        self.tools_call(index, pressure, |tool, cx| tool.input(cx, event)).unwrap_or(false)
    }

    /// Tells tool `index` about a drag of the gizmo on its target (an
    /// `Event::Gizmo`). By index rather than "the tool that is up", so the
    /// end of a drag still reaches a tool that is being put away.
    pub(super) fn tools_gizmo_event(&mut self, index: usize, event: Event) {
        self.tools_call(index, None, |tool, cx| tool.input(cx, event));
    }

    /// Does what tools and the interface asked for that needs the app.
    fn tools_settle(&mut self) {
        // Bringing a tool up or putting one away calls tools again, which
        // may ask for more; a couple of rounds settles it.
        for _ in 0..4 {
            let pending = std::mem::take(&mut self.doc.tools.pending);
            self.redraw |= pending.repaint || pending.print_warning.is_some();
            // A tool says what it did in the call that did it, so its line and the
            // warning about what it left arrive together; the warning goes after.
            match (pending.status, pending.print_warning) {
                (Some((text, _)), Some(warning)) => self.status(format!("{text} {warning}"), false),
                (Some((text, ok)), None) => self.status(text, ok),
                (None, Some(warning)) => self.status(warning, false),
                (None, None) => {}
            }
            if let Some(text) = pending.toast {
                self.toast(text);
            }
            match (pending.start, pending.finish) {
                (Some(index), _) => self.tools_activate(index),
                (None, true) => self.tools_leave(),
                (None, false) => break,
            }
        }
    }

    /// Notices that the app moved to one of its own tools (a button, a key,
    /// the bridge) while a hosted one was up, and lets that one finish.
    fn tools_sync(&mut self) {
        if self.doc.tool == Tool::Plugin {
            return;
        }
        if let Some(index) = self.doc.tools.active.take() {
            self.tools_drop_input();
            self.doc.tools.env = self.tools_env(None);
            Tools::call(&mut self.doc, &self.ui.assets, index, |tool, cx| tool.leave(cx));
        }
    }

    fn tools_drop_input(&mut self) {
        let tools = &mut self.doc.tools;
        tools.captured = false;
        tools.watch = None;
        tools.radial = None;
    }

    /// Brings tool `index` up.
    pub(super) fn tools_activate(&mut self, index: usize) {
        self.tools_sync();
        if index >= self.doc.tools.len() || self.doc.tools.active(self.doc.tool) == Some(index) {
            return;
        }
        if self.doc.sculptor.is_active() {
            return;
        }
        self.tools_drop_input();
        if let Some(old) = self.doc.tools.active.take() {
            self.doc.tools.env = self.tools_env(None);
            Tools::call(&mut self.doc, &self.ui.assets, old, |tool, cx| tool.leave(cx));
        }
        // Puts the SSpheres and the gizmo away, as any tool switch does.
        self.set_tool(Tool::Plugin);
        self.doc.tools.active = Some(index);
        self.sync_view();
        self.doc.tools.env = self.tools_env(None);
        Tools::call(&mut self.doc, &self.ui.assets, index, |tool, cx| tool.enter(cx));
        if let Some(info) = self.doc.tools.info(index) {
            self.toast(info.name);
        }
        // The tool's own first words win over the bare name.
        self.tools_settle();
        self.redraw = true;
    }

    /// Puts the hosted tool away and goes back to sculpting.
    pub(super) fn tools_leave(&mut self) {
        if self.doc.tool == Tool::Plugin {
            self.set_tool(Tool::Sculpt);
        }
        self.tools_sync();
        self.redraw = true;
    }

    /// A button went down on the viewport while a hosted tool is up.
    pub(super) fn tools_pressed(&mut self, button: MouseButton, pressure: Option<f32>) -> Drag {
        self.sync_view();
        let button = match button {
            MouseButton::Left => kit::Button::Left,
            MouseButton::Right => kit::Button::Right,
            _ => return Drag::Ui,
        };
        if self.doc.tools.radial.is_some() {
            return Drag::Ui;
        }
        // A handle of the gizmo on the tool's own target wins the press, as
        // the gizmo's handles do everywhere; the tool hears of the drag as
        // `Event::Gizmo` instead.
        if button == kit::Button::Left && self.gizmo_pressed() {
            return Drag::Gizmo;
        }
        let at = self.input.pointer;
        if self.tools_event(Event::Press(button), pressure) {
            self.doc.tools.captured = true;
            return Drag::Ui;
        }
        // Not the tool's: the camera orbits, unless it turns out to be a
        // click.
        self.doc.tools.watch = Some(Watch { button, at, moved: false });
        if button == kit::Button::Left { Drag::Orbit } else { Drag::Ui }
    }

    /// The pointer moved. True when the movement was the tool's, so nothing
    /// else should use it.
    pub(super) fn tools_moved(&mut self, pos: Vec2, pressure: Option<f32>) -> bool {
        if !self.tools_active() {
            return false;
        }
        if self.doc.tools.captured {
            self.tools_event(Event::Drag, pressure);
            return true;
        }
        let slop = CLICK_SLOP * self.egui_ctx.pixels_per_point();
        if let Some(watch) = &mut self.doc.tools.watch {
            if !watch.moved {
                if pos.distance(watch.at) <= slop {
                    // Held still so far: the camera does not creep under what
                    // may yet be a click.
                    return true;
                }
                watch.moved = true;
            }
        }
        false
    }

    pub(super) fn tools_released(&mut self) {
        let captured = std::mem::take(&mut self.doc.tools.captured);
        let watch = self.doc.tools.watch.take();
        if !self.tools_active() {
            return;
        }
        if captured {
            self.tools_event(Event::Release, None);
            return;
        }
        let Some(watch) = watch.filter(|w| !w.moved) else { return };
        if self.tools_event(Event::Click(watch.button), None) {
            self.input.last_click = None;
            return;
        }
        if watch.button != kit::Button::Left {
            return;
        }
        // A click the tool had no use for: twice in one place picks the
        // object under it or, on empty space, frames the model, as when
        // sculpting.
        let now = Instant::now();
        let double =
            self.input.last_click.is_some_and(|(t, p)| now - t < DOUBLE_CLICK && p.distance(watch.at) < DOUBLE_CLICK_PX);
        self.input.last_click = Some((now, watch.at));
        if double {
            self.input.last_click = None;
            match self.pick(watch.at) {
                Some(id) if id != self.doc.scene.active_id() => self.act(Action::SelectObject(id)),
                _ => self.frame_view(),
            }
        }
    }

    /// The window lost focus. A drag the tool holds ends as the release it
    /// would have had; a press it passed on, still waiting to turn out a
    /// click, is dropped rather than made into one.
    pub(super) fn tools_focus_lost(&mut self) {
        self.doc.tools.watch = None;
        self.tools_released();
    }

    /// Whether the tool that is up holds the drag that is going on.
    pub(super) fn tools_holds_drag(&self) -> bool {
        self.doc.tools.captured
    }

    /// True when a hosted tool is up: it gets the wheel first, and what it
    /// does not use zooms (never the brush, which is put away).
    pub(super) fn tools_wheel(&mut self, lines: f32) -> bool {
        if !self.tools_active() {
            return false;
        }
        if !self.tools_event(Event::Wheel(lines), None) {
            let factor = 1.0 - lines.signum() * camera::WHEEL_ZOOM_STEP;
            self.doc.camera.zoom(factor, self.view_depth, self.scene_size, None);
        }
        true
    }

    /// A key went down. True when it was a hosted tool's, or one that has
    /// nothing to act on while a hosted tool is up. Ctrl shortcuts stay the
    /// app's.
    pub(super) fn tools_key_pressed(&mut self, code: KeyCode) -> bool {
        if !self.tools_active() || self.input.mods.control_key() {
            return false;
        }
        let idle = self.input.drag == Drag::None;
        if code == KeyCode::Space {
            let has_wheel = !self.doc.tools.radial_items(self.doc.tool).is_empty();
            if idle && has_wheel && self.doc.tools.radial.is_none() {
                let ppp = self.egui_ctx.pixels_per_point();
                self.doc.tools.radial = Some(egui::pos2(self.input.pointer.x / ppp, self.input.pointer.y / ppp));
            }
            return true;
        }
        if let Some(key) = key_of(code) {
            if self.tools_event(Event::KeyDown(key), None) {
                return true;
            }
        }
        match code {
            // Nothing of the tool's to cancel: Esc closes its wheel, else
            // puts the tool away.
            KeyCode::Escape => {
                if self.doc.tools.radial.take().is_none() {
                    self.tools_leave();
                }
                return true;
            }
            // F F frames, as when sculpting; a lone F has no meaning here.
            KeyCode::KeyF if idle => {
                let now = Instant::now();
                if self.input.last_f.is_some_and(|t| now - t < DOUBLE_CLICK) {
                    self.input.last_f = None;
                    self.frame_view();
                } else {
                    self.input.last_f = Some(now);
                }
                return true;
            }
            _ if stays_with_app(code) => return false,
            _ => {}
        }
        // Another tool's key goes straight to it; this tool's own key puts
        // it away.
        if idle {
            self.tools_hotkey(code);
        }
        // Everything else is a brush key with nothing to act on now.
        true
    }

    /// Brings up (or puts away) the hosted tool whose key this is. The app
    /// calls it for keys nothing else uses, so its own keys always win.
    pub(super) fn tools_hotkey(&mut self, code: KeyCode) -> bool {
        if self.input.mods.control_key() || self.input.drag != Drag::None || self.doc.tool == Tool::SSpheres {
            return false;
        }
        let Some(key) = key_of(code) else { return false };
        let Some(index) = self.doc.tools.infos.iter().position(|info| info.hotkey == Some(key)) else { return false };
        if self.doc.tools.active(self.doc.tool) == Some(index) {
            self.tools_leave();
        } else {
            self.tools_activate(index);
        }
        true
    }

    /// Releases always go through, so a key the tool is holding can never
    /// stick.
    pub(super) fn tools_key_released(&mut self, code: KeyCode) {
        if !self.tools_active() {
            return;
        }
        if code == KeyCode::Space {
            self.doc.tools.radial = None;
        }
        if let Some(key) = key_of(code) {
            self.tools_event(Event::KeyUp(key), None);
        }
    }

    /// Z while a hosted tool is up: its own history first. False sends the
    /// press on to the scene's history.
    pub(super) fn tools_undo(&mut self, redo: bool) -> bool {
        if !self.tools_active() {
            return false;
        }
        if !self.tools_event(if redo { Event::Redo } else { Event::Undo }, None) {
            return false;
        }
        self.toast(if redo { "Redo" } else { "Undo" });
        true
    }

    /// Once a frame, before the interface is built: every tool gets its
    /// frame, and what they ask to have drawn is handed on.
    pub(super) fn tools_frame(&mut self) {
        self.tools_sync();
        let mut env = self.tools_env(None);
        env.dt = (env.time - self.doc.tools.frame_time).clamp(0.0, 0.1) as f32;
        self.doc.tools.frame_time = env.time;
        self.doc.tools.env = env;
        // A brush stroke has the mesh while it runs: tools wait, and what
        // they last asked for stays on screen. A gizmo drag that reshapes
        // the mesh around a mask has it in the same way.
        if !self.doc.sculptor.is_active() && !self.transform_holds_mesh() {
            let mut draw = Draw::default();
            for index in 0..self.doc.tools.len() {
                let mut one = Draw::default();
                Tools::call(&mut self.doc, &self.ui.assets, index, |tool, cx| tool.frame(cx, &mut one));
                draw.append(one);
            }
            // The copies tools keep of objects (an array's) are places the
            // object is shown, for the pointer and for the other tools.
            let mut places = std::collections::HashMap::<u64, Vec<Mat4>>::new();
            for (object, copies) in draw.object_places() {
                places.entry(object.0).or_default().extend(copies.iter().copied());
            }
            self.doc.scene.set_tool_places(places);
            self.doc.tools.draw = draw;
        }
        self.renderer.previews.show(&self.gpu, &self.view, self.doc.tools.draw.previews());
        if self.doc.tools.jobs.is_busy() {
            let at = Instant::now() + JOB_POLL;
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
        }
        self.tools_settle();
    }

    /// After the interface pass: its buttons may have asked for a tool.
    pub(super) fn tools_after_ui(&mut self) {
        self.tools_settle();
    }

    /// Whether a tool asked for an object to be left out of this frame.
    pub(super) fn tools_hide(&self, object: u64) -> bool {
        self.doc.tools.draw.hidden().contains(&ObjectId(object))
    }

    /// Adds the extra copies of scene objects the tools asked for.
    pub(super) fn tools_copies(&self, mut items: Vec<DrawItem>) -> Vec<DrawItem> {
        for (object, transforms) in self.doc.tools.draw.object_copies() {
            let Some(o) = self.doc.scene.get(object.0) else { continue };
            // The last element marks the active object; a copy never is.
            items.extend(transforms.iter().map(|m| {
                let mut model = m.to_cols_array_2d();
                model[3][3] = 0.0;
                DrawItem { mesh: o.mesh, model }
            }));
        }
        items
    }

    /// `--demo-tool id[:what]`: brings a tool up in a state worth looking
    /// at, for visual checks without a person at the tablet.
    pub(super) fn tools_demo(&mut self, spec: &str) {
        let (id, what) = spec.split_once(':').unwrap_or((spec, ""));
        if id == "studs" && !self.doc.tools.infos.iter().any(|info| info.id == id) {
            // The toolkit's worked example: not on a button, but here to be
            // looked at, and to show previews and jobs working end to end.
            let tool = kit::example::tool();
            self.doc.tools.infos.push(tool.info());
            self.doc.tools.slots.push(Some(tool));
        }
        let Some(index) = self.doc.tools.infos.iter().position(|info| info.id == id) else {
            let known: Vec<&str> = self.doc.tools.infos.iter().map(|info| info.id).collect();
            eprintln!("no tool called {id:?}; there are: {}", known.join(", "));
            return;
        };
        self.demo_stage();
        self.sync_view();
        self.tools_activate(index);
        if what == "play" {
            return self.tools_play(index);
        }
        if what == "check" {
            return self.tools_check(index);
        }
        if what == "focus" {
            return self.focus_demo(index);
        }
        if what == "turn" {
            return self.turn_demo();
        }
        if self.demo_with_array(index, id, what) {
            return;
        }
        self.tools_call(index, None, |tool, cx| tool.demo(cx, what));
        // The picture may be taken from somewhere other than where the demo drew from.
        self.demo_view("SCULPT_DEMO_LOOK");
        // A glide the demo asked for is over before the picture is taken, so that
        // the gizmo and the overlay are drawn from the camera that ends up there.
        self.doc.camera.update(10.0);
        self.sync_view();
    }

    /// For a scripted tool run on a real file: `SCULPT_DEMO_OBJECT` names the object to show alone and
    /// select, and `SCULPT_DEMO_VIEW` (`yaw,pitch[,zoom[,right,up]]`) is where to look at it from, how
    /// much closer than fitted, and how far off its middle in units of its size.
    fn demo_stage(&mut self) {
        if let Ok(name) = std::env::var("SCULPT_DEMO_OBJECT") {
            let found = self.doc.scene.objects.iter().find(|o| o.name == name).map(|o| o.id);
            if let Some(id) = found {
                for o in &mut self.doc.scene.objects {
                    o.visible = o.id == id;
                }
                self.doc.scene.select(id, &mut self.doc.mesh);
                self.scene_changed();
            }
        }
        for o in &self.doc.scene.objects {
            println!("object {:?} visible {}", o.name, o.visible);
        }
        self.demo_view("SCULPT_DEMO_VIEW");
    }

    fn demo_view(&mut self, variable: &str) {
        let Ok(spec) = std::env::var(variable) else { return };
        let v: Vec<f32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if v.len() < 2 {
            return;
        }
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.selection_bounds(&self.doc.mesh);
        let reach = (bounds.max - bounds.min).length() * 0.5;
        let camera = &mut self.doc.camera;
        (camera.yaw, camera.pitch) = (v[0], v[1].clamp(-89.0, 89.0));
        camera.frame(bounds, aspect, false);
        camera.distance /= v.get(2).copied().unwrap_or(1.0).max(0.05);
        self.sync_view();
        let (right, up) = (v.get(3).copied().unwrap_or(0.0), v.get(4).copied().unwrap_or(0.0));
        self.doc.camera.pivot += (self.view.right * right + self.view.up * up) * reach;
    }

    /// `--demo-tool id:check`: runs each kind of command against the real
    /// document through a tool's context, steps the app's history back and
    /// forward over them, and prints what the scene held at each point.
    fn tools_check(&mut self, index: usize) {
        use kit::{Command, NewObject, Placement};
        let cube = kit::testing::cube(0.3);
        let active = ObjectId(self.doc.scene.active_id());
        let say = |app: &Running, when: &str| {
            let o = app.doc.scene.active();
            println!(
                "check: {when}: {} objects, active \"{}\" mesh {} with {} tris, history {:?}, mask sum {:.0}",
                app.doc.scene.objects.len(),
                o.name,
                o.mesh,
                app.doc.mesh.triangle_count(),
                app.doc.history.steps(),
                app.doc.mesh.mask.iter().sum::<f32>(),
            );
        };
        say(self, "start");
        let piece = cube.clone();
        let receipt = self.tools_call(index, None, |_, cx| {
            cx.commit(
                "Check",
                vec![
                    Command::Replace { object: active, mesh: cube.into() },
                    Command::Add(NewObject::new("Piece", piece).placed(Placement::at(glam::Vec3::X)).unselected()),
                    Command::Rename { object: active, name: "Replaced".into() },
                    Command::Delete { object: ObjectId(u64::MAX) },
                ],
            )
        });
        println!("check: receipt {receipt:?} (one added, one failed, changed)");
        say(self, "after replace + add + rename");
        self.act(Action::Undo);
        say(self, "after undo");
        self.act(Action::Redo);
        say(self, "after redo");
        let count = self.doc.mesh.vertex_count();
        self.tools_call(index, None, |_, cx| cx.commit("Mask", vec![Command::Mask { object: active, mask: vec![1.0; count] }]));
        say(self, "after mask");
        self.act(Action::Undo);
        say(self, "after undoing the mask");
        self.act(Action::Undo);
        say(self, "after undoing the commit");
    }

    /// `--demo-tool id:play`: drives the tool through the handlers the
    /// window feeds (presses, drags, the wheel, keys) and prints what the
    /// host did with each, to check the routing without a person at the
    /// tablet.
    fn tools_play(&mut self, index: usize) {
        self.input.inside = true;
        let centre = self.view.size * 0.5;
        let drag = |app: &mut Running, from: Vec2, to: Vec2| {
            app.pointer_moved(from, None);
            app.pointer_pressed(MouseButton::Left, None);
            for i in 1..=8 {
                app.pointer_moved(from.lerp(to, i as f32 / 8.0), None);
            }
            app.pointer_released(MouseButton::Left);
        };
        let yaw = self.doc.camera.yaw;
        drag(self, centre + Vec2::new(-120.0, 40.0), centre + Vec2::new(120.0, -40.0));
        println!("play: a drag on the model turned the camera {:.1} degrees (0 if the tool took it)", self.doc.camera.yaw - yaw);
        let corner = Vec2::new(centre.x, 40.0);
        drag(self, corner, corner + Vec2::new(160.0, 0.0));
        println!("play: a drag on the background turned the camera {:.1} degrees (40 expected)", self.doc.camera.yaw - yaw);
        let yaw = self.doc.camera.yaw;
        drag(self, corner, corner + Vec2::new(2.0, 1.0));
        println!("play: a click on the background turned the camera {:.1} degrees (0 expected)", self.doc.camera.yaw - yaw);
        let distance = self.doc.camera.distance;
        self.wheel(1.0);
        println!("play: the wheel off the model moved the camera from {distance:.2} to {:.2}", self.doc.camera.distance);
        let steps = self.doc.history.steps();
        self.act(Action::Undo);
        println!("play: Z with the tool up left the scene history at {:?} (was {steps:?})", self.doc.history.steps());
        for press in 1..=4 {
            let taken = self.tools_key_pressed(KeyCode::Escape);
            println!("play: Esc {press}: taken {taken}, tool now {:?}", self.doc.tool);
        }
        if let Some(code) = [KeyCode::KeyL, KeyCode::KeyT, KeyCode::KeyJ, KeyCode::KeyK, KeyCode::KeyY]
            .into_iter()
            .find(|&code| key_of(code) == self.doc.tools.info(index).and_then(|info| info.hotkey))
        {
            let up = self.tools_hotkey(code);
            println!("play: its key from Sculpt: taken {up}, tool now {:?}", self.doc.tool);
            let away = self.tools_key_pressed(code);
            println!("play: its key again: taken {away}, tool now {:?}", self.doc.tool);
        }
        self.tools_activate(index);
        self.sync_view();
    }
}

#[cfg(test)]
mod tests {
    use kit::{Command, Scene as _};

    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    fn cube() -> Mesh {
        primitives::build(Primitive::Cube, 600, 0.5)
    }

    /// A tool that asks for radial symmetry changes the settings the Symmetry card shows, and leaves alone
    /// what it does not name.
    #[test]
    fn a_tools_request_for_radial_symmetry_changes_the_symmetry_settings() {
        let mut radial = Radial::default();
        assert!(!radial.on);
        fn ask(radial: &mut Radial, on: Option<bool>, count: Option<u32>, axis: Option<Vec3>) {
            change_radial(radial, kit::RadialChange { on, count, axis });
        }
        ask(&mut radial, Some(true), None, Some(Vec3::Z));
        assert_eq!((radial.on, radial.count, radial.axis), (true, Radial::default().count, RadialAxis::Z));
        ask(&mut radial, None, Some(5), None);
        assert_eq!((radial.on, radial.count, radial.axis), (true, 5, RadialAxis::Z));
        // The count is kept in range, and a direction that is not one of the three is the custom axis.
        ask(&mut radial, None, Some(0), Some(Vec3::new(0.0, 2.0, 2.0)));
        assert_eq!((radial.count, radial.axis), (Radial::MIN_COUNT, RadialAxis::Custom));
        assert!(radial.axis_vector().abs_diff_eq(Vec3::new(0.0, 1.0, 1.0).normalize(), 1e-6));
        ask(&mut radial, None, Some(99), Some(Vec3::Y * 4.0));
        assert_eq!((radial.count, radial.axis), (Radial::MAX_COUNT, RadialAxis::Y));
        // Something with no direction is no axis; off keeps the rest for next time.
        ask(&mut radial, Some(false), None, Some(Vec3::ZERO));
        assert_eq!((radial.on, radial.count, radial.axis), (false, Radial::MAX_COUNT, RadialAxis::Y));
        assert_eq!(radial.repeats(), 1);
    }

    /// What a merge or a remesh gives back a model through a context (`Command::Replace`): the new mesh is
    /// not the old one's vertices, so it arrives with no mask and every face showing, as the Unity app's
    /// `ReplaceGeometry` left it.
    #[test]
    fn a_replaced_mesh_arrives_with_no_mask_and_nothing_hidden() {
        let mut mesh = cube();
        let mut scene = Scene::new("Cube");
        let id = ObjectId(scene.active_id());
        let mut history = History::default();
        history.target = scene.active_mesh();
        let mut sculptor = Sculptor::default();
        let mut access = Access {
            scene: &mut scene,
            mesh: &mut mesh,
            history: &mut history,
            sculptor: &mut sculptor,
            before: None,
            rebuilt: Vec::new(),
        };

        let count = access.mesh(id).unwrap().triangle_count();
        let vertices = access.mesh(id).unwrap().vertex_count();
        let half: Vec<u32> = (0..count as u32 / 2).collect();
        let hidden = half.len();
        let marked = kit::scene::apply(
            &mut access,
            "Mark",
            vec![
                Command::Mask { object: id, mask: vec![1.0; vertices] },
                Command::HideFaces { object: id, triangles: half, hide: true },
            ],
        );
        assert!(marked.changed);
        let view = access.mesh(id).unwrap();
        assert!(view.mask.iter().all(|&m| m == 1.0) && view.hidden_triangles.is_some());

        let replacement = primitives::build(Primitive::Cube, 600, 0.5);
        let rebuilt = TriMesh::new(replacement.positions.clone(), replacement.indices.clone());
        let data = kit::MeshData::Plain { mesh: rebuilt, mask: None };
        let receipt = kit::scene::apply(&mut access, "Merge", vec![Command::Replace { object: id, mesh: data }]);
        assert!(receipt.changed);
        let view = access.mesh(id).unwrap();
        assert_eq!(view.mask.len(), view.vertex_count());
        assert!(view.mask.iter().all(|&m| m == 0.0), "the mask is clear");
        assert!(view.hidden_triangles.is_none() && view.hidden_vertices.is_none(), "nothing is hidden");
        assert_eq!(view.visible_triangle_count(), view.triangle_count());

        // Undo is a step on the object list, so it brings the old mesh back as it was, mask and hidden
        // faces with it (the Unity app's undo of a topology change left the mask blank).
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert!(mesh.mask.iter().all(|&m| m == 1.0), "the old mask is back");
        assert_eq!(mesh.hidden_triangle_count(), hidden, "and the old hidden faces");
    }

    /// What a tool makes or rebuilds (a trim, a split, a merge, built keys) is looked over for holes
    /// when its commit lands, and the warning names the objects that are not closed and no others.
    #[test]
    fn what_a_tool_makes_or_rebuilds_is_looked_over_for_holes() {
        let mut mesh = cube();
        let mut scene = Scene::new("Cube");
        let id = ObjectId(scene.active_id());
        let mut history = History::default();
        history.target = scene.active_mesh();
        let mut sculptor = Sculptor::default();
        let mut access = Access {
            scene: &mut scene,
            mesh: &mut mesh,
            history: &mut history,
            sculptor: &mut sculptor,
            before: None,
            rebuilt: Vec::new(),
        };
        let whole = cube();
        let plain = |indices: &[u32]| kit::MeshData::from(TriMesh::new(whole.positions.clone(), indices.to_vec()));

        // A commit of closed meshes, with edits that rebuild nothing: every object it gave a mesh is
        // listed for the check, and the check has nothing to say.
        let commands = vec![
            Command::Replace { object: id, mesh: plain(&whole.indices) },
            Command::Add(kit::NewObject::new("Part", plain(&whole.indices)).unselected()),
            Command::Rename { object: id, name: "Body".into() },
        ];
        let receipt = kit::scene::apply(&mut access, "Split", commands);
        let part = receipt.added[0];
        assert_eq!(access.rebuilt, [id.0, part.0]);
        assert_eq!(print_warning(access.scene, access.mesh, &access.rebuilt), None);

        // The part replaced by a cube with two of its triangles missing: it is named, the body is not.
        access.rebuilt.clear();
        let open = kit::MeshData::from(TriMesh::new(whole.positions.clone(), whole.indices[6..].to_vec()));
        kit::scene::apply(&mut access, "Trim", vec![Command::Replace { object: part, mesh: open }]);
        assert_eq!(access.rebuilt, [part.0]);
        let both = [id.0, part.0];
        let warning = print_warning(access.scene, access.mesh, &both).expect("the part has a hole in it");
        assert!(warning.starts_with("\"Part\" has ") && warning.contains(" open edges ("), "{warning}");
        assert!(warning.ends_with(": it may not print. A Remesh (R) usually closes it."), "{warning}");
        assert!(!warning.contains("Body"), "{warning}");

        // An instance shows the same mesh, and is not named a second time; an object that has gone is passed over.
        let part_mesh = access.scene.get(part.0).unwrap().mesh;
        access.scene.select(part.0, access.mesh);
        let twin = access.scene.instance_active();
        assert_eq!(access.scene.get(twin).unwrap().mesh, part_mesh);
        assert_eq!(print_warning(access.scene, access.mesh, &[part.0, twin, u64::MAX]), Some(warning));
    }

    /// Symmetry belongs to objects, and that is what tools are shown: any
    /// object's own on asking, the piece a tool cuts from an object starting
    /// with that object's, and a change going to the object that was selected
    /// when the tool asked for it.
    #[test]
    fn tools_read_and_set_each_objects_own_symmetry() {
        use crate::scene::{Space, Symmetry};

        let mut mesh = cube();
        let mut scene = Scene::new("Body");
        let body = scene.active_id();
        let radial = Radial { on: true, count: 5, axis: RadialAxis::Z, ..Radial::default() };
        scene.set_symmetry(body, Symmetry { mirror: [true, false, false], radial, space: Space::Local });
        let head = scene.add("Head", cube(), &mut mesh);
        scene.get_mut(head).unwrap().position = Vec3::new(0.0, 2.0, 0.0);
        scene.set_symmetry(head, Symmetry { mirror: [false, false, true], space: Space::World, ..Symmetry::default() });
        let mut history = History::default();
        history.target = scene.active_mesh();
        let mut sculptor = Sculptor::default();
        let mut access = Access {
            scene: &mut scene,
            mesh: &mut mesh,
            history: &mut history,
            sculptor: &mut sculptor,
            before: None,
            rebuilt: Vec::new(),
        };
        let (a, b) = (ObjectId(body), ObjectId(head));

        // Each object's own, whichever is selected (the head is).
        let own = access.symmetry(a);
        assert_eq!((own.mirror, own.radial_count, own.radial_axis, own.world), ([true, false, false], 5, Vec3::Z, None));
        assert_eq!(own.group().len(), 10);
        let fixed = access.symmetry(b);
        assert_eq!((fixed.mirror, fixed.radial_count), ([false, false, true], 1));
        assert_eq!(fixed.world.map(|at| at.position), Some(Vec3::new(0.0, 2.0, 0.0)), "where the head stands");
        assert_eq!(access.symmetry(ObjectId(u64::MAX)), kit::Symmetry::NONE);

        // A piece cut from the body starts with the body's settings, all of
        // them; an object made from nothing starts with none.
        let shape = access.mesh(a).unwrap().to_tri_mesh();
        let part = kit::NewObject::new("Body Part", shape.clone()).unselected().with_symmetry_of(a);
        let fresh = kit::NewObject::new("Lathe", shape);
        let receipt = kit::scene::apply(&mut access, "Split", vec![Command::Add(part), Command::Add(fresh)]);
        let (part, fresh) = (receipt.added[0], receipt.added[1]);
        assert_eq!(access.scene.symmetry(part.0), access.scene.symmetry(body));
        assert_eq!(access.symmetry(fresh), kit::Symmetry::NONE);
        assert_eq!(access.active(), fresh, "the new object is the selected one");

        // The Lathe's Radial Symmetry on Create, asked for right after its
        // commit: it is the new object's symmetry that is set.
        let mut out = Outbox { symmetry_of: Some(fresh), ..Outbox::default() };
        out.radial = Some(kit::RadialChange { on: Some(true), axis: Some(Vec3::Y), count: None });
        take_symmetry(access.scene, &out);
        let made = access.scene.symmetry(fresh.0);
        assert_eq!((made.radial.on, made.radial.count, made.radial.axis, made.mirror), (true, 8, RadialAxis::Y, [false; 3]));
        assert_eq!(access.scene.symmetry(head).mirror, [false, false, true], "the object selected before keeps its own");
        assert_eq!(access.scene.symmetry(body).radial, radial);
        // With no object named, it is the selected one's.
        take_symmetry(access.scene, &Outbox { mirror: Some([true, true, false]), ..Outbox::default() });
        assert_eq!(access.scene.symmetry(fresh.0).mirror, [true, true, false]);

        // Undoing the commit takes the two objects away; their symmetry was
        // never an undo step, and the ones that stay have what they had.
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(scene.objects.len(), 2);
        assert_eq!((scene.symmetry(body).mirror, scene.symmetry(head).mirror), ([true, false, false], [false, false, true]));
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert_eq!(scene.symmetry(fresh.0).mirror, [true, true, false], "redone, the object is back as it was left");
    }

    /// What a tool's context does with the document, without the window.
    #[test]
    fn a_tool_reads_the_selection_and_hides_faces_as_steps_of_their_own() {
        let mut mesh = cube();
        let mut scene = Scene::new("Cube");
        let first = scene.active_id();
        let second = scene.add("Cube", cube(), &mut mesh);
        let third = scene.add("Cube", cube(), &mut mesh);
        // Picked third, then first; the second is the active one.
        scene.select(second, &mut mesh);
        scene.select_also(third);
        scene.select_also(first);
        let mut history = History::default();
        history.target = scene.active_mesh();
        let mut sculptor = Sculptor::default();
        let mut access = Access {
            scene: &mut scene,
            mesh: &mut mesh,
            history: &mut history,
            sculptor: &mut sculptor,
            before: None,
            rebuilt: Vec::new(),
        };
        let (a, b, c) = (ObjectId(first), ObjectId(second), ObjectId(third));
        assert_eq!((access.active(), access.selected()), (b, vec![b, c, a]));

        let view = access.mesh(a).unwrap();
        assert!(view.hidden_triangles.is_none() && view.hidden_vertices.is_none());
        let (count, vertices) = (view.triangle_count(), view.vertex_count());
        let half: Vec<u32> = (0..count as u32 / 2).collect();
        let hide = |triangles: &[u32], hide: bool| vec![Command::HideFaces { object: a, triangles: triangles.to_vec(), hide }];

        // Hiding faces of an object brings it to hand, like a mask does.
        let receipt = kit::scene::apply(&mut access, "Hide", hide(&half, true));
        assert_eq!((receipt.changed, receipt.failed), (true, 0));
        assert_eq!(access.active(), a);
        let view = access.mesh(a).unwrap();
        assert_eq!(view.visible_triangle_count(), count - half.len());
        assert_eq!((view.hidden_triangles.unwrap().len(), view.hidden_vertices.unwrap().len()), (count, vertices));
        assert!(view.is_triangle_hidden(0) && !view.is_triangle_hidden(count - 1));
        assert!(view.hidden_vertices.unwrap().contains(&true), "some vertices are on hidden faces only");
        assert!(access.mesh(b).unwrap().hidden_triangles.is_none(), "another object's mesh is untouched");

        // Nothing new to hide, and the whole object, are both refused.
        let all: Vec<u32> = (0..count as u32).collect();
        for triangles in [&half, &all] {
            let receipt = kit::scene::apply(&mut access, "Hide", hide(triangles, true));
            assert_eq!((receipt.changed, receipt.failed), (false, 1));
        }
        assert_eq!(access.mesh(a).unwrap().visible_triangle_count(), count - half.len());

        let receipt = kit::scene::apply(&mut access, "Invert", vec![Command::InvertFaces { object: a }]);
        assert!(receipt.changed);
        assert_eq!(access.mesh(a).unwrap().visible_triangle_count(), half.len());
        let receipt = kit::scene::apply(&mut access, "Show", hide(&half[..1], false));
        assert_eq!((receipt.changed, receipt.failed), (false, 1), "those faces were showing already");
        let receipt = kit::scene::apply(&mut access, "Show All", vec![Command::ShowAllFaces { object: a }]);
        assert!(receipt.changed);
        assert!(access.mesh(a).unwrap().hidden_triangles.is_none());
        for command in [Command::ShowAllFaces { object: a }, Command::InvertFaces { object: a }] {
            let receipt = kit::scene::apply(&mut access, "Nothing", vec![command]);
            assert_eq!((receipt.changed, receipt.failed), (false, 1), "nothing is hidden");
        }
        let gone = kit::scene::apply(&mut access, "Hide", vec![Command::ShowAllFaces { object: ObjectId(u64::MAX) }]);
        assert_eq!((gone.changed, gone.failed), (false, 1));

        // Three steps on the mesh, each undone on its own.
        assert_eq!(history.steps(), (3, 0));
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(mesh.visible_triangle_count(), half.len());
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert_eq!(mesh.visible_triangle_count(), count - half.len());
        assert_eq!(history.undo(&mut mesh), Some(false));
        assert!(!mesh.any_hidden());
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert_eq!(mesh.hidden_triangle_count(), half.len());
    }
}
