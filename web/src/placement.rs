//! Where an object's centre is and how it is scaled, on the web: Apply Scale
//! and Set Origin to Mesh in the Objects card, and the Pivot rows of the
//! Transform card (Edit Pivot, Pivot to Centre, Align to World), as on the
//! desktop (`scene/apply_scale.rs`, `scene/origin.rs`, `app/apply_scale.rs`,
//! `app/origin.rs`, `app/pivot.rs`), with the same words.
//!
//! Each leaves the object where it is in the world: the mesh's points move
//! one way in its own space and the object's place, turn or size the other.
//! The result is a new mesh in the object's place, and the undo step holds the
//! old mesh and the old place together, so one Undo puts both back exactly.
//! Live mirror copies are drawn through the object's placement, so they stay
//! as they look. The web version has no instances, arrays on an object or
//! SSphere rigs, so nothing of those is said or refused.

use eframe::egui::Ui;
use glam::{DVec3, Mat3, Quat, Vec2, Vec3};

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use super::{GolemApp, Mode, Step};
use crate::gizmo::{Handles, Placement, Target};
use crate::mesh::{Mesh, Polys};

/// A Scale whose longest side is more than this fraction longer than its
/// shortest counts as uneven.
const UNEVEN: f32 = 1e-4;
/// A Scale this small on any axis has flattened the object.
const FLATTENED: f32 = 1e-6;
/// How uneven a Scale has to be for Remesh to even it first.
const NOTICEABLE: f32 = 1.05;
/// A centre this close to the middle of its mesh, as a fraction of the
/// mesh's longest side, is on it.
const ALREADY_THERE: f64 = 1e-6;
/// A turn of the pivot smaller than this, in radians, is none.
const LEAST_TURN: f32 = 1e-5;
/// A move of the pivot shorter than this fraction of the mesh's size is none.
const LEAST_MOVE: f32 = 1e-6;
/// How far the pointer may wander, in points, for a press to be a click.
const CLICK_SLOP: f32 = 4.0;
/// The handles a pivot has: no size of its own to change.
pub(crate) const PIVOT_HANDLES: Handles = Handles { moves: true, turns: true, scales: false };

/// Edit Pivot's state.
#[derive(Default)]
pub(crate) struct PivotState {
    pub on: bool,
    /// Where a press landed on the active object, and where the pointer was.
    click: Option<(Vec2, Vec3)>,
}

/// A copy of `mesh` with its points at `positions`, numbered afresh, with
/// everything a point or a face carries brought along (the desktop's
/// `scene::duplicate_placed`).
pub(crate) fn placed_copy(mesh: &Mesh, positions: Vec<Vec3>) -> Mesh {
    let hidden = mesh.hidden_triangles().unwrap_or(&[]);
    let (mut copy, numbering) = Mesh::new_numbered(positions, mesh.indices.clone(), Some(mesh.mask.clone()), hidden);
    copy.levels = mesh.levels.as_ref().and_then(|levels| levels.renumbered(&copy, &numbering));
    copy.layers = mesh.layers_renumbered(&numbering.vertex);
    copy.set_group_ids(crate::mesh::groups::renumbered(mesh.group_ids(), &numbering.triangle));
    let _ = copy.set_uvs(crate::mesh::uvs::renumbered(mesh.uvs(), &numbering.triangle));
    copy.paint = mesh.paint_renumbered(&numbering.vertex, copy.vertex_count());
    let moved = numbering.vertex;
    if let Some(polys) = &mesh.polys {
        let corners: Vec<u32> = polys.indices.iter().map(|&i| moved.get(i as usize).copied().unwrap_or(u32::MAX)).collect();
        if !corners.contains(&u32::MAX) {
            copy.polys = Some(Polys { start: polys.start.clone(), indices: corners });
        }
    }
    copy
}

/// The middle of the box round `positions`, in double, and its longest side.
fn middle_of(positions: &[Vec3]) -> Option<(DVec3, f64)> {
    let (mut low, mut high) = (DVec3::INFINITY, DVec3::NEG_INFINITY);
    for p in positions {
        let p = p.as_dvec3();
        (low, high) = (low.min(p), high.max(p));
    }
    let middle = (low + high) * 0.5;
    (!positions.is_empty() && middle.is_finite()).then(|| (middle, (high - low).max_element()))
}

impl GolemApp {
    /// How many times longer the active object's longest side is stretched than its shortest.
    fn stretch(&self) -> f32 {
        let scale = self.active_object().scale.abs();
        scale.max_element() / scale.min_element().max(FLATTENED)
    }

    /// Whether Apply Scale has anything to do on the active object.
    pub(crate) fn scale_is_uneven(&self) -> bool {
        self.stretch() > 1.0 + UNEVEN && self.active_object().scale.abs().min_element() > FLATTENED
    }

    /// How far the active object's centre is from the middle of its mesh;
    /// `None` when it is there already.
    pub(crate) fn origin_off_mesh(&self) -> Option<Vec3> {
        let (middle, longest) = middle_of(&self.mesh.positions)?;
        let shift = middle.as_vec3();
        (middle.abs().max_element() > longest * ALREADY_THERE && shift != Vec3::ZERO).then_some(shift)
    }

    /// The active mesh's points moved to `positions` in a new mesh, and the
    /// active object's place changed by `place`, as one undo step.
    fn replace_placed(&mut self, positions: Vec<Vec3>, turn_layers: Option<Mat3>, place: impl FnOnce(&mut crate::scene::Object)) {
        self.deform_commit();
        let id = self.active;
        let mut moved = placed_copy(&self.mesh, positions);
        if let Some(turn) = turn_layers {
            moved.turn_layers(turn);
        }
        let Some(o) = self.objects.iter_mut().find(|o| o.id == id) else { return };
        let before = o.props();
        place(o);
        let old = std::mem::replace(&mut self.mesh, moved);
        self.push_undo(Step::Many(vec![Step::Mesh(id, Box::new(old)), Step::Props(vec![(id, before)])]));
        self.upload_whole = true;
        self.sculptor.last_hit = None;
        self.mark_changed();
    }

    // ---- Apply Scale --------------------------------------------------------------

    fn apply_scale_refusal(&self) -> Option<String> {
        if self.mesh.levels.is_some() {
            return Some(
                "The Scale was not applied: the coarser subdivision levels keep the axes they were made in. Delete \
                 them first (the Levels card), then apply it."
                    .into(),
            );
        }
        if self.mesh.layers.is_some() {
            return Some("The Scale was not applied: sculpt layers keep the axes they were made in. Bake them first (the Layers card), then apply it.".into());
        }
        None
    }

    fn apply_scale_step(&mut self) {
        let by = self.active_object().scale.abs();
        let positions = self.mesh.positions.iter().map(|&p| p * by).collect();
        self.replace_placed(positions, None, |o| {
            o.scale = Vec3::select(o.scale.cmplt(Vec3::ZERO), Vec3::NEG_ONE, Vec3::ONE);
        });
    }

    /// Apply Scale: the button.
    pub(crate) fn apply_scale(&mut self) {
        let name = self.active_object().name.clone();
        if !self.scale_is_uneven() {
            self.status = format!("The Scale of \"{name}\" is the same every way: there is nothing to apply.");
            return;
        }
        if let Some(why) = self.apply_scale_refusal() {
            self.status = why;
            return;
        }
        self.apply_scale_step();
        self.status = format!(
            "Put the Scale of \"{name}\" into its mesh. Nothing changed on screen, and brushes land round on it now. \
             Press Remesh to even out the triangles too. Undo (Ctrl+Z) puts it back."
        );
    }

    /// As a Remesh starts: evens a noticeably uneven Scale first, if nothing
    /// stands in the way, so the voxels are cubes in the world.
    pub(crate) fn even_scale_for_remesh(&mut self) -> bool {
        if !self.scale_is_uneven() || self.stretch() < NOTICEABLE || self.apply_scale_refusal().is_some() {
            return false;
        }
        self.apply_scale_step();
        true
    }

    // ---- Set Origin to Mesh and the pivot -----------------------------------------------

    /// Moves the active object's centre by `shift` and turns its axes by
    /// `turn`, both in its mesh's own space, the object left where it is.
    fn reorigin(&mut self, shift: Vec3, turn: Quat) {
        let turned = turn != Quat::IDENTITY;
        let back = turn.inverse();
        let carry = |p: Vec3| if turned { back * (p - shift) } else { p - shift };
        let positions = self.mesh.positions.iter().map(|&p| carry(p)).collect();
        self.replace_placed(positions, turned.then(|| Mat3::from_quat(back)), |o| {
            o.position += o.rotation * (o.scale * shift);
            if turned {
                o.rotation = (o.rotation * turn).normalize();
            }
        });
    }

    /// What the brushes' symmetry says, when the object has one: it goes
    /// through the object's centre along its axes.
    fn local_symmetry(&self) -> Option<String> {
        let s = &self.sculptor.settings;
        let mut on: Vec<String> = ["X", "Y", "Z"].iter().zip(s.symmetry).filter(|(_, on)| *on).map(|(a, _)| a.to_string()).collect();
        if let Some(radial) = super::radial::summary(&s.radial) {
            on.push(radial);
        }
        (!on.is_empty()).then(|| on.join(" "))
    }

    /// Set Origin to Mesh (and Pivot to Centre).
    pub(crate) fn set_origin_to_mesh(&mut self) {
        let name = self.active_object().name.clone();
        let Some(shift) = self.origin_off_mesh() else {
            self.status = if self.mesh.triangle_count() == 0 {
                format!("\"{name}\" has no mesh to put its centre on.")
            } else {
                format!("The centre of \"{name}\" is already in the middle of its mesh.")
            };
            return;
        };
        self.reorigin(shift, Quat::IDENTITY);
        let mut text = format!("Moved the centre of \"{name}\" to the middle of its mesh. Nothing moved in the scene.");
        if let Some(summary) = self.local_symmetry() {
            text += &format!(" Its own symmetry ({summary}) goes through the new centre now.");
        }
        self.status = text + " Undo (Ctrl+Z) puts the centre back.";
    }

    /// Whether the gizmo is on the active object's pivot (Edit Pivot).
    pub(crate) fn pivot_on(&self) -> bool {
        self.mode == Mode::Transform && self.shaping.pivot.on
    }

    /// The gizmo's target while Edit Pivot is on.
    pub(crate) fn pivot_target(&self) -> Option<Target> {
        let o = self.active_object();
        if !o.visible {
            return None;
        }
        let b = o.world_bounds(&self.mesh);
        let radius = if b.is_empty() { 0.5 } else { b.size().length() * 0.5 };
        Some(Target { placement: Placement::of(o), radius, turns: true, scales: false })
    }

    fn pivot_turn_refusal(&self) -> Option<String> {
        let name = &self.active_object().name;
        if self.mesh.levels.as_ref().is_some_and(|levels| levels.current() < levels.top()) {
            return Some(format!(
                "The pivot was not turned: \"{name}\" is below its finest subdivision level, and the finer levels \
                 keep the axes they were made in. Step up to the finest level (the Levels card), then turn it. \
                 Moving it works on any level."
            ));
        }
        let s = self.active_object().scale.abs();
        if s.max_element() > s.min_element() * (1.0 + UNEVEN) {
            return Some(format!(
                "The pivot was not turned: \"{name}\" is stretched more one way than another, and turning the axes would \
                 shear it. Make the Scale the same on all three axes (Apply Scale, the Objects card), then turn it. Moving it works."
            ));
        }
        None
    }

    /// Puts the active object's pivot at `position`, turned by `rotation`,
    /// leaving the object where it is. One undo step.
    pub(crate) fn place_pivot(&mut self, position: Vec3, rotation: Quat) {
        let o = self.active_object();
        let back = o.rotation.inverse();
        let shift = back * (position - o.position) / o.scale;
        let turn = (back * rotation).normalize();
        let name = o.name.clone();
        let size = self.mesh.bounds().size().max_element().max(f32::MIN_POSITIVE);
        let shifted = shift.length() > size * LEAST_MOVE;
        let turned = turn.angle_between(Quat::IDENTITY) > LEAST_TURN;
        if !shifted && !turned {
            return;
        }
        if let Some(why) = turned.then(|| self.pivot_turn_refusal()).flatten() {
            self.status = why;
            return;
        }
        let shift = if shifted { shift } else { Vec3::ZERO };
        let turn = if turned { turn } else { Quat::IDENTITY };
        self.reorigin(shift, turn);
        let what = match (shifted, turned) {
            (true, true) => "Moved and turned",
            (false, true) => "Turned",
            _ => "Moved",
        };
        let mut text = format!("{what} the pivot of \"{name}\". Nothing moved in the scene.");
        if let Some(summary) = self.local_symmetry() {
            text += &format!(" Its own symmetry ({summary}) goes through the pivot, along its axes.");
        }
        self.status = text + " Undo (Ctrl+Z) puts it back.";
    }

    /// Align to World: the pivot's axes back to the world's.
    fn align_pivot(&mut self) {
        let o = self.active_object();
        if o.rotation.angle_between(Quat::IDENTITY) <= LEAST_TURN {
            self.status = format!("The pivot of \"{}\" is already lined up with the world.", o.name);
            return;
        }
        self.place_pivot(o.position, Quat::IDENTITY);
    }

    /// A press off the handles with Edit Pivot on: on the active object it is
    /// kept, and true; if it is let go without wandering, the pivot goes there.
    pub(crate) fn pivot_pressed(&mut self, px: Vec2) -> bool {
        let hit = self.pick(px).filter(|(id, _)| *id == self.active).map(|(_, pos)| pos);
        self.shaping.pivot.click = hit.map(|at| (px, at));
        hit.is_some()
    }

    /// The button came up: a click kept by `pivot_pressed` puts the pivot on
    /// the surface where it was.
    pub(crate) fn pivot_released(&mut self, pointer: Option<Vec2>, ppp: f32) {
        let Some((pressed, at)) = self.shaping.pivot.click.take() else { return };
        let Some(now) = pointer else { return };
        if self.pivot_on() && pressed.distance(now) <= CLICK_SLOP * ppp {
            let rotation = self.active_object().rotation;
            self.place_pivot(at, rotation);
        }
    }

    // ---- the cards' rows ----------------------------------------------------------

    /// The Transform card's pivot rows: Edit Pivot, Pivot to Centre, Align to World.
    pub(crate) fn pivot_rows(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        theme::hint(ui, "Pivot (where the gizmo sits, and its axes)");
        let on = self.shaping.pivot.on;
        let style = ButtonStyle { active: on.then_some(ACCENT()), ..Default::default() };
        let help = "While this is on, the gizmo moves and turns the object's pivot instead of the object: drag an arrow to \
                    move it, a ring to turn its axes, or click on the object to put the pivot on the surface there. The \
                    object stays where it is. The pivot is what the object turns and resizes about, and where its own \
                    symmetry planes pass, so those come with it. Each change is one undo step.";
        if tip(theme::button(ui, ui.available_width(), "Edit Pivot", style), tips, help).clicked() {
            self.deform_commit();
            self.shaping.pivot.on = !on;
            if !on {
                self.shaping.deform.on = false;
            }
        }
        if on {
            theme::hint(ui, "Drag an arrow or a ring to move or turn the pivot, or click on the object to put it there. The object stays put.");
        }
        let off_centre = self.origin_off_mesh().is_some();
        let turned = self.active_object().rotation.angle_between(Quat::IDENTITY) > 1e-5;
        let (mut centre, mut align) = (false, false);
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(off_centre, |ui| {
                let help = "Puts the pivot in the middle of the object, for a model that came in with it far away. The object stays where it is.";
                centre = tip(theme::button(ui, w, "Pivot to Centre", ButtonStyle::default()), tips, help).clicked();
            });
            ui.add_enabled_ui(turned, |ui| {
                let help = "Turns the pivot's axes back to line up with the world's, for when they have got tipped over. The object stays where it is.";
                align = tip(theme::button(ui, w, "Align to World", ButtonStyle::default()), tips, help).clicked();
            });
        });
        if centre {
            self.set_origin_to_mesh();
        }
        if align {
            self.align_pivot();
        }
    }

    /// Apply Scale and Set Origin to Mesh, under the objects in the Objects
    /// card, each only while it would do something (as under the desktop's
    /// Scale fields).
    pub(crate) fn placement_rows(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        if self.scale_is_uneven() {
            let style = ButtonStyle { height: 26.0, ..Default::default() };
            let help = "Puts the object's stretch into its mesh and sets its Scale to 1: nothing changes on screen, but brushes land round where the ring shows them instead of stretched with the object, and Remesh makes even triangles. Remesh does this by itself first. Live mirror copies stay as they look. Undo (Ctrl+Z) puts it back.";
            if tip(theme::button(ui, ui.available_width(), "Apply Scale", style), tips, help).clicked() {
                self.apply_scale();
            }
        }
        if self.origin_off_mesh().is_some() {
            let style = ButtonStyle { height: 26.0, ..Default::default() };
            let help = "Moves the object's centre to the middle of its mesh without moving the object: nothing changes on screen. The centre is where the gizmo sits, what the object turns and resizes about, and where its own mirror planes and radial axis pass, so those come with it. For a piece cut off a model, which keeps the model's centre. Live mirror copies stay where they are. Undo (Ctrl+Z) puts the centre back.";
            if tip(theme::button(ui, ui.available_width(), "Set Origin to Mesh", style), tips, help).clicked() {
                self.deform_commit();
                self.set_origin_to_mesh();
            }
        }
    }
}
