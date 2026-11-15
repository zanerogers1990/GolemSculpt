//! Several objects and the transform gizmo.
//!
//! The objects themselves are `crate::scene`. This file is what the app does
//! with them: choosing the active one (whose mesh the brushes work on),
//! adding, copying, hiding and deleting them, each an undo step, the
//! Objects card, and the Transform tool.
//!
//! The Transform tool is the desktop's gizmo (`src/gizmo.rs`, compiled in):
//! arrows move, rings turn and cubes scale whatever objects are selected,
//! all together about their shared centre. A click on an object selects it
//! (with Shift it joins or leaves the selection). With part of the active
//! object masked, and it alone selected, a drag does not move the object: it
//! moves the unmasked part of its surface and leaves the masked part where
//! it is (the desktop's `app/masked.rs`, compiled in), so a limb can be
//! posed out of a body. Each drag is one undo step.

use eframe::egui::{self, Color32, Modifiers, PointerButton, Pos2, Stroke as Line, Ui};
use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};

use super::ui::tip;
use super::{Drag, GolemApp, HoverHit, Mode, Step};
use crate::gizmo::{self, Frame, Handles, Orientation, Placement, Target};
use crate::masked::{Deform, Mirror, Turns};
use crate::mesh::Mesh;
use crate::mesh::bvh::Aabb;
use crate::scene::{self, Object, Props};

/// The Transform tool's state.
#[derive(Default)]
pub struct Transform {
    /// Which axes the arrows and rings follow.
    pub orientation: Orientation,
    dragging: Option<Dragging>,
    /// The joint a masked drag turns about, kept while the mesh, its mask
    /// and the symmetry stay as they were (`masked::joint` reads the whole
    /// mesh).
    joint: Option<((u64, u64, Mirror), Option<Vec3>)>,
    ppp: f32,
}

struct Dragging {
    drag: gizmo::Drag,
    what: Dragged,
}

enum Dragged {
    /// Whole objects, in the order of the drag's targets, with what they
    /// were at the press for the undo step.
    Objects { ids: Vec<u64>, before: Vec<(u64, Props)> },
    /// The unmasked part of the active object's surface; `stale` once the
    /// pointer has moved since it was last reshaped (done once a frame).
    Surface { deform: Deform, stale: bool },
}

impl GolemApp {
    // ---- the objects ---------------------------------------------------------

    pub(crate) fn active_object(&self) -> &Object {
        self.objects.iter().find(|o| o.id == self.active).expect("the active object is in the list")
    }

    fn object_mut(&mut self, id: u64) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    /// Where the active object stands: its space into the world's.
    pub(crate) fn model(&self) -> Mat4 {
        self.active_object().matrix()
    }

    /// The view as the active object sees it: brushes, the tools and the
    /// cursor work in its own coordinates (`View::local_to`, as the desktop).
    pub(crate) fn local_view(&self) -> crate::camera::View {
        let model = self.model();
        if model == Mat4::IDENTITY { self.view } else { self.view.local_to(model) }
    }

    /// An object's mesh: its own, or the app's for the active one.
    pub(crate) fn mesh_of<'a>(&'a self, object: &'a Object) -> &'a Mesh {
        if object.id == self.active { &self.mesh } else { object.mesh.as_ref().unwrap_or(&self.mesh) }
    }

    /// The box round every object shown (round the active one when none is).
    pub(crate) fn scene_bounds(&self) -> Aabb {
        let mut out = Aabb { min: Vec3::INFINITY, max: Vec3::NEG_INFINITY };
        for o in self.objects.iter().filter(|o| o.visible) {
            let b = o.world_bounds(self.mesh_of(o));
            if !b.is_empty() {
                out.min = out.min.min(b.min);
                out.max = out.max.max(b.max);
            }
        }
        if out.min.x > out.max.x { self.active_object().world_bounds(&self.mesh) } else { out }
    }

    /// The active object's own point `local`, and its normal there, in the world.
    pub(crate) fn to_world(&self, local: Vec3, normal: Vec3) -> (Vec3, Vec3) {
        let m = self.model();
        let n = Mat3::from_mat4(m).inverse().transpose() * normal;
        (m.transform_point3(local), n.normalize_or(normal))
    }

    /// The active object under pixel `px`, as the brushes see it.
    pub(super) fn hit(&self, px: Vec2) -> Option<HoverHit> {
        if !self.active_object().visible {
            return None;
        }
        let view = self.local_view();
        let (o, d) = view.ray(px);
        let h = self.mesh.raycast(o, d)?;
        let (pos, normal) = self.to_world(h.pos, self.mesh.hit_normal(&h, false));
        Some(HoverHit { pos, normal, local: h.pos })
    }

    /// The nearest object shown under pixel `px`, and where on it (world).
    pub(crate) fn pick(&self, px: Vec2) -> Option<(u64, Vec3)> {
        let (origin, dir) = self.view.ray(px);
        let mut best: Option<(u64, Vec3, f32)> = None;
        for o in self.objects.iter().filter(|o| o.visible) {
            let m = o.matrix();
            let inv = m.inverse();
            let (lo, ld) = (inv.transform_point3(origin), inv.transform_vector3(dir));
            let len = ld.length();
            if len < 1e-12 {
                continue;
            }
            let Some(h) = self.mesh_of(o).raycast(lo, ld / len) else { continue };
            let pos = m.transform_point3(h.pos);
            let along = (pos - origin).dot(dir);
            if best.is_none_or(|(_, _, b)| along < b) {
                best = Some((o.id, pos, along));
            }
        }
        best.map(|(id, pos, _)| (id, pos))
    }

    /// Makes object `id` the one the brushes work on: its mesh becomes the
    /// app's, and the one that was gives its mesh back to its own object.
    /// It joins the selection, at its head. False if there is no such
    /// object.
    pub(crate) fn activate(&mut self, id: u64) -> bool {
        if id != self.active {
            let Some(mesh) = self.object_mut(id).and_then(|o| o.take_mesh()) else { return false };
            let mut old = std::mem::replace(&mut self.mesh, mesh);
            old.flush_normals();
            let was = self.active;
            if let Some(o) = self.object_mut(was) {
                o.set_aside(old);
            }
            self.active = id;
            self.upload_whole = true;
            // The last dab was on the other object's surface.
            self.sculptor.last_hit = None;
            self.transform.joint = None;
        }
        match self.selected.iter().position(|&s| s == id) {
            Some(i) => {
                self.selected.remove(i);
                self.selected.insert(0, id);
            }
            None => self.selected = vec![id],
        }
        true
    }

    /// Selects object `id`, alone, or (with `add`) joining the selection or
    /// leaving it if it was in it. The object picked is the active one.
    pub(crate) fn select(&mut self, id: u64, add: bool) {
        if self.sculptor.is_active() || self.drag == Drag::Gizmo {
            return;
        }
        if add {
            if let Some(i) = self.selected.iter().position(|&s| s == id) {
                if self.selected.len() > 1 {
                    self.selected.remove(i);
                    if id == self.active {
                        let next = self.selected[0];
                        self.activate(next);
                    }
                }
                return;
            }
            self.selected.insert(0, id);
            self.activate(id);
        } else {
            self.activate(id);
            self.selected = vec![id];
        }
        if let Some(o) = self.objects.iter().find(|o| o.id == id) {
            self.status = format!("{} selected: brushes and the Mesh tab work on it.", o.name);
        }
    }

    /// A new object holding `mesh`, placed by `props` (its name made unique),
    /// as one undo step; with `activate` it is the one worked on next.
    pub(crate) fn add_object(&mut self, mesh: Mesh, props: Props, activate: bool) -> u64 {
        let id = self.next_object;
        self.next_object += 1;
        let mut object = Object::new(id, String::new(), Some(mesh));
        let name = scene::unique_name(&self.objects, &props.name);
        object.set_props(Props { name, ..props });
        self.objects.push(object);
        self.push_undo(Step::Exists { index: self.objects.len() - 1, id, held: None });
        if activate {
            self.activate(id);
        }
        self.mark_changed();
        id
    }

    /// Takes object `id` out of the scene and hands it back. The last object
    /// stays: there is always one to sculpt.
    fn take_object(&mut self, id: u64) -> Option<(usize, Box<Object>)> {
        if self.objects.len() <= 1 {
            return None;
        }
        if id == self.active {
            let other = self.objects.iter().find(|o| o.id != id)?.id;
            self.activate(other);
        }
        let i = self.objects.iter().position(|o| o.id == id)?;
        let object = self.objects.remove(i);
        self.selected.retain(|&s| s != id);
        if self.selected.is_empty() {
            self.selected = vec![self.active];
        }
        Some((i, Box::new(object)))
    }

    /// Undo or redo of an object added or deleted: puts `held` back at
    /// `index`, or takes object `id` out. Returns what the step holds next.
    pub(super) fn toggle_exists(&mut self, index: usize, id: u64, held: Option<Box<Object>>) -> Option<Box<Object>> {
        match held {
            Some(object) => {
                let i = index.min(self.objects.len());
                self.objects.insert(i, *object);
                self.activate(id);
                None
            }
            None => self.take_object(id).map(|(_, object)| object),
        }
    }

    /// Undo or redo of a change of objects' places, names or shown state:
    /// sets each to what the step holds and returns what they were.
    pub(super) fn swap_props(&mut self, list: Vec<(u64, Props)>) -> Vec<(u64, Props)> {
        list.into_iter()
            .filter_map(|(id, props)| {
                let o = self.object_mut(id)?;
                let was = o.props();
                o.set_props(props);
                Some((id, was))
            })
            .collect()
    }

    /// Changes object `id`'s props with `f`, as one undo step.
    fn change_props(&mut self, id: u64, f: impl FnOnce(&mut Props)) {
        let Some(o) = self.object_mut(id) else { return };
        let before = o.props();
        let mut after = before.clone();
        f(&mut after);
        if after == before {
            return;
        }
        o.set_props(after);
        self.push_undo(Step::Props(vec![(id, before)]));
        self.mark_changed();
    }

    pub(crate) fn delete_object(&mut self, id: u64) {
        if self.sculptor.is_active() || self.drag != Drag::None {
            return;
        }
        let name = self.objects.iter().find(|o| o.id == id).map(|o| o.name.clone()).unwrap_or_default();
        match self.take_object(id) {
            Some((index, object)) => {
                self.push_undo(Step::Exists { index, id, held: Some(object) });
                self.mark_changed();
                self.status = format!("Deleted {name}. Ctrl+Z brings it back.");
            }
            None => self.status = "The last object stays: there is always one to sculpt.".into(),
        }
    }

    pub(crate) fn duplicate_active(&mut self) {
        if self.sculptor.is_active() || self.drag != Drag::None {
            return;
        }
        let m = &self.mesh;
        // With its levels, layers and hidden faces, as on the desktop.
        let copy = super::copy_of(m);
        let props = self.active_object().props();
        let name = props.name.clone();
        self.add_object(copy, props, true);
        self.status = format!("Copied {name}. The copy stands where it does: move it with the Transform tool (W).");
    }

    // ---- the gizmo -------------------------------------------------------------

    /// The symmetry a masked drag is repeated under: the brushes' own.
    fn masked_mirror(&self) -> Mirror {
        let s = &self.sculptor.settings;
        Mirror { axes: s.symmetry, world: None, radial: Some(Turns { radial: s.radial, world: None }) }
    }

    /// The joint a masked drag turns about, in the active object's space.
    fn masked_joint(&mut self) -> Option<Vec3> {
        let mirror = self.masked_mirror();
        let key = (self.mesh.uid, self.mesh.edits(), mirror);
        if self.transform.dragging.is_some() {
            return self.transform.joint.and_then(|(_, j)| j);
        }
        match self.transform.joint {
            Some((was, joint)) if was == key => joint,
            _ => {
                let joint = crate::masked::joint(&self.mesh, mirror);
                self.transform.joint = Some((key, joint));
                joint
            }
        }
    }

    /// What the gizmo sits on: the selected objects that are shown, the
    /// active one first, and each as the gizmo sees it.
    fn gizmo_targets(&mut self) -> (Vec<u64>, Vec<Target>) {
        let ids: Vec<u64> = self.selected.iter().copied().filter(|id| self.objects.iter().any(|o| o.id == *id && o.visible)).collect();
        let mut targets: Vec<Target> = ids
            .iter()
            .filter_map(|id| self.objects.iter().find(|o| o.id == *id))
            .map(|o| {
                let b = o.world_bounds(self.mesh_of(o));
                let radius = if b.is_empty() { 0.5 } else { (b.max - b.min).length() * 0.5 };
                Target::object(o, radius.max(1e-4))
            })
            .collect();
        // A lone masked object turns about the joint where the free part
        // meets the mask, so the gizmo sits there, not on its middle.
        if ids == [self.active] {
            if let Some(joint) = self.masked_joint() {
                targets[0].placement.position = self.model().transform_point3(joint);
            }
        }
        (ids, targets)
    }

    fn gizmo_frame(&mut self) -> Option<(Vec<u64>, Vec<Target>, Frame)> {
        if self.mode != Mode::Transform {
            return None;
        }
        let (ids, targets) = self.gizmo_targets();
        if targets.is_empty() {
            return None;
        }
        let ppp = self.transform.ppp.max(0.5);
        let frame = Frame::on(&targets, self.transform.orientation, Handles::ALL, &self.view, ppp)?;
        Some((ids, targets, frame))
    }

    /// A press on the viewport while the Transform tool is up. On a handle
    /// it starts a drag; on an object it selects it (Shift or Ctrl: joins or
    /// leaves the selection); either way off a handle the drag turns the view.
    pub(super) fn transform_press(&mut self, px: Vec2, button: PointerButton, mods: Modifiers) -> Drag {
        if button != PointerButton::Primary {
            return Drag::Orbit;
        }
        if let Some((ids, targets, frame)) = self.gizmo_frame() {
            if let Some(handle) = frame.pick(&self.view, px, self.transform.ppp.max(0.5)) {
                let drag = gizmo::Drag::on(handle, frame, &targets, &self.view, px);
                let mirror = self.masked_mirror();
                let alone = ids == [self.active];
                let what = match alone.then(|| Deform::begin(&mut self.mesh, mirror)).flatten() {
                    Some(deform) => Dragged::Surface { deform, stale: false },
                    None => {
                        let before = ids.iter().filter_map(|id| self.objects.iter().find(|o| o.id == *id)).map(|o| (o.id, o.props())).collect();
                        Dragged::Objects { ids, before }
                    }
                };
                self.transform.dragging = Some(Dragging { drag, what });
                return Drag::Gizmo;
            }
        }
        if let Some((id, _)) = self.pick(px) {
            self.select(id, mods.shift || mods.ctrl || mods.command);
        }
        Drag::Orbit
    }

    pub(super) fn transform_moved(&mut self, px: Vec2) {
        let Some(mut d) = self.transform.dragging.take() else { return };
        d.drag.update(&self.view, px);
        match &mut d.what {
            Dragged::Objects { ids, .. } => {
                for (id, placement) in ids.iter().zip(d.drag.placements()) {
                    if let Some(o) = self.objects.iter_mut().find(|o| o.id == *id) {
                        placement.apply(o);
                    }
                }
            }
            Dragged::Surface { stale, .. } => *stale = true,
        }
        self.transform.dragging = Some(d);
    }

    /// Reshapes the surface a masked drag is working on, once a frame,
    /// before the mesh is sent to be drawn.
    pub(super) fn transform_apply(&mut self) {
        let model = self.model();
        let Some(Dragging { drag, what: Dragged::Surface { deform, stale } }) = &mut self.transform.dragging else { return };
        if std::mem::take(stale) {
            deform.apply(&mut self.mesh, model, drag.world_delta());
        }
    }

    /// Ends a handle drag: one undo step if anything moved.
    pub(super) fn transform_released(&mut self) {
        self.transform_apply();
        let Some(d) = self.transform.dragging.take() else { return };
        let name = d.drag.handle.name();
        match d.what {
            Dragged::Objects { before, .. } => {
                // A click that picked a handle without moving it is not an edit.
                let moved = before.iter().zip(d.drag.placements()).any(|((_, was), now)| {
                    was.position != now.position || was.rotation != now.rotation || was.scale != now.scale
                });
                if moved {
                    self.push_undo(Step::Props(before));
                    self.mark_changed();
                    self.status = format!("{name}. Ctrl+Z puts it back.");
                }
            }
            Dragged::Surface { deform, .. } => {
                let how = deform.how();
                if let Some(delta) = deform.end(&mut self.mesh) {
                    self.push_undo(Step::Stroke(self.active, delta));
                    self.sculptor.last_hit = None;
                    self.mark_changed();
                    self.status = format!(
                        "Reshaped the unmasked part{how}; the masked part stayed where it was. Ctrl+Z puts it back."
                    );
                }
            }
        }
    }

    /// Esc during a drag of whole objects: they go back to where they were
    /// at the press, and no undo step is left. True if there was one.
    pub(super) fn transform_cancel(&mut self) -> bool {
        match self.transform.dragging.take() {
            Some(Dragging { what: Dragged::Objects { before, .. }, .. }) => {
                self.swap_props(before);
                true
            }
            Some(d) => {
                // A masked drag ends as it stands: the surface is mid-stroke.
                self.transform.dragging = Some(d);
                false
            }
            None => false,
        }
    }

    /// Draws the gizmo over the viewport while the Transform tool is up.
    pub(super) fn draw_transform(&mut self, painter: &egui::Painter, ppp: f32) {
        self.transform.ppp = ppp;
        if self.mode != Mode::Transform {
            return;
        }
        let (frame, hot) = match &self.transform.dragging {
            Some(d) => (d.drag.shown_frame(), Some(d.drag.handle)),
            None => {
                let Some((_, _, frame)) = self.gizmo_frame() else { return };
                let hot = self.pointer_px.filter(|_| self.drag == Drag::None).and_then(|px| frame.pick(&self.view, px, ppp));
                (frame, hot)
            }
        };
        let to_pt = |p: &Vec2| Pos2::new(p.x / ppp, p.y / ppp);
        let rgba = |c: [u8; 4]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        for shape in frame.shapes(&self.view, hot, ppp) {
            match shape {
                gizmo::Shape::Line { points, width, color } => {
                    let points: Vec<Pos2> = points.iter().map(to_pt).collect();
                    painter.add(egui::Shape::line(points, Line::new(width / ppp, rgba(color))));
                }
                gizmo::Shape::Fill { points, color } => {
                    let points: Vec<Pos2> = points.iter().map(to_pt).collect();
                    painter.add(egui::Shape::convex_polygon(points, rgba(color), Line::NONE));
                }
            }
        }
    }

    /// Puts the selected objects back where a new one stands: in the middle,
    /// upright, at their own size.
    fn reset_placement(&mut self) {
        let before: Vec<(u64, Props)> = self.selected.iter().filter_map(|id| self.objects.iter().find(|o| o.id == *id)).map(|o| (o.id, o.props())).collect();
        let mut changed = false;
        for (id, _) in &before {
            if let Some(o) = self.objects.iter_mut().find(|o| o.id == *id) {
                let p = Placement { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };
                changed |= Placement::of(o) != p;
                p.apply(o);
            }
        }
        if changed {
            self.push_undo(Step::Props(before));
            self.mark_changed();
            self.status = "Put back in the middle, upright, at its own size. Ctrl+Z undoes it.".into();
        }
    }

    // ---- the cards ---------------------------------------------------------------

    /// The Objects card: every object, the active one lit, with its eye and
    /// its bin; Copy and the Transform tool below.
    pub(super) fn objects_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let count = self.objects.len();
        let summary = if count == 1 { "1 object".to_owned() } else { format!("{count} objects") };
        let eye = self.ui.pictures.icon("eye");
        let eye_off = self.ui.pictures.icon("eye_off");
        let bin = self.ui.pictures.icon("trash");
        let mut chosen: Option<(u64, bool)> = None;
        let mut flip: Option<u64> = None;
        let mut delete: Option<u64> = None;
        theme::category(ui, "Objects", theme::lit(ACCENT()), &summary, true, |ui| {
            let shift = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl || i.modifiers.command);
            for o in &self.objects {
                ui.horizontal(|ui| {
                    let small = 28.0;
                    let eye_style = ButtonStyle { icon: if o.visible { eye } else { eye_off }, ..Default::default() };
                    if tip(theme::button(ui, small, "", eye_style), tips, if o.visible { "Hide it" } else { "Show it" }).clicked() {
                        flip = Some(o.id);
                    }
                    let w = (ui.available_width() - small - 6.0).max(40.0);
                    let lit = if o.id == self.active {
                        Some(ACCENT())
                    } else if self.selected.contains(&o.id) {
                        Some(theme::lit(ACCENT()).gamma_multiply(0.5))
                    } else {
                        None
                    };
                    let tris = self.mesh_of(o).live_triangle_count();
                    let label = format!("{}  ({})", o.name, theme::count(tris));
                    let style = ButtonStyle { active: lit, ..Default::default() };
                    let about = "Pick it to sculpt it. Shift-click to select several for the Transform tool.";
                    if tip(theme::button(ui, w, &label, style), tips, about).clicked() {
                        chosen = Some((o.id, shift));
                    }
                    let bin_style = ButtonStyle { icon: bin, ..Default::default() };
                    if tip(theme::button(ui, small, "", bin_style), tips, "Delete it (Ctrl+Z brings it back)").clicked() {
                        delete = Some(o.id);
                    }
                });
            }
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                if tip(theme::button(ui, w, "Copy", ButtonStyle::default()), tips, "A copy of the active object, as a new object").clicked() {
                    self.duplicate_active();
                }
                let style = ButtonStyle {
                    icon: self.ui.pictures.icon("tool_transform"),
                    active: (self.mode == Mode::Transform).then_some(ACCENT()),
                    badge: "W",
                    ..Default::default()
                };
                if tip(theme::button(ui, w, "Transform", style), tips, "Move, turn and scale the selected objects (W)").clicked() {
                    self.toggle_transform();
                }
            });
        });
        if let Some((id, add)) = chosen {
            self.select(id, add);
        }
        if let Some(id) = flip {
            self.change_props(id, |p| p.visible = !p.visible);
        }
        if let Some(id) = delete {
            self.delete_object(id);
        }
    }

    pub(crate) fn toggle_transform(&mut self) {
        if self.drag != Drag::None {
            return;
        }
        self.mode = if self.mode == Mode::Transform { Mode::Sculpt } else { Mode::Transform };
        if self.mode == Mode::Transform {
            self.status = "Transform: drag an arrow to move, a ring to turn, a cube to scale. Click an object to select it; \
                           with part of it masked, the drag poses the unmasked part. W or Esc: back to sculpting."
                .into();
        }
    }

    /// The Transform tool's panel: which axes the gizmo follows, and Reset.
    pub(super) fn transform_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let names: Vec<String> = self.selected.iter().filter_map(|id| self.objects.iter().find(|o| o.id == *id)).map(|o| o.name.clone()).collect();
        let summary = match names.len() {
            0 => "nothing".to_owned(),
            1 => names[0].clone(),
            n => format!("{n} objects"),
        };
        theme::category(ui, "Transform", Color32::from_rgb(255, 170, 70), &summary, true, |ui| {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 3, 4.0);
                for o in Orientation::ALL {
                    let style = ButtonStyle { active: (self.transform.orientation == o).then_some(ACCENT()), ..Default::default() };
                    if tip(theme::button(ui, w, o.name(), style), tips, o.tip()).clicked() {
                        self.transform.orientation = o;
                    }
                }
            });
            let full = ui.available_width();
            if tip(theme::button(ui, full, "Reset Placement", ButtonStyle::default()), tips, "Back to the middle, upright, at its own size").clicked() {
                self.reset_placement();
            }
            if let Some(o) = self.objects.iter().find(|o| o.id == self.active) {
                let deg = o.rotation.to_euler(glam::EulerRot::XYZ);
                theme::hint(
                    ui,
                    &format!(
                        "Place {:.2}, {:.2}, {:.2}   Turn {:.0}, {:.0}, {:.0}   Size {:.2}, {:.2}, {:.2}",
                        o.position.x,
                        o.position.y,
                        o.position.z,
                        deg.0.to_degrees(),
                        deg.1.to_degrees(),
                        deg.2.to_degrees(),
                        o.scale.x,
                        o.scale.y,
                        o.scale.z
                    ),
                );
            }
            theme::hint(
                ui,
                "Arrows move, rings turn, cubes scale; the middle square scales evenly. Click an object to select it, \
                 Shift-click to add more. With part of the object masked, a drag poses the unmasked part instead \
                 (mask the body, pull out a limb), mirrored with symmetry.",
            );
        });
    }
}
