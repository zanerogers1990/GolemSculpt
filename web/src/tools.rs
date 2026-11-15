//! The desktop's hosted tools in the web version: Tube, Muscle and Strap,
//! Lathe, Hand & Foot, Array, the region tools (Mask, Hide, Trim, Split and
//! Extract) and Measure. They are the desktop's own crates, hosted here the
//! way the desktop's `app/tools.rs` hosts them, through the tool kit
//! (`sculpt-toolkit`), so a tool fixed on the desktop is fixed here too.
//!
//! A tool sees one object, the active one, in its own space: the view it is
//! handed is the active object's (`GolemApp::local_view`), and its previews
//! are drawn where the object stands. What a tool adds as a new object
//! becomes a new object of the scene, standing where the tool put it: a
//! piece Split cuts off, an Extract shell, tubes kept separate. The Lathe
//! and Hand & Foot, which make a new base, can instead be joined into the
//! active object or take its place (a choice under their panels). Placing
//! the object (Measure's Set Size, moving an arrayed object) moves its
//! vertices, as the tool's one object stays where its own origin is.
//!
//! A tool's background work (a merge, a cut, a bake) runs there and then: a
//! browser page has no threads to put it on (crates/toolkit/src/jobs.rs).

use std::collections::HashMap;

use eframe::egui::{self, Color32, Modifiers, Pos2, Rect, Stroke as Line, StrokeKind, pos2, vec2};
use glam::{Mat4, Vec2, Vec3};
use sculpt_geom::LengthUnit;
use sculpt_toolkit::{
    self as kit, Ctx, Document, Draw, Env, Event, Hit, Info, Jobs, Key, MeshData, MeshId, MeshView, NewObject,
    ObjectId, ObjectInfo, Outbox, Placement, Preview, PreviewData, Ray, RadialChange, Scene, Shape, Style, Symmetry,
    Tool,
};

use crate::app::GolemApp;
use crate::mesh::{Mesh, StrokeDelta, VisibilityDelta};
use crate::sculpt::{Radial, RadialAxis};

/// The web scene's one object.
const MODEL: ObjectId = ObjectId(1);
/// How far a press may wander before it is a drag rather than a click.
const CLICK_SLOP: f32 = 4.0;
/// The colour an array's copies of the model are drawn in: lit clay, as the
/// Lathe's preview.
const COPY_CLAY: Color32 = Color32::from_rgb(184, 184, 189);
/// The tools that make a new base, which may take the model's place rather
/// than be joined into it.
const MAKERS: [&str; 2] = ["lathe", "hands"];

/// Every hosted tool. Tube, Muscle and Strap stay first, at 0, 1 and 2; the
/// Tools tab finds the rest by id.
fn registry() -> Vec<Box<dyn Tool>> {
    vec![
        sculpt_tube::tool(),
        sculpt_tube::muscle(),
        sculpt_tube::strap(),
        sculpt_lathe::tool(),
        sculpt_hands::tool(),
        sculpt_array::tool(),
        sculpt_region::extract(),
        sculpt_region::mask(),
        sculpt_region::hide(),
        sculpt_region::trim(),
        sculpt_region::split(),
        sculpt_measure::tool(),
    ]
}

struct Watch {
    button: kit::Button,
    at: Vec2,
    moved: bool,
}

/// The hosted tools and what the web app keeps for them.
pub struct Tools {
    tools: Vec<Box<dyn Tool>>,
    infos: Vec<Info>,
    /// The tool picked on the panel.
    pub which: usize,
    /// The tool that has been told it is up (`Tool::enter`).
    entered: Option<usize>,
    jobs: Jobs,
    /// The tool took the press: its drag and release are its own.
    captured: bool,
    watch: Option<Watch>,
    /// What the tools asked to have drawn this frame.
    pub draw: Draw,
    /// The places the model is shown besides its own (an array's copies), as
    /// the tools said last frame: the region tools cut through them.
    places: Vec<Mat4>,
    /// The model as a preview mesh, for drawing an array's copies of it, and
    /// which state of the model it was made from.
    copies: Preview,
    copies_of: Option<(u64, u64, u64, u64)>,
    /// The unit lengths are shown and typed in (Measure).
    unit: LengthUnit,
    /// What becomes of what the Lathe or Hand & Foot makes.
    made: Made,
    /// The interface's icons, handed over by the Tools tab, for the tools' panels.
    pub(crate) icons: HashMap<&'static str, egui::TextureId>,
    /// Whether tooltips are on.
    pub(crate) tips: bool,
    /// The hidden faces the graphics card was last sent (`Mesh::visibility_version`).
    pub(crate) sent_visibility: u64,
    // The pointer as the tool is told of it.
    pub(crate) pointer: Vec2,
    pub(crate) pressure: Option<f32>,
    pub(crate) mods: kit::Mods,
    pub(crate) hovering: bool,
    pub(crate) time: f64,
    pub(crate) dt: f32,
    pub(crate) ppp: f32,
}

impl Default for Tools {
    fn default() -> Tools {
        let tools = registry();
        Tools {
            infos: tools.iter().map(|t| t.info()).collect(),
            tools,
            which: 0,
            entered: None,
            jobs: Jobs::plain(),
            captured: false,
            watch: None,
            draw: Draw::default(),
            places: Vec::new(),
            copies: Preview::new(),
            copies_of: None,
            unit: LengthUnit::Millimetres,
            made: Made::NewObject,
            icons: HashMap::new(),
            tips: true,
            sent_visibility: 0,
            pointer: Vec2::ZERO,
            pressure: None,
            mods: kit::Mods::default(),
            hovering: false,
            time: 0.0,
            dt: 1.0 / 60.0,
            ppp: 1.0,
        }
    }
}

impl Tools {
    /// What a tool calls itself: its name, icon, tooltip and key.
    pub fn info(&self, index: usize) -> Option<&Info> {
        self.infos.get(index)
    }

    /// The tool with this id (`Info::id`).
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.infos.iter().position(|info| info.id == id)
    }

    /// The name of the tool picked.
    pub fn name(&self) -> &'static str {
        self.infos.get(self.which).map_or("", |info| info.name)
    }

    /// Each tool's key (the desktop's), and the tool it brings up.
    pub fn hotkeys(&self) -> Vec<(Key, usize)> {
        self.infos.iter().enumerate().filter_map(|(i, info)| Some((info.hotkey?, i))).collect()
    }

    /// The key of the tool picked, which it may take for itself while it is
    /// up (the region tools step through their shapes with it).
    pub fn hotkey(&self) -> Option<Key> {
        self.infos.get(self.which).and_then(|info| info.hotkey)
    }
}

pub fn mods_of(m: Modifiers) -> kit::Mods {
    kit::Mods { shift: m.shift, ctrl: m.ctrl || m.command, alt: m.alt }
}

/// The digit keys, with which the region tools go straight back to sculpting.
pub fn is_digit(key: Key) -> bool {
    use Key::*;
    matches!(key, Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9)
}

// ---- the scene a tool sees ----------------------------------------------------

/// What becomes of what a tool that makes a new base makes.
#[derive(Clone, Copy, PartialEq)]
enum Made {
    /// A new object of the scene.
    NewObject,
    /// Joined into the active object, beside what is there.
    Join,
    /// In the active object's place, in the middle of the scene.
    Replace,
}

/// A new object a tool made, for the app to add once the tool is done:
/// its name, its mesh (in its own space), where it stands in the active
/// object's space, and whether it is to be the one worked on next.
struct Made1 {
    name: String,
    mesh: Mesh,
    place: Mat4,
    select: bool,
}

struct WebDoc<'a> {
    mesh: &'a mut Mesh,
    /// What is added is joined into the model rather than made a new object.
    join: bool,
    /// New objects made, added to the scene once the tool is done.
    made: Vec<Made1>,
    symmetry: Symmetry,
    places: &'a [Mat4],
    /// The model as it was before this commit changed it: one undo step.
    before: Option<Mesh>,
    /// Masks and hidden faces set, each an undo step of its own.
    masks: Vec<StrokeDelta>,
    faces: Vec<VisibilityDelta>,
    /// The first object a commit adds takes the model's place rather than
    /// being joined into it.
    replace_on_add: bool,
    /// What this commit has added so far in the model's place, as placed in
    /// the world, before it is set in the middle of the scene.
    fresh: Option<(Vec<Vec3>, Vec<u32>)>,
    /// Something was added to the model (joined) or put in its place.
    joined: bool,
    replaced: bool,
    changed: bool,
}

impl WebDoc<'_> {
    fn hit(&self, ray: &Ray) -> Option<Hit> {
        let h = self.mesh.raycast(ray.origin, ray.dir)?;
        Some(Hit {
            object: MODEL,
            position: h.pos,
            normal: self.mesh.hit_normal(&h, false),
            distance: (h.pos - ray.origin).dot(ray.dir),
        })
    }

    fn swap_in(&mut self, mesh: Mesh) {
        let old = std::mem::replace(self.mesh, mesh);
        // The first change in a commit is what undo goes back to.
        self.before.get_or_insert(old);
        self.changed = true;
    }
}

/// Appends `tri` placed by each of `places` to `positions` and `indices`,
/// with the winding turned where a place mirrors.
fn append_placed(positions: &mut Vec<Vec3>, indices: &mut Vec<u32>, tri: &sculpt_geom::TriMesh, places: &[Mat4]) {
    for place in places {
        let base = positions.len() as u32;
        positions.extend(tri.positions.iter().map(|&p| place.transform_point3(p)));
        let flips = place.determinant() < 0.0;
        for t in tri.indices.chunks_exact(3) {
            let [a, b, c] = [t[0] + base, t[1] + base, t[2] + base];
            indices.extend_from_slice(&if flips { [a, c, b] } else { [a, b, c] });
        }
    }
}

impl Scene for WebDoc<'_> {
    fn objects(&self) -> Vec<ObjectId> {
        vec![MODEL]
    }

    fn info(&self, id: ObjectId) -> Option<ObjectInfo<'_>> {
        (id == MODEL).then(|| ObjectInfo {
            id,
            name: "Model",
            visible: true,
            active: true,
            mesh: MeshId(self.mesh.uid),
            placement: Placement::IDENTITY,
            mirror: None,
            live_copies: self.places,
            vertices: self.mesh.vertex_count(),
            triangles: self.mesh.triangle_count(),
        })
    }

    fn active(&self) -> ObjectId {
        MODEL
    }

    fn mesh(&self, id: ObjectId) -> Option<MeshView<'_>> {
        (id == MODEL).then(|| MeshView {
            id: MeshId(self.mesh.uid),
            positions: &self.mesh.positions,
            normals: &self.mesh.normals,
            indices: &self.mesh.indices,
            mask: &self.mesh.mask,
            hidden_triangles: self.mesh.hidden_triangles(),
            hidden_vertices: self.mesh.hidden_vertices(),
        })
    }

    fn raycast(&self, ray: &Ray) -> Option<Hit> {
        self.hit(ray)
    }

    fn raycast_object(&self, id: ObjectId, ray: &Ray) -> Option<Hit> {
        if id == MODEL { self.hit(ray) } else { None }
    }

    fn raycast_placed(&self, id: ObjectId, model: &Mat4, ray: &Ray) -> Option<Hit> {
        if id != MODEL {
            return None;
        }
        let local = ray.transformed(&model.inverse());
        let h = self.mesh.raycast(local.origin, local.dir)?;
        let position = model.transform_point3(h.pos);
        let normal = (glam::Mat3::from_mat4(model.inverse()).transpose() * self.mesh.hit_normal(&h, false))
            .normalize_or(Vec3::Y);
        Some(Hit { object: MODEL, position, normal, distance: (position - ray.origin).dot(ray.dir) })
    }

    fn bounds(&self, id: ObjectId) -> sculpt_geom::Aabb {
        let b = self.mesh.bounds();
        if id != MODEL || b.is_empty() { sculpt_geom::Aabb::EMPTY } else { sculpt_geom::Aabb::new(b.min, b.max) }
    }

    fn symmetry(&self, _id: ObjectId) -> Symmetry {
        self.symmetry
    }
}

/// A tool's mesh as plain triangles and a mask.
fn plain(data: MeshData) -> Option<(sculpt_geom::TriMesh, Option<Vec<f32>>)> {
    match data {
        MeshData::Plain { mesh, mask } => Some((mesh, mask)),
        // Only the desktop bakes meshes ahead (`Jobs::plain` here never does).
        MeshData::Baked(_) => None,
    }
}

impl Document for WebDoc<'_> {
    fn begin(&mut self, _label: &str) {
        self.fresh = None;
    }

    fn add(&mut self, object: NewObject) -> Option<ObjectId> {
        let (tri, _) = plain(object.mesh)?;
        let model = object.placement.matrix();
        // Its live mirror copies, made real: the web scene keeps no copies.
        let mut places = vec![model];
        if let Some(mirror) = object.mirror {
            places.extend(mirror.copies().into_iter().map(|copy| copy.matrix(&model, mirror.center)));
        }
        if self.replace_on_add || self.fresh.is_some() {
            // In the model's place: what this commit makes, all of it, set
            // in the middle of the scene, so a mirror or a turn round the
            // axis works on it from the start (a pair of hands, a lathe).
            let (mut positions, mut indices) = self.fresh.take().unwrap_or_default();
            append_placed(&mut positions, &mut indices, &tri, &places);
            let (lo, hi) = positions.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &p| (lo.min(p), hi.max(p)));
            let centre = if lo.x <= hi.x { (lo + hi) * 0.5 } else { Vec3::ZERO };
            let centred: Vec<Vec3> = positions.iter().map(|&p| p - centre).collect();
            self.swap_in(Mesh::new(centred, indices.clone(), None));
            self.fresh = Some((positions, indices));
            self.replace_on_add = false;
            self.replaced = true;
            return Some(MODEL);
        }
        if !self.join {
            // A new object of the scene, standing where the tool put it; its
            // mirror copies are made real in its own space.
            let inverse = model.inverse();
            let own: Vec<Mat4> = places.iter().map(|p| inverse * *p).collect();
            let (mut positions, mut indices) = (Vec::new(), Vec::new());
            append_placed(&mut positions, &mut indices, &tri, &own);
            if indices.is_empty() {
                return None;
            }
            let name = if object.name.is_empty() { "Object".to_owned() } else { object.name };
            self.made.push(Made1 { name, mesh: Mesh::new(positions, indices, None), place: model, select: object.select });
            self.changed = true;
            return Some(ObjectId(1_000_000 + self.made.len() as u64));
        }
        let mut positions = self.mesh.positions.clone();
        let mut indices = self.mesh.indices.clone();
        let mut mask = self.mesh.mask.clone();
        append_placed(&mut positions, &mut indices, &tri, &places);
        mask.resize(positions.len(), 0.0);
        self.swap_in(Mesh::new(positions, indices, Some(mask)));
        self.joined = true;
        Some(MODEL)
    }

    fn replace(&mut self, object: ObjectId, mesh: MeshData) -> bool {
        if object != MODEL {
            return false;
        }
        let Some((tri, mask)) = plain(mesh) else { return false };
        if tri.indices.is_empty() {
            return false;
        }
        self.swap_in(Mesh::new(tri.positions, tri.indices, mask));
        true
    }

    /// The model always stands at the scene's origin, so a new placement is
    /// carried out on its vertices.
    fn place(&mut self, object: ObjectId, placement: Placement) -> bool {
        if object != MODEL || placement == Placement::IDENTITY {
            return false;
        }
        let m = placement.matrix();
        if !m.is_finite() || m.determinant().abs() < 1e-12 {
            return false;
        }
        let positions = self.mesh.positions.iter().map(|&p| m.transform_point3(p)).collect();
        let mut indices = self.mesh.indices.clone();
        if m.determinant() < 0.0 {
            for t in indices.chunks_exact_mut(3) {
                t.swap(1, 2);
            }
        }
        let mask = self.mesh.mask.clone();
        self.swap_in(Mesh::new(positions, indices, Some(mask)));
        true
    }

    fn show(&mut self, _object: ObjectId, _visible: bool) -> bool {
        false
    }

    fn rename(&mut self, _object: ObjectId, _name: String) -> bool {
        false
    }

    fn delete(&mut self, _object: ObjectId) -> bool {
        false
    }

    fn select(&mut self, object: ObjectId) -> bool {
        object == MODEL
    }

    fn end(&mut self) -> bool {
        self.fresh = None;
        self.changed
    }

    fn mask(&mut self, object: ObjectId, mask: Vec<f32>) -> bool {
        if object != MODEL || mask.len() != self.mesh.vertex_count() {
            return false;
        }
        let Some(delta) = self.mesh.remap_mask(|_, v, _| mask[v as usize]) else { return false };
        self.masks.push(delta);
        self.changed = true;
        true
    }

    fn hide_faces(&mut self, object: ObjectId, triangles: &[u32], hide: bool) -> bool {
        if object != MODEL {
            return false;
        }
        let Some(delta) = self.mesh.set_triangles_hidden(triangles, hide) else { return false };
        // A model with nothing showing looks deleted, not hidden: refused.
        if self.mesh.visible_triangle_count() == 0 {
            self.mesh.apply_visibility(&delta, true);
            return false;
        }
        self.faces.push(delta);
        self.changed = true;
        true
    }

    fn show_all_faces(&mut self, object: ObjectId) -> bool {
        if object != MODEL {
            return false;
        }
        let Some(delta) = self.mesh.show_all() else { return false };
        self.faces.push(delta);
        self.changed = true;
        true
    }

    fn invert_faces(&mut self, object: ObjectId) -> bool {
        if object != MODEL {
            return false;
        }
        let Some(delta) = self.mesh.invert_visible() else { return false };
        self.faces.push(delta);
        self.changed = true;
        true
    }
}

/// A tool's request for radial symmetry, taken into the brush settings (the
/// desktop's `change_radial`). A direction along X, Y or Z is that axis; any
/// other is the custom one.
fn change_radial(radial: &mut Radial, change: RadialChange) {
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

// ---- hosting -----------------------------------------------------------------

impl GolemApp {
    fn tool_symmetry(&self) -> Symmetry {
        let s = &self.sculptor.settings;
        Symmetry {
            mirror: s.symmetry,
            radial_count: s.radial.repeats().clamp(1, Radial::MAX_COUNT),
            radial_axis: s.radial.axis_vector(),
            ..Symmetry::NONE
        }
    }

    fn tool_env(&self, active: bool) -> Env {
        // The active object's own view: the tool works in its space.
        let local = self.local_view();
        let v = &local;
        let t = &self.tubes;
        let mut env = Env::at_rest(kit::View {
            eye: v.pos,
            right: v.right,
            up: v.up,
            forward: v.forward,
            view_proj: v.view_proj,
            size: v.size,
            tan_half_fov: v.tan_half_fov,
            ortho_half_height: v.ortho_half_height,
            pixels_per_point: t.ppp,
        });
        env.pointer = t.pointer - v.origin;
        env.hovering = t.hovering;
        env.pressure = t.pressure;
        env.mods = t.mods;
        env.time = t.time;
        env.dt = t.dt;
        env.active = active;
        let symmetry = self.tool_symmetry();
        env.mirror = symmetry.mirror;
        env.radial_count = symmetry.radial_count;
        env.radial_axis = symmetry.radial_axis;
        env.unit = t.unit;
        env.tips = t.tips;
        env
    }

    /// Runs `f` with tool `index` and a `Ctx` on the web scene, then carries
    /// out what the tool asked for. `active` says whether it is the one up.
    fn tool_call<R>(&mut self, index: usize, active: bool, f: impl FnOnce(&mut dyn Tool, &mut Ctx) -> R) -> R {
        let env = self.tool_env(active);
        let symmetry = self.tool_symmetry();
        let mut out = Outbox::default();
        let Tools { tools, infos, jobs, places, icons, made, .. } = &mut self.tubes;
        let maker = MAKERS.contains(&infos[index].id);
        let mut doc = WebDoc {
            mesh: &mut self.mesh,
            join: maker && *made == Made::Join,
            made: Vec::new(),
            symmetry,
            places,
            before: None,
            masks: Vec::new(),
            faces: Vec::new(),
            replace_on_add: maker && *made == Made::Replace,
            fresh: None,
            joined: false,
            replaced: false,
            changed: false,
        };
        let icon_of = |name: &str| icons.get(name).copied();
        let result = {
            let mut cx = Ctx::new(env, &mut doc, jobs, &icon_of, &mut out);
            f(tools[index].as_mut(), &mut cx)
        };
        let WebDoc { before, masks, faces, joined, replaced, made, .. } = doc;
        if let Some(before) = before {
            self.mesh_replaced(before);
        }
        // New objects, standing where the tool put them in the active
        // object's space. A new base (the Lathe's, a pair of hands) is the
        // one worked on next; a piece or a tube is not, so the tool goes on
        // working on the object it was drawing on.
        let model = self.model();
        let mut first_made = None;
        for m in made {
            let (scale, rotation, position) = (model * m.place).to_scale_rotation_translation();
            let props = crate::scene::Props { name: m.name, visible: true, position, rotation, scale };
            let id = self.add_object(m.mesh, props, false);
            if m.select && maker {
                first_made.get_or_insert(id);
            }
        }
        for delta in masks {
            self.stroke_done(delta);
        }
        for delta in faces {
            self.faces_changed(delta);
        }
        if let Some((text, _)) = out.status.take().or(out.toast.take().map(|t| (t, true))) {
            self.status = text;
        }
        if let Some(unit) = out.unit {
            self.tubes.unit = unit;
        }
        if let Some(mirror) = out.mirror {
            self.sculptor.settings.symmetry = mirror;
        }
        if let Some(change) = out.radial {
            if joined && !replaced {
                // A lathe joined beside the model is off the model's axis, so
                // a turn round that axis would not follow it.
                self.status = "Made. Radial symmetry was left off: the new part was joined beside the model, off its \
                               axis. Choose Replace the Model to sculpt it with radial symmetry."
                    .into();
            } else {
                change_radial(&mut self.sculptor.settings.radial, change);
            }
        }
        if let Some(id) = first_made {
            self.activate(id);
        }
        let aspect = (self.view.size.x / self.view.size.y.max(1.0)).max(0.1);
        // What to frame, in the world (a tool names it in the object's space).
        let frame = if replaced || first_made.is_some() {
            Some(self.active_object().world_bounds(&self.mesh))
        } else {
            out.frame.map(|b| {
                let m = self.model();
                let corners = (0..8).map(|i| {
                    let pick = |bit: usize, lo: f32, hi: f32| if i & bit == 0 { lo } else { hi };
                    m.transform_point3(Vec3::new(pick(1, b.min.x, b.max.x), pick(2, b.min.y, b.max.y), pick(4, b.min.z, b.max.z)))
                });
                let (min, max) = corners.fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), p| (lo.min(p), hi.max(p)));
                crate::mesh::bvh::Aabb { min, max }
            })
        };
        if let Some(b) = frame.filter(|b| !b.is_empty()) {
            self.camera.frame(b, aspect, true);
        }
        if out.changed {
            self.note_changed();
        }
        // The tool's Back to Sculpt, from the tool that is up only.
        if out.finish && active {
            self.mode = crate::app::Mode::Sculpt;
        }
        if out.start {
            self.tubes.which = index;
            self.mode = crate::app::Mode::Tubes;
        }
        result
    }

    fn tool_event(&mut self, event: Event) -> bool {
        let index = self.tubes.which;
        self.tool_call(index, true, |tool, cx| tool.input(cx, event))
    }

    /// Once a frame: the tool that is up (and only it) is told so when it
    /// comes up, then every tool gets its frame (the one up with `active`
    /// set) and says what to draw, so a tool put away can still land its
    /// work and keep its copies on screen, as on the desktop.
    pub(crate) fn tubes_frame(&mut self, active: bool) {
        let want = active.then_some(self.tubes.which);
        if self.tubes.entered != want {
            if let Some(old) = self.tubes.entered.take() {
                self.tool_call(old, true, |tool, cx| tool.leave(cx));
                self.tubes.captured = false;
                self.tubes.watch = None;
            }
            if let Some(new) = want {
                self.tool_call(new, true, |tool, cx| tool.enter(cx));
                self.tubes.entered = want;
            }
        }
        // During a brush stroke the tools wait, and what they drew last stays up.
        if self.sculptor.is_active() {
            return;
        }
        let mut draw = Draw::default();
        for index in 0..self.tubes.tools.len() {
            let up = self.tubes.entered == Some(index);
            self.tool_call(index, up, |tool, cx| tool.frame(cx, &mut draw));
        }
        self.tubes.places =
            draw.object_places().iter().filter(|(object, _)| *object == MODEL).flat_map(|(_, m)| m.iter().copied()).collect();
        self.draw_model_copies(&mut draw);
        self.tubes.draw = draw;
    }

    /// The copies of the model a tool keeps (an array's), drawn as the model
    /// again: the renderer draws one model, so they go as a preview of it.
    /// The preview is made again only once the model has changed.
    fn draw_model_copies(&mut self, draw: &mut Draw) {
        let places: Vec<Mat4> =
            draw.object_copies().iter().filter(|(object, _)| *object == MODEL).flat_map(|(_, m)| m.iter().copied()).collect();
        let t = &mut self.tubes;
        if places.is_empty() {
            t.copies.clear();
            t.copies_of = None;
            return;
        }
        let m = &self.mesh;
        let state = (m.uid, m.edits(), m.topology_version(), m.visibility_version());
        if t.copies_of != Some(state) {
            t.copies_of = Some(state);
            t.copies.set_parts(PreviewData {
                positions: m.positions.clone(),
                normals: m.normals.clone(),
                indices: m.visible_indices().into_owned(),
            });
        }
        draw.instances(&t.copies, Style::solid(COPY_CLAY), &places);
    }

    /// The panel of the tool that is up, its card (Measure's object size)
    /// and what the web version does differently with it.
    pub(crate) fn tubes_panel(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.tubes.entered.filter(|&i| i == self.tubes.which) else { return };
        self.tool_call(index, true, |tool, cx| tool.panel(ui, cx));
        self.tool_call(index, true, |tool, cx| tool.card(ui, cx));
        self.web_notes(ui, index);
    }

    /// Where the web version's one model changes what a tool does, it says so.
    fn web_notes(&mut self, ui: &mut egui::Ui, index: usize) {
        use kit::theme::{self, ACCENT, ButtonStyle};
        let id = self.tubes.infos[index].id;
        if MAKERS.contains(&id) {
            theme::category(ui, "What Is Made", Color32::from_rgb(110, 200, 120), "", true, |ui| {
                ui.horizontal(|ui| {
                    let w = theme::row_width(ui, 3, 4.0);
                    let choices = [
                        (Made::NewObject, "New Object", "What is made is a new object, standing where it is shown."),
                        (Made::Join, "Join", "What is made is joined into the active object, beside what is there."),
                        (
                            Made::Replace,
                            "Replace",
                            "What is made takes the active object's place, in the middle of the scene. Ctrl+Z brings \
                             the old one back.",
                        ),
                    ];
                    for (choice, name, about) in choices {
                        let style = ButtonStyle { active: (self.tubes.made == choice).then_some(ACCENT()), ..Default::default() };
                        let response = theme::button(ui, w, name, style);
                        let response = if self.tubes.tips { response.on_hover_text(about) } else { response };
                        if response.clicked() {
                            self.tubes.made = choice;
                        }
                    }
                });
            });
            return;
        }
        let note = match id {
            "array" => {
                "Brushes work on the object itself, not through its copies, and a live array is not kept in the \
                 browser when the page is closed: make it real to keep it."
            }
            _ => return,
        };
        theme::hint(ui, note);
    }

    /// A button went down on the viewport. True when the tool took it.
    pub(crate) fn tool_press(&mut self, px: Vec2, secondary: bool) -> bool {
        let button = if secondary { kit::Button::Right } else { kit::Button::Left };
        self.tubes.pointer = px;
        if self.tool_event(Event::Press(button)) {
            self.tubes.captured = true;
            return true;
        }
        // Not the tool's: the camera turns, unless it turns out to be a click.
        self.tubes.watch = Some(Watch { button, at: px, moved: false });
        false
    }

    /// The pointer moved. True when the move was the tool's (or a press that
    /// may yet be a click), so the camera keeps still.
    pub(crate) fn tool_moved(&mut self, px: Vec2) -> bool {
        self.tubes.pointer = px;
        if self.tubes.captured {
            self.tool_event(Event::Drag);
            return true;
        }
        let slop = CLICK_SLOP * self.tubes.ppp;
        if let Some(watch) = &mut self.tubes.watch {
            if !watch.moved {
                if px.distance(watch.at) <= slop {
                    return true;
                }
                watch.moved = true;
            }
        }
        false
    }

    pub(crate) fn tool_released(&mut self) {
        let captured = std::mem::take(&mut self.tubes.captured);
        let watch = self.tubes.watch.take();
        if captured {
            self.tool_event(Event::Release);
        } else if let Some(watch) = watch.filter(|w| !w.moved) {
            self.tool_event(Event::Click(watch.button));
        }
    }

    pub(crate) fn tool_wheel(&mut self, lines: f32) -> bool {
        self.tool_event(Event::Wheel(lines))
    }

    pub(crate) fn tool_key(&mut self, key: egui::Key, down: bool) -> bool {
        self.tool_event(if down { Event::KeyDown(key) } else { Event::KeyUp(key) })
    }

    pub(crate) fn tool_undo(&mut self, redo: bool) -> bool {
        self.tool_event(if redo { Event::Redo } else { Event::Undo })
    }

    /// Whether the tool holds the drag going on.
    pub(crate) fn tool_holds_drag(&self) -> bool {
        self.tubes.captured
    }
}

// ---- what the tool draws over the viewport --------------------------------------

/// The tool's overlay: positions arrive in the viewport's pixels, sizes in points.
pub fn paint_overlay(painter: &egui::Painter, draw: &Draw, origin: Vec2, ppp: f32) {
    let at = |p: &Vec2| pos2((p.x + origin.x) / ppp, (p.y + origin.y) / ppp);
    for shape in &draw.overlay.shapes {
        match shape {
            Shape::Line { points, width, color, closed, dash } => {
                let mut path: Vec<Pos2> = points.iter().map(at).collect();
                let stroke = Line::new(*width, *color);
                match dash {
                    Some((dash, gap)) => {
                        if *closed {
                            path.extend(path.first().copied());
                        }
                        painter.extend(egui::Shape::dashed_line(&path, stroke, *dash, *gap));
                    }
                    None if *closed => {
                        painter.add(egui::Shape::closed_line(path, stroke));
                    }
                    None => {
                        painter.line(path, stroke);
                    }
                }
            }
            Shape::Dot { center, radius, fill, outline, square } => {
                let c = at(center);
                if *square {
                    let rect = Rect::from_center_size(c, egui::Vec2::splat(radius * 2.0));
                    painter.rect_filled(rect, 1.0, *fill);
                    painter.rect_stroke(rect, 1.0, Line::new(1.5, *outline), StrokeKind::Outside);
                } else {
                    painter.circle_filled(c, radius + 1.5, *outline);
                    painter.circle_filled(c, *radius, *fill);
                }
            }
            Shape::Fill { points, color } => {
                painter.add(egui::Shape::convex_polygon(points.iter().map(at).collect(), *color, Line::NONE));
            }
            Shape::Label(label) => {
                let mut job = egui::text::LayoutJob { halign: egui::Align::Center, ..Default::default() };
                for run in &label.runs {
                    let font = if run.bold { kit::theme::bold(run.size) } else { kit::theme::font(run.size) };
                    job.append(&run.text, 0.0, egui::TextFormat::simple(font, run.color));
                }
                let galley = painter.layout_job(job);
                let size = galley.size() + vec2(12.0, 6.0);
                let push = label.push.normalize_or_zero();
                let reach =
                    if push == Vec2::ZERO { 0.0 } else { push.x.abs() * size.x * 0.5 + push.y.abs() * size.y * 0.5 + label.gap };
                let rect = Rect::from_center_size(at(&label.at) + vec2(push.x, push.y) * reach, size);
                painter.rect_filled(rect, 4.0, label.background);
                painter.galley(pos2(rect.center().x, rect.top() + 3.0), galley, Color32::WHITE);
            }
        }
    }
}
