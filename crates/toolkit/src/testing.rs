//! A stand-in for the app, so a tool can be driven and checked in a unit
//! test with no window, GPU or person: a small scene with real undo, a
//! camera, and the same routing of presses, drags and clicks the app does.
//!
//! ```ignore
//! let mut bench = Bench::new();
//! let cube = bench.add("Cube", testing::cube(0.5));
//! let mut tool = MyTool::default();
//! bench.enter(&mut tool);
//! bench.drag(&mut tool, bench.px(Vec3::new(-0.2, 0.0, 0.5)), bench.px(Vec3::new(0.2, 0.0, 0.5)));
//! let draw = bench.frame(&mut tool);
//! ```

use std::collections::HashMap;

use glam::{Mat3, Mat4, Vec2, Vec3};
use sculpt_geom::{Aabb, TriMesh, tri};

use crate::ctx::{Ctx, Env, Outbox};
use crate::draw::{Draw, smooth_normals};
use crate::jobs::Jobs;
use sculpt_symmetry::MirrorRepeater;

use crate::scene::{Document, Hit, MeshData, MeshId, MeshView, NewObject, ObjectId, ObjectInfo, Placement, Scene};
use crate::tool::{Button, Event, GizmoPhase, Key, Tool};
use crate::view::{Ray, View};

/// An axis-aligned cube about the origin, `half` from centre to face.
pub fn cube(half: f32) -> TriMesh {
    let corner = |i: usize| Vec3::new((i & 1) as f32, (i >> 1 & 1) as f32, (i >> 2 & 1) as f32) * 2.0 - Vec3::ONE;
    let positions = (0..8).map(|i| corner(i) * half).collect();
    #[rustfmt::skip]
    let indices = vec![
        0, 2, 1, 1, 2, 3, // -z
        4, 5, 6, 5, 7, 6, // +z
        0, 1, 4, 1, 5, 4, // -y
        2, 6, 3, 3, 6, 7, // +y
        0, 4, 2, 2, 4, 6, // -x
        1, 3, 5, 3, 7, 5, // +x
    ];
    TriMesh::new(positions, indices)
}

#[derive(Clone, Debug, PartialEq)]
struct Object {
    id: ObjectId,
    name: String,
    visible: bool,
    mesh: MeshId,
    placement: Placement,
    mirror: Option<MirrorRepeater>,
}

struct Geometry {
    mesh: TriMesh,
    normals: Vec<Vec3>,
    mask: Vec<f32>,
    /// One flag per triangle, or empty while nothing is hidden.
    hidden: Vec<bool>,
    /// One flag per vertex, kept in step with `hidden`: a vertex is hidden
    /// when every triangle on it is.
    hidden_verts: Vec<bool>,
}

impl Geometry {
    /// Sets which triangles are hidden; all false is stored as nothing
    /// hidden.
    fn set_hidden(&mut self, hidden: Vec<bool>) {
        if !hidden.contains(&true) {
            (self.hidden, self.hidden_verts) = (Vec::new(), Vec::new());
            return;
        }
        let count = self.mesh.positions.len();
        let (mut used, mut shown) = (vec![false; count], vec![false; count]);
        for (t, corners) in self.mesh.indices.chunks_exact(3).enumerate() {
            for &v in corners {
                used[v as usize] = true;
                shown[v as usize] |= !hidden[t];
            }
        }
        self.hidden_verts = (0..count).map(|v| used[v] && !shown[v]).collect();
        self.hidden = hidden;
    }
}

enum Step {
    /// The object list and selection on the other side of the step.
    List(Vec<Object>, ObjectId),
    /// A mesh's mask on the other side of the step.
    Mask(MeshId, Vec<f32>),
    /// A mesh's hidden triangles on the other side of the step.
    Faces(MeshId, Vec<bool>),
}

/// The bench's scene: objects, their geometry, and an undo history that
/// works the way the app's does.
pub struct TestDoc {
    objects: Vec<Object>,
    meshes: HashMap<MeshId, Geometry>,
    active: ObjectId,
    /// The objects selected along with the active one, in the order they
    /// were added (`Bench::select`).
    also: Vec<ObjectId>,
    next: u64,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The list as it was at `begin`, while an edit is open.
    before: Option<(Vec<Object>, ObjectId)>,
    /// Every call the document received, in order: "begin Split", "replace 1",
    /// "add Half", "end", "mask 1".
    pub log: Vec<String>,
}

impl TestDoc {
    fn new() -> TestDoc {
        TestDoc {
            objects: Vec::new(),
            meshes: HashMap::new(),
            active: ObjectId(0),
            also: Vec::new(),
            next: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            before: None,
            log: Vec::new(),
        }
    }

    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next - 1
    }

    fn store(&mut self, data: MeshData) -> Option<MeshId> {
        let MeshData::Plain { mesh, mask } = data else { return None };
        let normals = smooth_normals(&mesh.positions, &mesh.indices);
        let mask = mask.filter(|m| m.len() == mesh.positions.len()).unwrap_or_else(|| vec![0.0; mesh.positions.len()]);
        let id = MeshId(self.fresh());
        self.meshes.insert(id, Geometry { mesh, normals, mask, hidden: Vec::new(), hidden_verts: Vec::new() });
        Some(id)
    }

    fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    fn unique_name(&self, base: &str) -> String {
        let taken = |name: &str| self.objects.iter().any(|o| o.name == name);
        if !taken(base) {
            return base.to_owned();
        }
        (2..).map(|n| format!("{base} ({n})")).find(|name| !taken(name)).unwrap()
    }

    /// Where the ray meets the object's mesh drawn through `model`.
    fn hit_placed(&self, object: &Object, model: Mat4, ray: &Ray) -> Option<Hit> {
        let geometry = self.meshes.get(&object.mesh)?;
        let inverse = model.inverse();
        let local = ray.transformed(&inverse);
        let mut best: Option<(f32, Vec3)> = None;
        for t in 0..geometry.mesh.triangle_count() {
            // Hidden faces are not there to be hit.
            if geometry.hidden.get(t).copied().unwrap_or(false) {
                continue;
            }
            let [a, b, c] = geometry.mesh.triangle(t);
            if let Some(d) = tri::ray_hit(local.origin, local.dir, a, b, c) {
                if d >= 0.0 && best.is_none_or(|(nearest, _)| d < nearest) {
                    best = Some((d, (b - a).cross(c - a)));
                }
            }
        }
        let (d, normal) = best?;
        let position = model.transform_point3(local.at(d));
        let mut normal = (Mat3::from_mat4(inverse).transpose() * normal).normalize_or(Vec3::Y);
        if normal.dot(ray.dir) > 0.0 {
            normal = -normal;
        }
        Some(Hit { object: object.id, position, normal, distance: (position - ray.origin).dot(ray.dir) })
    }

    fn info_of<'a>(&'a self, o: &'a Object, geometry: &Geometry) -> ObjectInfo<'a> {
        ObjectInfo {
            id: o.id,
            name: &o.name,
            visible: o.visible,
            active: o.id == self.active,
            mesh: o.mesh,
            placement: o.placement,
            mirror: o.mirror,
            vertices: geometry.mesh.vertex_count(),
            triangles: geometry.mesh.triangle_count(),
        }
    }

    /// The nearest hit on an object or on any of its live mirror copies, as
    /// the app picks them.
    fn hit_object(&self, object: &Object, ray: &Ray) -> Option<Hit> {
        let geometry = self.meshes.get(&object.mesh)?;
        let places = self.info_of(object, geometry).placements();
        places
            .into_iter()
            .filter_map(|(_, model)| self.hit_placed(object, model, ray))
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }

    /// How many undo steps are held.
    pub fn undo_steps(&self) -> usize {
        self.undo.len()
    }

    /// Changes which triangles of an object's mesh are hidden, as the app
    /// does: a step of its own, refused when it changes nothing or would
    /// leave none of the object showing.
    fn change_faces(&mut self, object: ObjectId, change: impl FnOnce(&mut [bool])) -> bool {
        let Some(mesh) = self.get(object).map(|o| o.mesh) else { return false };
        let Some(geometry) = self.meshes.get_mut(&mesh) else { return false };
        let before = geometry.hidden.clone();
        let mut hidden = before.clone();
        hidden.resize(geometry.mesh.triangle_count(), false);
        let was = hidden.clone();
        change(&mut hidden);
        if hidden == was || !hidden.contains(&false) {
            return false;
        }
        geometry.set_hidden(hidden);
        if self.active != object {
            self.active = object;
            self.also.clear();
        }
        self.undo.push(Step::Faces(mesh, before));
        self.redo.clear();
        true
    }

    fn step(&mut self, undo: bool) -> bool {
        let Some(step) = (if undo { self.undo.pop() } else { self.redo.pop() }) else { return false };
        let other = match step {
            Step::List(objects, active) => {
                let now = Step::List(std::mem::replace(&mut self.objects, objects), self.active);
                self.active = active;
                now
            }
            Step::Mask(mesh, mask) => match self.meshes.get_mut(&mesh) {
                Some(geometry) => Step::Mask(mesh, std::mem::replace(&mut geometry.mask, mask)),
                None => return false,
            },
            Step::Faces(mesh, hidden) => match self.meshes.get_mut(&mesh) {
                Some(geometry) => {
                    let now = std::mem::take(&mut geometry.hidden);
                    geometry.set_hidden(hidden);
                    Step::Faces(mesh, now)
                }
                None => return false,
            },
        };
        if undo { self.redo.push(other) } else { self.undo.push(other) }
        true
    }
}

impl Scene for TestDoc {
    fn objects(&self) -> Vec<ObjectId> {
        self.objects.iter().map(|o| o.id).collect()
    }

    fn info(&self, id: ObjectId) -> Option<ObjectInfo<'_>> {
        let o = self.get(id)?;
        let geometry = self.meshes.get(&o.mesh)?;
        Some(self.info_of(o, geometry))
    }

    fn active(&self) -> ObjectId {
        self.active
    }

    fn selected(&self) -> Vec<ObjectId> {
        let others = self.also.iter().copied().filter(|&id| id != self.active && self.get(id).is_some());
        std::iter::once(self.active).chain(others).collect()
    }

    fn mesh(&self, id: ObjectId) -> Option<MeshView<'_>> {
        let o = self.get(id)?;
        let geometry = self.meshes.get(&o.mesh)?;
        Some(MeshView {
            id: o.mesh,
            positions: &geometry.mesh.positions,
            normals: &geometry.normals,
            indices: &geometry.mesh.indices,
            mask: &geometry.mask,
            hidden_triangles: (!geometry.hidden.is_empty()).then_some(&geometry.hidden[..]),
            hidden_vertices: (!geometry.hidden.is_empty()).then_some(&geometry.hidden_verts[..]),
        })
    }

    fn raycast(&self, ray: &Ray) -> Option<Hit> {
        self.objects
            .iter()
            .filter(|o| o.visible)
            .filter_map(|o| self.hit_object(o, ray))
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }

    fn raycast_object(&self, id: ObjectId, ray: &Ray) -> Option<Hit> {
        let o = self.get(id)?;
        self.hit_placed(o, o.placement.matrix(), ray)
    }

    fn raycast_placed(&self, id: ObjectId, model: &Mat4, ray: &Ray) -> Option<Hit> {
        self.hit_placed(self.get(id)?, *model, ray)
    }

    fn bounds(&self, id: ObjectId) -> Aabb {
        let Some(o) = self.get(id) else { return Aabb::EMPTY };
        let Some(geometry) = self.meshes.get(&o.mesh) else { return Aabb::EMPTY };
        let model = o.placement.matrix();
        geometry.mesh.positions.iter().fold(Aabb::EMPTY, |mut b, p| {
            b.grow(model.transform_point3(*p));
            b
        })
    }
}

impl Document for TestDoc {
    fn begin(&mut self, label: &str) {
        self.log.push(format!("begin {label}"));
        self.before = Some((self.objects.clone(), self.active));
    }

    fn add(&mut self, object: NewObject) -> Option<ObjectId> {
        self.log.push(format!("add {}", object.name));
        let mesh = self.store(object.mesh)?;
        let id = ObjectId(self.fresh());
        let name = self.unique_name(&object.name);
        self.objects.push(Object {
            id,
            name,
            visible: object.visible,
            mesh,
            placement: object.placement,
            mirror: object.mirror,
        });
        if object.select || self.objects.len() == 1 {
            self.active = id;
            self.also.clear();
        }
        Some(id)
    }

    fn replace(&mut self, object: ObjectId, mesh: MeshData) -> bool {
        self.log.push(format!("replace {}", object.0));
        let Some(old) = self.get(object).map(|o| o.mesh) else { return false };
        let Some(new) = self.store(mesh) else { return false };
        // Instances share a mesh, so they all follow.
        for o in self.objects.iter_mut().filter(|o| o.mesh == old) {
            o.mesh = new;
        }
        true
    }

    fn place(&mut self, object: ObjectId, placement: Placement) -> bool {
        self.log.push(format!("place {}", object.0));
        self.get_mut(object).map(|o| o.placement = placement).is_some()
    }

    fn set_mirror(&mut self, object: ObjectId, mirror: Option<MirrorRepeater>) -> bool {
        self.log.push(format!("mirror {}", object.0));
        match self.get_mut(object) {
            Some(o) if o.mirror != mirror => {
                o.mirror = mirror;
                true
            }
            _ => false,
        }
    }

    fn show(&mut self, object: ObjectId, visible: bool) -> bool {
        self.log.push(format!("show {} {visible}", object.0));
        self.get_mut(object).map(|o| o.visible = visible).is_some()
    }

    fn rename(&mut self, object: ObjectId, name: String) -> bool {
        self.log.push(format!("rename {}", object.0));
        self.get_mut(object).map(|o| o.name = name).is_some()
    }

    fn delete(&mut self, object: ObjectId) -> bool {
        self.log.push(format!("delete {}", object.0));
        let Some(row) = self.objects.iter().position(|o| o.id == object) else { return false };
        if self.objects.len() < 2 {
            return false;
        }
        self.objects.remove(row);
        if self.active == object {
            self.active = self.objects[row.min(self.objects.len() - 1)].id;
        }
        true
    }

    fn select(&mut self, object: ObjectId) -> bool {
        self.log.push(format!("select {}", object.0));
        if self.get(object).is_none() {
            return false;
        }
        // Selecting one object drops any others, as a click on its row does.
        self.active = object;
        self.also.clear();
        true
    }

    fn end(&mut self) -> bool {
        self.log.push("end".to_owned());
        let Some((objects, active)) = self.before.take() else { return false };
        // A change of selection alone is not something to undo.
        if objects == self.objects {
            return false;
        }
        self.undo.push(Step::List(objects, active));
        self.redo.clear();
        true
    }

    fn mask(&mut self, object: ObjectId, mask: Vec<f32>) -> bool {
        self.log.push(format!("mask {}", object.0));
        let Some(mesh) = self.get(object).map(|o| o.mesh) else { return false };
        let Some(geometry) = self.meshes.get_mut(&mesh) else { return false };
        if mask.len() != geometry.mask.len() {
            return false;
        }
        if self.active != object {
            self.active = object;
            self.also.clear();
        }
        self.undo.push(Step::Mask(mesh, std::mem::replace(&mut geometry.mask, mask)));
        self.redo.clear();
        true
    }

    fn hide_faces(&mut self, object: ObjectId, triangles: &[u32], hide: bool) -> bool {
        self.log.push(format!("{} faces {}", if hide { "hide" } else { "show" }, object.0));
        self.change_faces(object, |hidden| {
            for &t in triangles {
                if let Some(flag) = hidden.get_mut(t as usize) {
                    *flag = hide;
                }
            }
        })
    }

    fn show_all_faces(&mut self, object: ObjectId) -> bool {
        self.log.push(format!("show all faces {}", object.0));
        self.change_faces(object, |hidden| hidden.fill(false))
    }

    fn invert_faces(&mut self, object: ObjectId) -> bool {
        self.log.push(format!("invert faces {}", object.0));
        // With nothing hidden the swap would hide everything, which
        // `change_faces` refuses.
        self.change_faces(object, |hidden| hidden.iter_mut().for_each(|flag| *flag = !*flag))
    }
}

/// A headless host: build a scene, drive a tool, look at what it did.
pub struct Bench {
    /// The moment the tool is shown: camera, pointer, modifiers, unit. Set
    /// fields directly (`bench.env.mods.shift = true`).
    pub env: Env,
    pub doc: TestDoc,
    pub jobs: Jobs,
    /// The last status line a tool set, and whether it was good news.
    pub status: Option<(String, bool)>,
    /// Every toast, in order.
    pub toasts: Vec<String>,
    /// Whether the tool asked to be brought up.
    pub started: bool,
    /// Whether the tool asked to be put away.
    pub finished: bool,
    /// The box the tool last asked the camera to frame.
    pub framed: Option<Aabb>,
    /// Whether the tool took the press that is down.
    captured: bool,
    down: Option<(Button, Vec2)>,
}

impl Default for Bench {
    fn default() -> Bench {
        Bench::new()
    }
}

impl Bench {
    /// An empty scene seen from the front: the camera 5 units out on +z
    /// looking at the origin, an 800 x 600 viewport.
    pub fn new() -> Bench {
        let view = View::perspective(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, 60.0, Vec2::new(800.0, 600.0));
        Bench {
            env: Env::at_rest(view),
            doc: TestDoc::new(),
            jobs: Jobs::plain(),
            status: None,
            toasts: Vec::new(),
            started: false,
            finished: false,
            framed: None,
            captured: false,
            down: None,
        }
    }

    /// Adds an object to the scene (not an undo step) and selects it.
    pub fn add(&mut self, name: &str, mesh: TriMesh) -> ObjectId {
        let id = self.doc.add(NewObject::new(name, mesh)).expect("plain meshes can always be added");
        self.doc.log.clear();
        id
    }

    /// Gives an object live mirror copies across these world axes (X = 1,
    /// Y = 2, Z = 4) through the world's centre, as the Mirror section of the
    /// Objects card does; 0 takes them away. Not an undo step.
    pub fn mirror(&mut self, object: ObjectId, axes: u8) {
        self.doc.set_mirror(object, MirrorRepeater::new(axes, Vec3::ZERO));
        self.doc.log.clear();
    }

    /// Makes `instance` a second object showing the same mesh as `of`, at
    /// `placement`, as Instance in the Objects card does. Not an undo step.
    pub fn instance(&mut self, of: ObjectId, name: &str, placement: Placement) -> ObjectId {
        let mesh = self.doc.get(of).expect("an object to make an instance of").mesh;
        let id = ObjectId(self.doc.fresh());
        self.doc.objects.push(Object { id, name: name.to_owned(), visible: true, mesh, placement, mirror: None });
        id
    }

    /// Selects several objects, as Ctrl + clicking their rows does: the
    /// first becomes the active one and the rest are selected along with it,
    /// which is what `Scene::selected` then lists. Not an undo step.
    pub fn select(&mut self, objects: &[ObjectId]) {
        let Some((&first, rest)) = objects.split_first() else { return };
        if self.doc.get(first).is_some() {
            self.doc.active = first;
            self.doc.also = rest.to_vec();
        }
    }

    /// The pixel a world point is drawn at.
    pub fn px(&self, world: Vec3) -> Vec2 {
        self.env.view.project(world).expect("the point is in front of the camera")
    }

    /// Runs `f` with a context, then takes in what the tool asked for.
    pub fn call<R>(&mut self, f: impl FnOnce(&mut Ctx) -> R) -> R {
        let mut out = Outbox::default();
        let no_icons = |_: &str| None;
        let result = f(&mut Ctx::new(self.env, &mut self.doc, &self.jobs, &no_icons, &mut out));
        if let Some(status) = out.status {
            self.status = Some(status);
        }
        self.toasts.extend(out.toast);
        self.started |= out.start;
        self.finished |= out.finish;
        if let Some(unit) = out.unit {
            self.env.unit = unit;
        }
        if let Some(mirror) = out.mirror {
            self.env.mirror = mirror;
        }
        if out.frame.is_some() {
            self.framed = out.frame;
        }
        result
    }

    pub fn enter(&mut self, tool: &mut dyn Tool) {
        self.finished = false;
        self.call(|cx| tool.enter(cx));
    }

    pub fn leave(&mut self, tool: &mut dyn Tool) {
        self.call(|cx| tool.leave(cx));
    }

    /// Sends one event with the pointer where it is.
    pub fn send(&mut self, tool: &mut dyn Tool, event: Event) -> bool {
        self.call(|cx| tool.input(cx, event))
    }

    /// Moves the pointer with no button down.
    pub fn hover(&mut self, px: Vec2) {
        self.env.pointer = px;
    }

    /// A button goes down at `px`. Returns whether the tool took the drag.
    pub fn press(&mut self, tool: &mut dyn Tool, px: Vec2, button: Button) -> bool {
        self.env.pointer = px;
        self.captured = self.send(tool, Event::Press(button));
        self.down = Some((button, px));
        self.captured
    }

    /// The pointer moves with the button down. A drag the tool passed on
    /// would orbit the camera in the app; here it only stops being a click.
    pub fn drag_to(&mut self, tool: &mut dyn Tool, px: Vec2) {
        self.env.pointer = px;
        if self.captured {
            self.send(tool, Event::Drag);
        } else if let Some((_, from)) = self.down {
            if from.distance(px) > CLICK_SLOP {
                self.down = None;
            }
        }
    }

    /// The button comes up: the end of the drag, or a click if the tool had
    /// passed the press on and the pointer never moved.
    pub fn release(&mut self, tool: &mut dyn Tool) {
        if std::mem::take(&mut self.captured) {
            self.send(tool, Event::Release);
        } else if let Some((button, _)) = self.down {
            self.send(tool, Event::Click(button));
        }
        self.down = None;
    }

    /// Press and release in one place.
    pub fn click(&mut self, tool: &mut dyn Tool, px: Vec2, button: Button) {
        self.press(tool, px, button);
        self.release(tool);
    }

    /// Press at `from`, move to `to` in a few steps, release.
    pub fn drag(&mut self, tool: &mut dyn Tool, from: Vec2, to: Vec2) {
        self.press(tool, from, Button::Left);
        for i in 1..=8 {
            self.drag_to(tool, from.lerp(to, i as f32 / 8.0));
        }
        self.release(tool);
    }

    /// Drags the transform gizmo on the tool's own target (`Tool::gizmo`)
    /// until the target is at `to`, reported the way the app reports it:
    /// `Begin` with where the target was, a `Move` part of the way and one
    /// all the way, then `End`. What the target cannot do (a point has no
    /// turn or size) is left as it was, since no handle would have changed
    /// it. Returns false, and sends nothing, if the tool offers no target.
    pub fn gizmo(&mut self, tool: &mut dyn Tool, to: Placement) -> bool {
        let Some(target) = tool.gizmo() else { return false };
        let from = target.placement;
        let to = Placement {
            position: if target.moves { to.position } else { from.position },
            rotation: if target.turns { to.rotation } else { from.rotation },
            scale: if target.scales { to.scale } else { from.scale },
        };
        let halfway = Placement {
            position: from.position.lerp(to.position, 0.5),
            rotation: from.rotation.slerp(to.rotation, 0.5),
            scale: from.scale.lerp(to.scale, 0.5),
        };
        self.send(tool, Event::Gizmo(from, GizmoPhase::Begin));
        self.send(tool, Event::Gizmo(halfway, GizmoPhase::Move));
        self.send(tool, Event::Gizmo(to, GizmoPhase::Move));
        self.send(tool, Event::Gizmo(to, GizmoPhase::End));
        true
    }

    /// A key goes down and comes up. Returns whether the tool took it.
    pub fn key(&mut self, tool: &mut dyn Tool, key: Key) -> bool {
        let taken = self.send(tool, Event::KeyDown(key));
        self.send(tool, Event::KeyUp(key));
        taken
    }

    /// One frame: what the tool draws.
    pub fn frame(&mut self, tool: &mut dyn Tool) -> Draw {
        let mut draw = Draw::default();
        self.call(|cx| tool.frame(cx, &mut draw));
        self.env.time += self.env.dt as f64;
        draw
    }

    /// Z, as the app routes it: the tool's own history first, then the
    /// scene's. Returns whether anything was stepped back.
    pub fn undo(&mut self, tool: &mut dyn Tool) -> bool {
        self.send(tool, Event::Undo) || self.doc.step(true)
    }

    pub fn redo(&mut self, tool: &mut dyn Tool) -> bool {
        self.send(tool, Event::Redo) || self.doc.step(false)
    }

    /// Z with no tool up: steps the scene's history.
    pub fn undo_scene(&mut self) -> bool {
        self.doc.step(true)
    }

    pub fn redo_scene(&mut self) -> bool {
        self.doc.step(false)
    }
}

/// How far (pixels) a press may wander and still be a click: the app's own
/// figure.
pub const CLICK_SLOP: f32 = 4.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Command, apply};

    fn mask_of(bench: &Bench, id: ObjectId) -> Vec<f32> {
        bench.doc.mesh(id).unwrap().mask.to_vec()
    }

    #[test]
    fn list_commands_are_one_step_and_masks_follow_as_their_own() {
        let mut bench = Bench::new();
        let cube_id = bench.add("Cube", cube(0.5));
        let receipt = apply(
            &mut bench.doc,
            "Split",
            vec![
                Command::Mask { object: cube_id, mask: vec![1.0; 8] },
                Command::Replace { object: cube_id, mesh: cube(0.25).into() },
                Command::Add(NewObject::new("Half", cube(0.1))),
                Command::Show { object: cube_id, visible: false },
            ],
        );
        let n = cube_id.0;
        let expected =
            ["begin Split".to_owned(), format!("replace {n}"), "add Half".to_owned(), format!("show {n} false"), "end".to_owned(), format!("mask {n}")];
        assert_eq!(bench.doc.log, expected);
        assert_eq!(receipt.added.len(), 1);
        assert_eq!(receipt.failed, 0);
        assert!(receipt.changed);
        // One step for the list, one for the mask.
        assert_eq!(bench.doc.undo_steps(), 2);
    }

    #[test]
    fn one_undo_takes_back_everything_a_commit_did_to_the_list() {
        let mut bench = Bench::new();
        let a = bench.add("A", cube(0.5));
        let before_mesh = bench.doc.info(a).unwrap().mesh;
        let receipt = bench.call(|cx| {
            cx.commit(
                "Cut",
                vec![
                    Command::Replace { object: a, mesh: cube(0.2).into() },
                    Command::Add(NewObject::new("Piece", cube(0.1)).placed(Placement::at(Vec3::X))),
                    Command::Rename { object: a, name: "Kept".into() },
                ],
            )
        });
        let piece = receipt.added[0];
        assert_eq!(bench.doc.objects(), vec![a, piece]);
        assert_eq!(bench.doc.active(), piece, "a new object is selected unless told otherwise");
        assert_ne!(bench.doc.info(a).unwrap().mesh, before_mesh);
        assert_eq!(bench.doc.info(a).unwrap().name, "Kept");

        assert!(bench.undo_scene());
        assert_eq!(bench.doc.objects(), vec![a]);
        assert_eq!(bench.doc.active(), a);
        assert_eq!(bench.doc.info(a).unwrap().mesh, before_mesh, "the old geometry is back under its old id");
        assert_eq!(bench.doc.info(a).unwrap().name, "A");
        assert!(!bench.undo_scene());

        assert!(bench.redo_scene());
        assert_eq!(bench.doc.objects(), vec![a, piece], "redo brings the piece back under the same id");
        assert!((bench.doc.bounds(a).size().x - 0.4).abs() < 1e-6);
    }

    #[test]
    fn a_failed_command_is_counted_and_the_rest_still_run() {
        let mut bench = Bench::new();
        let a = bench.add("A", cube(0.5));
        let receipt = bench.call(|cx| {
            cx.commit(
                "Mixed",
                vec![
                    Command::Delete { object: ObjectId(999) },
                    Command::Delete { object: a },
                    Command::Mask { object: a, mask: vec![0.5; 3] },
                    Command::Show { object: a, visible: false },
                ],
            )
        });
        // No such object; the last object cannot go; the mask is the wrong length.
        assert_eq!(receipt.failed, 3);
        assert!(receipt.changed);
        assert!(!bench.doc.info(a).unwrap().visible);
    }

    #[test]
    fn selecting_alone_leaves_no_undo_step() {
        let mut bench = Bench::new();
        let a = bench.add("A", cube(0.5));
        let b = bench.add("B", cube(0.5));
        assert_eq!(bench.doc.active(), b);
        let receipt = bench.call(|cx| cx.commit("Pick", vec![Command::Select { object: a }]));
        assert!(!receipt.changed);
        assert_eq!(bench.doc.active(), a);
        assert_eq!(bench.doc.undo_steps(), 0);
    }

    #[test]
    fn a_mask_is_set_and_undone_on_its_own() {
        let mut bench = Bench::new();
        let a = bench.add("A", cube(0.5));
        bench.call(|cx| cx.commit("Mask Top", vec![Command::Mask { object: a, mask: vec![1.0; 8] }]));
        assert_eq!(mask_of(&bench, a), vec![1.0; 8]);
        assert!(bench.undo_scene());
        assert_eq!(mask_of(&bench, a), vec![0.0; 8]);
        assert!(bench.redo_scene());
        assert_eq!(mask_of(&bench, a), vec![1.0; 8]);
    }

    #[test]
    fn faces_are_hidden_shown_and_swapped_each_as_a_step_of_its_own() {
        let mut bench = Bench::new();
        let far = bench.add("Far", cube(0.5));
        let near = bench.add("Near", cube(0.5));
        bench.call(|cx| cx.commit("Place", vec![Command::Place { object: far, placement: Placement::at(Vec3::new(0.0, 0.0, -3.0)) }]));
        let steps = bench.doc.undo_steps();
        let centre = bench.env.view.size * 0.5;
        assert_eq!(bench.call(|cx| cx.pick(centre)).unwrap().object, near);
        let view = bench.doc.mesh(near).unwrap();
        assert!(view.hidden_triangles.is_none() && view.hidden_vertices.is_none());
        assert_eq!(view.visible_triangle_count(), 12);

        // The two triangles of the face toward the camera (+z: 2 and 3).
        let hide = |triangles: Vec<u32>, hide: bool| Command::HideFaces { object: near, triangles, hide };
        let receipt = bench.call(|cx| cx.commit("Hide", vec![hide(vec![2, 3, 3, 99], true)]));
        assert_eq!((receipt.changed, receipt.failed), (true, 0));
        assert_eq!(bench.doc.undo_steps(), steps + 1);
        assert_eq!(bench.doc.active(), near);
        let view = bench.doc.mesh(near).unwrap();
        assert_eq!(view.hidden_triangles.unwrap().iter().filter(|&&h| h).count(), 2);
        assert!(view.is_triangle_hidden(2) && !view.is_triangle_hidden(4) && !view.is_triangle_hidden(99));
        assert_eq!(view.visible_triangle_count(), 10);
        // Every vertex of that face is still on a face that shows.
        assert!(view.hidden_vertices.unwrap().iter().all(|&h| !h) && !view.is_vertex_hidden(0));
        // The pointer goes through the hole to the far inside of the cube.
        let hit = bench.call(|cx| cx.pick(centre)).unwrap();
        assert!(hit.object == near && hit.position.z < 0.0, "{:?}", hit.position);

        // The same hide again changes nothing; so would hiding it all.
        let receipt = bench.call(|cx| cx.commit("Hide", vec![hide(vec![2, 3], true)]));
        assert_eq!((receipt.changed, receipt.failed), (false, 1));
        let receipt = bench.call(|cx| cx.commit("Hide", vec![hide((0..12).collect(), true)]));
        assert_eq!((receipt.changed, receipt.failed), (false, 1));
        assert_eq!(bench.doc.undo_steps(), steps + 1);

        // Swapped: only that face shows, and the vertices off it are hidden.
        let receipt = bench.call(|cx| cx.commit("Invert", vec![Command::InvertFaces { object: near }]));
        assert!(receipt.changed);
        let view = bench.doc.mesh(near).unwrap();
        assert_eq!(view.visible_triangle_count(), 2);
        assert_eq!(view.hidden_vertices.unwrap().iter().filter(|&&h| h).count(), 4);
        assert!(bench.undo_scene());
        assert_eq!(bench.doc.mesh(near).unwrap().visible_triangle_count(), 10);

        // Shown one at a time, then all at once; with nothing hidden there
        // is nothing to show or to swap.
        assert!(bench.call(|cx| cx.commit("Show", vec![hide(vec![2], false)])).changed);
        assert_eq!(bench.doc.mesh(near).unwrap().visible_triangle_count(), 11);
        assert!(bench.call(|cx| cx.commit("Show All", vec![Command::ShowAllFaces { object: near }])).changed);
        assert!(bench.doc.mesh(near).unwrap().hidden_triangles.is_none());
        for command in [Command::ShowAllFaces { object: near }, Command::InvertFaces { object: near }] {
            let receipt = bench.call(|cx| cx.commit("Nothing", vec![command]));
            assert_eq!((receipt.changed, receipt.failed), (false, 1));
        }

        // Each was a step of its own, undone in turn.
        assert_eq!(bench.doc.undo_steps(), steps + 3);
        assert!(bench.undo_scene() && bench.undo_scene());
        assert_eq!(bench.doc.mesh(near).unwrap().visible_triangle_count(), 10);
        assert!(bench.undo_scene());
        assert!(bench.doc.mesh(near).unwrap().hidden_triangles.is_none());
        assert!((bench.call(|cx| cx.pick(centre)).unwrap().position.z - 0.5).abs() < 1e-4);
        assert!(bench.redo_scene());
        assert_eq!(bench.doc.mesh(near).unwrap().visible_triangle_count(), 10);
    }

    #[test]
    fn instances_follow_a_replaced_mesh() {
        let mut bench = Bench::new();
        let a = bench.add("A", cube(0.5));
        let shared = bench.doc.info(a).unwrap().mesh;
        // Hand-make an instance: a second object showing the same mesh.
        let b = ObjectId(bench.doc.fresh());
        bench.doc.objects.push(Object {
            id: b,
            name: "A Instance".into(),
            visible: true,
            mesh: shared,
            placement: Placement::at(Vec3::X),
            mirror: None,
        });
        bench.call(|cx| cx.commit("Remesh", vec![Command::Replace { object: a, mesh: cube(0.1).into() }]));
        assert_eq!(bench.doc.info(a).unwrap().mesh, bench.doc.info(b).unwrap().mesh);
        assert_ne!(bench.doc.info(b).unwrap().mesh, shared);
    }

    #[test]
    fn the_scene_is_hit_through_each_objects_placement() {
        let mut bench = Bench::new();
        let near = bench.add("Near", cube(0.5));
        let far = bench.add("Far", cube(0.5));
        bench.call(|cx| {
            cx.commit(
                "Place",
                vec![Command::Place { object: far, placement: Placement { position: Vec3::new(0.0, 0.0, -3.0), ..Placement::IDENTITY } }],
            )
        });
        let centre = bench.env.view.size * 0.5;
        let hit = bench.call(|cx| cx.pick(centre)).expect("the cube is in front of the camera");
        assert_eq!(hit.object, near);
        assert!(hit.position.distance(Vec3::new(0.0, 0.0, 0.5)) < 1e-4);
        assert!(hit.normal.distance(Vec3::Z) < 1e-4);
        assert!((hit.distance - 4.5).abs() < 1e-4);

        let ray = bench.env.view.ray(centre);
        let behind = bench.doc.raycast_object(far, &ray).unwrap();
        assert!(behind.position.distance(Vec3::new(0.0, 0.0, -2.5)) < 1e-4);
        // Hidden objects are not in the way.
        bench.call(|cx| cx.commit("Hide", vec![Command::Show { object: near, visible: false }]));
        assert_eq!(bench.call(|cx| cx.pick(centre)).unwrap().object, far);
    }

    /// A tool that takes presses on the model and passes the rest on.
    #[derive(Default)]
    struct Probe {
        seen: Vec<Event>,
    }

    impl Tool for Probe {
        fn info(&self) -> crate::tool::Info {
            crate::tool::Info { id: "probe", name: "Probe", icon: "", tip: "", hotkey: None }
        }

        fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
            self.seen.push(event);
            matches!(event, Event::Press(_)) && cx.pick(cx.pointer).is_some()
        }
    }

    #[test]
    fn a_taken_press_gets_drags_and_a_release_and_a_passed_one_becomes_a_click() {
        let mut bench = Bench::new();
        bench.add("Cube", cube(0.5));
        let mut probe = Probe::default();
        let centre = bench.env.view.size * 0.5;

        bench.drag(&mut probe, centre, centre + Vec2::new(40.0, 0.0));
        assert_eq!(probe.seen.first(), Some(&Event::Press(Button::Left)));
        assert_eq!(probe.seen.last(), Some(&Event::Release));
        assert_eq!(probe.seen.iter().filter(|e| **e == Event::Drag).count(), 8);

        // Off the model the tool passes: a still press is a click...
        probe.seen.clear();
        bench.click(&mut probe, Vec2::new(10.0, 10.0), Button::Left);
        assert_eq!(probe.seen, [Event::Press(Button::Left), Event::Click(Button::Left)]);

        // ...and a moving one is the camera's, so the tool hears nothing more.
        probe.seen.clear();
        bench.drag(&mut probe, Vec2::new(10.0, 10.0), Vec2::new(90.0, 10.0));
        assert_eq!(probe.seen, [Event::Press(Button::Left)]);
    }

    #[test]
    fn the_selection_lists_the_active_object_first_and_a_plain_select_drops_the_rest() {
        let mut bench = Bench::new();
        let a = bench.add("A", cube(0.5));
        let b = bench.add("B", cube(0.5));
        let c = bench.add("C", cube(0.5));
        assert_eq!(bench.doc.selected(), [c], "one object selected is a selection of one");
        bench.select(&[b, a]);
        assert_eq!((bench.doc.active(), bench.doc.selected()), (b, vec![b, a]));
        assert_eq!(bench.call(|cx| cx.scene().selected()), [b, a], "a tool reads both operands");
        // An object that is deleted leaves the selection with it.
        bench.call(|cx| cx.commit("Delete", vec![Command::Delete { object: a }]));
        assert_eq!(bench.doc.selected(), [b]);
        bench.select(&[b, c]);
        bench.call(|cx| cx.commit("Pick", vec![Command::Select { object: c }]));
        assert_eq!(bench.doc.selected(), [c]);
    }

    /// A tool with one point of its own, which it puts the gizmo on.
    struct Pin {
        at: Vec3,
        heard: Vec<(Placement, GizmoPhase)>,
    }

    impl Tool for Pin {
        fn info(&self) -> crate::tool::Info {
            crate::tool::Info { id: "pin", name: "Pin", icon: "", tip: "", hotkey: None }
        }

        fn gizmo(&self) -> Option<crate::tool::GizmoTarget> {
            Some(crate::tool::GizmoTarget::point(self.at))
        }

        fn input(&mut self, _cx: &mut Ctx, event: Event) -> bool {
            let Event::Gizmo(placement, phase) = event else { return false };
            self.at = placement.position;
            self.heard.push((placement, phase));
            true
        }
    }

    #[test]
    fn a_gizmo_drag_reports_where_the_target_is_from_press_to_release() {
        let mut bench = Bench::new();
        let mut pin = Pin { at: Vec3::new(0.0, 1.0, 0.0), heard: Vec::new() };
        // Asked to turn and resize a point as well, only the move arrives.
        let to = Placement { position: Vec3::new(2.0, 1.0, 0.0), rotation: glam::Quat::from_rotation_y(1.0), scale: Vec3::splat(3.0) };
        assert!(bench.gizmo(&mut pin, to));
        assert_eq!(pin.at, to.position);
        let phases: Vec<GizmoPhase> = pin.heard.iter().map(|h| h.1).collect();
        assert_eq!(phases, [GizmoPhase::Begin, GizmoPhase::Move, GizmoPhase::Move, GizmoPhase::End]);
        assert_eq!(pin.heard[0].0, Placement::at(Vec3::new(0.0, 1.0, 0.0)), "Begin says where it was");
        assert_eq!(pin.heard[1].0, Placement::at(Vec3::new(1.0, 1.0, 0.0)));
        assert_eq!(pin.heard[3].0, Placement::at(to.position), "End says where it was let go");

        // A tool that offers nothing is sent nothing.
        let mut probe = Probe::default();
        assert!(!bench.gizmo(&mut probe, to));
        assert!(probe.seen.is_empty());
    }

    #[test]
    fn an_objects_mirror_is_read_from_its_info_and_changed_by_a_command_as_part_of_the_commit() {
        let mut bench = Bench::new();
        let a = bench.add("A", cube(0.5));
        bench.call(|cx| cx.commit("Place", vec![Command::Place { object: a, placement: Placement::at(Vec3::new(2.0, 0.0, 0.0)) }]));
        assert_eq!(bench.doc.info(a).unwrap().mirror, None);
        assert_eq!(bench.doc.info(a).unwrap().placements().len(), 1);
        let steps = bench.doc.undo_steps();

        // Mirrored across X as one step with the rest of the commit, and a
        // new object that starts mirrored too.
        let mirror = MirrorRepeater::new(1, Vec3::ZERO);
        let receipt = bench.call(|cx| {
            cx.commit(
                "Mirror",
                vec![
                    Command::Mirror { object: a, mirror },
                    Command::Add(NewObject::new("B", cube(0.1)).mirrored(MirrorRepeater::new(2, Vec3::ZERO)).unselected()),
                ],
            )
        });
        assert_eq!((receipt.changed, receipt.failed), (true, 0));
        assert_eq!(bench.doc.undo_steps(), steps + 1);
        let info = bench.doc.info(a).unwrap();
        assert_eq!(info.mirror, mirror);
        let places = info.placements();
        assert_eq!(places.iter().map(|p| p.0).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(places[0].1, info.matrix());
        assert!(places[1].1.transform_point3(Vec3::ZERO).distance(Vec3::new(-2.0, 0.0, 0.0)) < 1e-6, "the copy is across the plane");
        assert!(places[1].1.determinant() < 0.0, "a reflection turns the mesh inside out");
        assert_eq!(bench.doc.info(receipt.added[0]).unwrap().mirror.map(|m| m.axis_mask), Some(2));

        // Saying it again changes nothing and is counted as failed.
        let receipt = bench.call(|cx| cx.commit("Mirror", vec![Command::Mirror { object: a, mirror }]));
        assert_eq!((receipt.changed, receipt.failed), (false, 1));

        assert!(bench.undo_scene());
        assert_eq!(bench.doc.info(a).unwrap().mirror, None);
        assert_eq!(bench.doc.objects(), vec![a], "the new object and the mirror went in the one step");
        assert!(bench.redo_scene());
        assert_eq!(bench.doc.info(a).unwrap().mirror, mirror);
        let off = bench.call(|cx| cx.commit("Unmirror", vec![Command::Mirror { object: a, mirror: None }]));
        assert!(off.changed && bench.doc.info(a).unwrap().mirror.is_none());
    }

    #[test]
    fn a_ray_is_cast_through_any_placement_and_the_scene_is_hit_through_the_mirror_copies() {
        let mut bench = Bench::new();
        let a = bench.add("A", cube(0.5));
        bench.call(|cx| cx.commit("Place", vec![Command::Place { object: a, placement: Placement::at(Vec3::new(1.5, 0.0, 0.0)) }]));
        bench.mirror(a, 1);
        let ray = bench.env.view.ray(bench.px(Vec3::new(-1.5, 0.0, 0.5)));

        // The scene's pick finds the copy; the object's own placement does not.
        let hit = bench.call(|cx| cx.pick(bench_px(cx, Vec3::new(-1.5, 0.0, 0.5)))).expect("the copy is under the pointer");
        assert_eq!(hit.object, a);
        assert!(hit.position.distance(Vec3::new(-1.5, 0.0, 0.5)) < 1e-4);
        assert!(bench.doc.raycast_object(a, &ray).is_none());

        // Through the copy's own matrix, the same ray meets it; through any other, where that puts the mesh.
        let places = bench.doc.info(a).unwrap().placements();
        let through = bench.doc.raycast_placed(a, &places[1].1, &ray).expect("the copy");
        assert!(through.position.distance(hit.position) < 1e-4);
        // The ray runs on past the copy; a cube dropped on it two units behind is met on its near face.
        let elsewhere = Mat4::from_translation(Vec3::new(-1.5 * 7.0 / 4.5, 0.0, -2.0));
        let behind = bench.doc.raycast_placed(a, &elsewhere, &ray).expect("a place behind it");
        assert!((behind.position.z + 1.5).abs() < 1e-4, "{:?}", behind.position);
        assert!(bench.doc.raycast_placed(a, &Mat4::from_translation(Vec3::new(9.0, 0.0, 0.0)), &ray).is_none());
        assert!(bench.doc.raycast_placed(ObjectId(999), &Mat4::IDENTITY, &ray).is_none());
    }

    fn bench_px(cx: &Ctx, world: Vec3) -> Vec2 {
        cx.view.project(world).unwrap()
    }

    #[test]
    fn requests_made_through_the_context_reach_the_host() {
        let mut bench = Bench::new();
        bench.call(|cx| {
            cx.status("Done.", true);
            cx.toast("Measure");
            cx.set_unit(sculpt_geom::LengthUnit::Inches);
            assert_eq!(cx.unit, sculpt_geom::LengthUnit::Inches, "the tool sees its own change at once");
            cx.set_mirror([true, false, false]);
            cx.frame_view(Aabb::new(Vec3::ZERO, Vec3::ONE));
            cx.finish();
        });
        assert_eq!(bench.status, Some(("Done.".to_owned(), true)));
        assert_eq!(bench.toasts, ["Measure"]);
        assert_eq!(bench.env.unit, sculpt_geom::LengthUnit::Inches);
        assert_eq!(bench.env.mirror, [true, false, false]);
        assert!(bench.finished && bench.framed.is_some());
    }
}
