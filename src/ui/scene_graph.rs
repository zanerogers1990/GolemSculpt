//! The Objects card of the Scene panel: add shapes, and the list of objects
//! with select, show / hide, rename, delete, clone, instance and reorder.
//! Rows and wording follow the Unity app's scene graph; holding a row to lift
//! it out and drag it to a new place is this app's own.
//!
//! Ctrl or Shift + click on a row adds it to the selection. The row brushes
//! work on is bright, the others selected with it grey. Join merges them
//! into the bright one, and Subtract, Union and Intersect cut it with them
//! or weld them into it. The question the Delete key asks is drawn here too.

use std::collections::HashMap;
use std::time::Duration;

use egui::{Align2, Color32, Id, Key, LayerId, Order, Painter, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::assets::Assets;
use super::question::Question;
use super::theme::{self, ACCENT, ButtonStyle};
use super::{UiState, tip};
use crate::doc::{Action, Doc};
use crate::mesh::primitives::Primitive;
use crate::scene::{Object, Scene};

const ROW: f32 = 24.0;
/// Row height plus the gap under it.
const PITCH: f32 = 28.0;
/// The list scrolls once it is longer than this.
const MAX_ROWS: f32 = 8.0;
/// Seconds a press must rest on a row before the row lifts out.
const HOLD: f64 = 0.25;
/// Name colour of objects that share their mesh with another.
const LINKED: Color32 = Color32::from_rgb(140, 217, 255);
/// Name colour of objects with a live mirror, as in the Unity app.
const MIRRORED: Color32 = Color32::from_rgb(255, 209, 115);
/// Row colour of objects selected along with the active one.
const ALSO: Color32 = Color32::from_rgb(102, 102, 115);

struct Rename {
    id: u64,
    text: String,
    /// The field has not been given the keyboard yet.
    fresh: bool,
}

/// A row held by the pointer, out of the list.
struct Lifted {
    id: u64,
    /// Pointer height within the row when it was picked up.
    grab: f32,
    /// The row of the list it would drop into.
    slot: usize,
}

pub struct State {
    rename: Option<Rename>,
    lifted: Option<Lifted>,
    /// Where each row is drawn, in rows from the top of the list. Eases
    /// toward the row's real place, so rows glide aside for a lifted one.
    places: HashMap<u64, f32>,
    /// Object and text of the name field under the list.
    field: (u64, String),
    /// A position or scale edit is under way (and already has its undo step).
    moving: bool,
    /// Remesh after Join.
    join_remesh: bool,
    /// Voxel resolution for Subtract, Union and Intersect, once it has been
    /// set here; until then it follows the Remesh card's.
    bool_resolution: Option<f32>,
    /// Delete the cutters of a boolean instead of hiding them.
    bool_delete: bool,
    /// The question that is up, waiting for an answer: the Delete key's, or
    /// one about closing or opening over unsaved work (see question.rs).
    confirm: Option<Question>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            rename: None,
            lifted: None,
            places: HashMap::new(),
            field: Default::default(),
            moving: false,
            // On, as in the Unity app: a plain join leaves the walls where
            // the objects overlapped inside the result.
            join_remesh: true,
            bool_resolution: None,
            bool_delete: false,
            confirm: None,
        }
    }
}

impl State {
    /// Puts up the question "delete this object?".
    pub fn ask_delete(&mut self, id: u64) {
        self.confirm = Some(Question::Delete(id));
    }

    /// Puts up any question (it takes the place of one already up).
    pub fn ask(&mut self, question: Question) {
        self.confirm = Some(question);
    }

    /// The question that is up, if one is.
    pub fn question(&self) -> Option<&Question> {
        self.confirm.as_ref()
    }

    /// Whether a question is up, waiting for an answer.
    pub fn asking(&self) -> bool {
        self.confirm.is_some()
    }

    /// Takes the question down. A yes (Enter) gives back what the answer
    /// Enter stands for sets in motion (`Question::default_action`: the first
    /// answer, or nothing where Enter is Cancel); a no (Esc) is Cancel.
    pub fn answer(&mut self, yes: bool) -> Option<Action> {
        self.confirm.take().filter(|_| yes).and_then(|question| question.default_action())
    }
}

pub fn show(ui: &mut Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let count = doc.scene.objects.len();
    let mut summary = if count == 1 { "1 object".to_owned() } else { format!("{count} objects") };
    let picked = doc.scene.selected().len();
    if picked > 1 {
        summary += &format!(", {picked} selected");
    }
    let (assets, st) = (&state.assets, &mut state.graph);
    theme::category(ui, "Objects", Color32::from_rgb(76, 153, 255), &summary, true, |ui| {
        theme::heading(ui, "Add");
        for row in Primitive::ALL.chunks(3) {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, row.len(), 4.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for &kind in row {
                    let r = theme::tile(ui, vec2(w, 46.0), assets.icon(kind.icon()), kind.name(), "", None);
                    let help = format!("Adds a new sculptable {}.", kind.name().to_lowercase());
                    if tip(r, tips, &help).clicked() {
                        actions.push(Action::New(kind));
                    }
                }
            });
        }
        let mut log = doc.opts.new_shape_tris.log10();
        if theme::slider(ui, "Starting Detail", &mut log, 4.0..=6.6, |v| {
            format!("{} tris", super::fmt_count(10f32.powf(v) as usize))
        })
        .changed()
        {
            doc.opts.new_shape_tris = 10f32.powf(log);
        }
        super::spheres::new_rig_button(ui, assets.icon("tool_sspheres"), tips, actions);
        theme::hint(ui, "Ctrl + click adds to the selection. Double-click renames. Hold a row, then drag, to reorder.");
        if doc.scene.in_local_view() {
            // Stays up for as long as it is on: a scene with most of its
            // objects missing should say why.
            let text = "Local View is on: the rest of the scene is set aside. Press / to bring it back.";
            ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(11.0)).color(theme::KEY)).wrap());
        }

        list(ui, &doc.scene, assets, st, tips, actions);

        theme::heading(ui, "Selected Object");
        selected(ui, doc, assets, st, tips, actions);
        super::mirror_copies::show(ui, doc, assets, tips, actions);
        combine(ui, doc, assets, st, tips, actions);
    });
}

/// The three parts of a row: name, eye, trash.
fn parts(rect: Rect) -> [Rect; 3] {
    let trash = Rect::from_min_max(pos2(rect.right() - 24.0, rect.top()), rect.max);
    let eye = trash.translate(vec2(-30.0, 0.0));
    let name = Rect::from_min_max(rect.min, pos2(eye.left() - 6.0, rect.bottom()));
    [name, eye, trash]
}

struct Look {
    /// The active object: the one brushes work on.
    selected: bool,
    /// Selected along with the active one.
    also: bool,
    linked: bool,
    /// Has live mirror copies.
    mirrored: bool,
    can_delete: bool,
    /// The name is a text field just now, so only the buttons are painted.
    editing: bool,
    /// Pointer over the name, eye, trash.
    hover: [bool; 3],
}

fn paint_row(p: &Painter, rect: Rect, object: &Object, look: &Look, assets: &Assets) {
    let [name, eye, trash] = parts(rect);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let glow = |r: Rect, on: bool| {
        if on {
            p.rect_filled(r, 5.0, Color32::from_white_alpha(20));
        }
    };
    if !look.editing {
        let fill = if look.selected {
            ACCENT
        } else if look.also {
            ALSO
        } else {
            theme::CONTROL
        };
        p.rect_filled(name, 5.0, fill);
        glow(name, look.hover[0]);
        let mut ink = match (look.selected || look.also, look.linked, look.mirrored) {
            (true, _, _) => Color32::WHITE,
            (false, true, _) => LINKED,
            (false, false, true) => MIRRORED,
            (false, false, false) => theme::TEXT,
        };
        if !object.visible {
            ink = ink.gamma_multiply(0.55);
        }
        let mut x = name.left() + 8.0;
        let badge = if look.linked { Some("instance") } else { look.mirrored.then_some("mirror") };
        if let Some(icon) = badge.and_then(|name| assets.icon(name)) {
            let r = Rect::from_center_size(pos2(x + 6.0, name.center().y), Vec2::splat(13.0));
            p.image(icon, r, uv, ink);
            x += 18.0;
        }
        let clip = Rect::from_min_max(name.min, pos2(name.right() - 4.0, name.bottom()));
        p.with_clip_rect(clip.intersect(p.clip_rect())).text(
            pos2(x, name.center().y),
            Align2::LEFT_CENTER,
            &object.name,
            theme::font(12.0),
            ink,
        );
    }

    glow(eye, look.hover[1]);
    let (icon, alpha) = if object.visible { ("eye", 235) } else { ("eye_off", 80) };
    if let Some(icon) = assets.icon(icon) {
        p.image(icon, Rect::from_center_size(eye.center(), Vec2::splat(16.0)), uv, Color32::from_white_alpha(alpha));
    }

    p.rect_filled(trash, 5.0, theme::CONTROL);
    glow(trash, look.hover[2] && look.can_delete);
    if let Some(icon) = assets.icon("trash") {
        let alpha = if look.can_delete { 210 } else { 60 };
        p.image(icon, Rect::from_center_size(trash.center(), Vec2::splat(14.0)), uv, Color32::from_white_alpha(alpha));
    }
}

fn list(ui: &mut Ui, scene: &Scene, assets: &Assets, st: &mut State, tips: bool, actions: &mut Vec<Action>) {
    let gone = |id: u64| scene.get(id).is_none();
    st.places.retain(|&id, _| !gone(id));
    if st.lifted.as_ref().is_some_and(|l| gone(l.id)) {
        st.lifted = None;
    }
    if st.rename.as_ref().is_some_and(|r| gone(r.id)) {
        st.rename = None;
    }
    ui.scope(|ui| {
        // A solid bar that takes its own space, so it is plainly there as
        // soon as the list is longer than its window.
        let scroll = &mut ui.spacing_mut().scroll;
        *scroll = egui::style::ScrollStyle::solid();
        scroll.bar_width = 6.0;
        egui::ScrollArea::vertical()
            .id_salt("objects")
            .max_height(MAX_ROWS * PITCH - (PITCH - ROW))
            .auto_shrink([false, true])
            .show(ui, |ui| rows(ui, scene, assets, st, tips, actions));
    });
}

fn rows(ui: &mut Ui, scene: &Scene, assets: &Assets, st: &mut State, tips: bool, actions: &mut Vec<Action>) {
    let n = scene.objects.len();
    let height = n as f32 * PITCH - (PITCH - ROW);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let (pointer, down, dt, now, pressed_at, pressed_on) = ui.input(|i| {
        let p = &i.pointer;
        (p.latest_pos(), p.primary_down(), i.stable_dt.min(0.05), i.time, p.press_start_time(), p.press_origin())
    });

    // The lifted row follows the pointer; the row it is over is where it
    // would land. On release it is drawn once more where it was let go, then
    // glides into its new place.
    let mut dropped = false;
    if let Some(l) = &mut st.lifted {
        match pointer.filter(|_| down) {
            Some(p) => {
                let top = (p.y - l.grab).clamp(area.top() - ROW * 0.5, area.bottom() - ROW * 0.5);
                st.places.insert(l.id, (top - area.top()) / PITCH);
                l.slot = (((top + ROW * 0.5 - area.top()) / PITCH).floor().max(0.0) as usize).min(n - 1);
                // Near the top or bottom of a long list, scroll to reach the rest.
                let window = ui.clip_rect();
                if p.y < window.top() + 18.0 {
                    ui.scroll_with_delta(vec2(0.0, 320.0 * dt));
                } else if p.y > window.bottom() - 18.0 {
                    ui.scroll_with_delta(vec2(0.0, -320.0 * dt));
                }
                ui.ctx().request_repaint();
            }
            None => {
                actions.push(Action::MoveObject(l.id, l.slot));
                dropped = true;
            }
        }
    }
    let lifted = st.lifted.as_ref().map(|l| (l.id, l.slot));
    let row_at = |place: f32| Rect::from_min_size(pos2(area.left(), area.top() + place * PITCH), vec2(area.width(), ROW));
    let look_of = |o: &Object| Look {
        selected: o.id == scene.active_id(),
        also: scene.is_selected(o.id),
        linked: scene.users(o.mesh) > 1,
        mirrored: o.mirror.is_some(),
        can_delete: n > 1,
        editing: false,
        hover: [false; 3],
    };

    let mut lift = None;
    let mut rename_done = false;
    let mut below = 0;
    for (i, o) in scene.objects.iter().enumerate() {
        if lifted.is_some_and(|(id, _)| id == o.id) {
            continue;
        }
        // With a row lifted the others close up, leaving a gap at its slot.
        let slot = match lifted {
            Some((_, gap)) => {
                below += 1;
                if below > gap { below } else { below - 1 }
            }
            None => i,
        };
        let place = st.places.entry(o.id).or_insert(slot as f32);
        let away = slot as f32 - *place;
        if away.abs() > 0.004 {
            *place += away * (1.0 - (-dt * 22.0).exp());
            ui.ctx().request_repaint();
        } else {
            *place = slot as f32;
        }
        let rect = row_at(*place);
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let [name, eye, trash] = parts(rect);
        let mut look = look_of(o);

        if let Some(r) = st.rename.as_mut().filter(|r| r.id == o.id) {
            look.editing = true;
            let id = Id::new(("object_rename", o.id));
            let edit = egui::TextEdit::singleline(&mut r.text).id(id).font(theme::font(12.0)).desired_width(name.width());
            let response = ui.put(name, edit);
            if r.fresh {
                // Start with the whole name selected, ready to type over.
                r.fresh = false;
                response.request_focus();
                if let Some(mut edit) = egui::TextEdit::load_state(ui.ctx(), id) {
                    let end = egui::text::CCursor::new(r.text.chars().count());
                    let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), end);
                    edit.cursor.set_char_range(Some(all));
                    edit.store(ui.ctx(), id);
                }
            } else if response.lost_focus() {
                // Enter or a click elsewhere keeps the name, Escape drops it.
                let text = r.text.trim();
                if !ui.input(|i| i.key_pressed(Key::Escape)) && !text.is_empty() {
                    actions.push(Action::RenameObject(o.id, text.to_owned()));
                }
                rename_done = true;
            }
        } else if lifted.is_none() {
            let response = ui.interact(name, Id::new(("object_row", o.id)), Sense::click_and_drag());
            look.hover[0] = response.hovered();
            if response.double_clicked() {
                st.rename = Some(Rename { id: o.id, text: o.name.clone(), fresh: true });
            } else if response.clicked() {
                // Ctrl or Shift adds the row to the selection; the active
                // object stays the one brushes work on.
                let add = ui.input(|i| i.modifiers.ctrl || i.modifiers.command || i.modifiers.shift);
                actions.push(if add { Action::SelectAlso(o.id) } else { Action::SelectObject(o.id) });
            }
            if response.is_pointer_button_down_on() && n > 1 && st.rename.is_none() {
                let held = pressed_at.map_or(0.0, |t| now - t);
                let moved = pressed_on.zip(pointer).is_some_and(|(a, b)| (b.y - a.y).abs() > 5.0);
                if held >= HOLD || moved {
                    if let Some(p) = pointer {
                        lift = Some(Lifted { id: o.id, grab: p.y - rect.top(), slot: i });
                    }
                } else {
                    ui.ctx().request_repaint_after(Duration::from_secs_f64(HOLD - held));
                }
            }
            let help = if look.linked {
                "Shares its mesh with the other blue objects: sculpting any of them changes all. Click to select, Ctrl + click to add to the selection, double-click to rename, hold and drag to reorder."
            } else {
                "Click to select, Ctrl + click to add to the selection, double-click to rename, hold and drag to reorder."
            };
            let note = "Mirrored live: its copies are drawn from this mesh (see Mirror below). ";
            tip(response, tips, &if look.mirrored { format!("{note}{help}") } else { help.to_owned() });
        }

        if lifted.is_none() {
            let response = ui.interact(eye, Id::new(("object_eye", o.id)), Sense::click());
            look.hover[1] = response.hovered();
            if tip(response, tips, "Shows or hides this object in the viewport.").clicked() {
                actions.push(Action::ToggleVisible(o.id));
            }
            let response = ui.interact(trash, Id::new(("object_trash", o.id)), Sense::click());
            look.hover[2] = response.hovered();
            let help = if look.linked {
                "Deletes this object. The mesh and its other copies stay. Undo (Z) restores it."
            } else {
                "Deletes this object. Undo (Z) restores it."
            };
            if tip(response, tips, help).clicked() && look.can_delete {
                actions.push(Action::DeleteObject(o.id));
            }
        }
        paint_row(ui.painter(), rect, o, &look, assets);
    }

    if let Some((id, slot)) = lifted {
        // The gap it would drop into...
        let gap = row_at(slot as f32);
        ui.painter().rect_filled(gap, 5.0, theme::FIELD);
        ui.painter().rect_stroke(gap, 5.0, Stroke::new(1.0, ACCENT.gamma_multiply(0.6)), StrokeKind::Inside);
        // ...and the row itself, drawn above everything so it can leave the list.
        if let Some(o) = scene.get(id) {
            let place = st.places.get(&id).copied().unwrap_or(slot as f32);
            let rect = row_at(place).expand2(vec2(4.0, 1.0));
            let p = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new("object_lifted")));
            p.rect_filled(rect.translate(vec2(0.0, 4.0)).expand(2.0), 8.0, Color32::from_black_alpha(110));
            p.rect_filled(rect.expand(2.0), 7.0, theme::HEADER);
            let mut look = look_of(o);
            look.hover[0] = true;
            paint_row(&p, rect, o, &look, assets);
            p.rect_stroke(rect.expand(2.0), 7.0, Stroke::new(1.5, Color32::from_white_alpha(150)), StrokeKind::Inside);
        }
    }
    if dropped {
        st.lifted = None;
    } else if lift.is_some() {
        st.lifted = lift;
        ui.ctx().request_repaint();
    }
    if rename_done {
        st.rename = None;
    }
}

/// Name field, copy buttons and position of the active object.
fn selected(ui: &mut Ui, doc: &mut Doc, assets: &Assets, st: &mut State, tips: bool, actions: &mut Vec<Action>) {
    let active = doc.scene.active().clone();

    let id = Id::new("object_name_field");
    if st.field.0 != active.id && !ui.memory(|m| m.has_focus(id)) {
        st.field = (active.id, active.name.clone());
    }
    let edit = egui::TextEdit::singleline(&mut st.field.1).id(id).font(theme::font(11.0)).desired_width(f32::INFINITY);
    let response = ui.add(edit);
    if response.lost_focus() {
        let text = st.field.1.trim();
        if !ui.input(|i| i.key_pressed(Key::Escape)) && !text.is_empty() {
            actions.push(Action::RenameObject(st.field.0, text.to_owned()));
        }
        st.field.0 = 0;
    } else if !response.has_focus() {
        st.field = (active.id, active.name.clone());
    }
    tip(response, tips, "The selected object's name - type to rename it.");

    let w = theme::row_width(ui, 2, 6.0);
    ui.horizontal(|ui| {
        let style = ButtonStyle { icon: assets.icon("clone"), height: 26.0, ..Default::default() };
        if tip(theme::button(ui, w, "Clone", style), tips, "Duplicates the selected object as a new, independent copy.").clicked() {
            actions.push(Action::CloneObject);
        }
        // Greyed until the object has been sculpted: before that it is
        // already the shape this goes back to.
        ui.add_enabled_ui(doc.scene.can_reset(&doc.mesh), |ui| {
            let style = ButtonStyle { height: 26.0, ..Default::default() };
            let help = "Puts the selected object back to the shape it had before any sculpting: as it was made, loaded or last remeshed. Undo (Z) brings the sculpt back.";
            if tip(theme::button(ui, w, "Reset Shape", style), tips, help).clicked() {
                actions.push(Action::ResetShape);
            }
        });
    });
    ui.horizontal(|ui| {
        let style = ButtonStyle { icon: assets.icon("instance"), height: 26.0, ..Default::default() };
        let help = "Adds a linked copy that shares this object's mesh: sculpt any one of them and they all change. Each has its own position, name and visibility. It lands on top of the original.";
        if tip(theme::button(ui, w, "Instance", style), tips, help).clicked() {
            actions.push(Action::InstanceObject);
        }
        ui.add_enabled_ui(doc.scene.users(active.mesh) > 1, |ui| {
            let style = ButtonStyle { height: 26.0, ..Default::default() };
            let help = "Gives the selected instance its own copy of the mesh, so sculpting it no longer changes the others.";
            if tip(theme::button(ui, w, "Make Unique", style), tips, help).clicked() {
                actions.push(Action::MakeUnique);
            }
        });
    });

    theme::heading(ui, "Position");
    let mut position = active.position.to_array();
    let (mut changed, mut busy) = (false, false);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        for (value, axis) in position.iter_mut().zip(["X ", "Y ", "Z "]) {
            let drag = egui::DragValue::new(value).speed(0.005).fixed_decimals(2).prefix(axis);
            let response = ui.add_sized([w, 22.0], drag);
            changed |= response.changed();
            busy |= response.dragged() || response.has_focus();
            tip(response, tips, "Drag left or right to move the object, or double-click to type a value.");
        }
    });
    if changed {
        // One undo step for the whole drag, taken before its first change.
        if !st.moving {
            st.moving = true;
            doc.history.push_scene(doc.scene.state());
        }
        doc.scene.active_mut().position = position.into();
    }

    theme::heading(ui, "Scale");
    let mut scale = active.scale.to_array();
    let mut changed = false;
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        for (value, axis) in scale.iter_mut().zip(["X ", "Y ", "Z "]) {
            let drag = egui::DragValue::new(value).speed(0.005).range(0.02..=1000.0).fixed_decimals(2).prefix(axis);
            let response = ui.add_sized([w, 22.0], drag);
            changed |= response.changed();
            busy |= response.dragged() || response.has_focus();
            tip(response, tips, "Drag left or right to stretch the object along its own axis, or double-click to type a value.");
        }
    });
    if changed {
        if !st.moving {
            st.moving = true;
            doc.history.push_scene(doc.scene.state());
        }
        doc.scene.active_mut().scale = scale.into();
    }
    if !busy {
        st.moving = false;
    }
    let placed = active.rotation != glam::Quat::IDENTITY || active.scale != glam::Vec3::ONE;
    ui.add_enabled_ui(placed, |ui| {
        let style = ButtonStyle { height: 26.0, ..Default::default() };
        let help = "Puts the object back to its original turn and size. It stays where it is.";
        if tip(theme::button(ui, ui.available_width(), "Reset Rotation and Scale", style), tips, help).clicked() {
            actions.push(Action::ResetTransform);
        }
    });
    theme::hint(ui, "New shapes, clones and instances start at the same place as the original.");
}

/// The booleans and Join: what they would do to the selection, their
/// settings, and the buttons.
fn combine(ui: &mut Ui, doc: &mut Doc, assets: &Assets, st: &mut State, tips: bool, actions: &mut Vec<Action>) {
    use sculpt_volume::BooleanOp::{Intersect, Subtract, Union};

    let count = doc.scene.selected().len();
    theme::heading(ui, "Combine (select 2+ objects)");
    if count > 1 {
        // Names the survivor: the result keeps that object's centre, and
        // with it the plane mirrored strokes reflect across.
        let name = &doc.scene.active().name;
        theme::hint(ui, &format!("\"{name}\", the bright one, is kept, with its centre and symmetry plane. Whatever else is selected cuts it or goes into it."));
    } else {
        theme::hint(ui, "Ctrl + click rows to select more. The bright one is the one kept.");
    }

    // The booleans keep a resolution of their own: cutting fine detail out
    // of a big block wants a far higher number than remeshing the block, and
    // moving the Remesh card's slider up there would make the next plain
    // remesh cost more than was asked for.
    let resolution = st.bool_resolution.unwrap_or(doc.opts.remesh_resolution);
    let after = if st.bool_delete { "deleted" } else { "hidden afterwards, not deleted: the eye in the list shows them again" };
    let tiles = [
        (Subtract, "bool_subtract", "Subtract", "Cuts the other selected objects out of the bright one."),
        (Union, "bool_union", "Union", "Welds the selected objects into the bright one as one solid. Unlike Join, this leaves no geometry inside the overlap."),
        (Intersect, "bool_intersect", "Intersect", "Keeps only the volume the bright object shares with every other selected object."),
    ];
    ui.add_enabled_ui(count > 1, |ui| {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, tiles.len(), 4.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            for (op, icon, label, what) in tiles {
                let r = theme::tile(ui, vec2(w, 46.0), assets.icon(icon), label, "", None);
                let help = format!("{what} The others are {after}. It clears the mask, as a remesh does. Undo (Z) restores the shape and the others.");
                if tip(r, tips, &help).clicked() {
                    actions.push(Action::Boolean { op, resolution: resolution as u32, delete: st.bool_delete });
                }
            }
        });
    });
    let mut log = resolution.log2();
    let slider = theme::slider(ui, "Voxel Resolution (across the target)", &mut log, 4f32.log2()..=1024f32.log2(), |v| {
        format!("{:.0}", v.exp2())
    });
    let help = "Voxel density the boolean is computed at, counted along the bright object's longest side: higher captures finer cutter detail but is slower.";
    if tip(slider, tips, help).changed() {
        st.bool_resolution = Some(log.exp2().round());
    }
    let help = "Deletes the other selected objects instead of just hiding them (hidden ones can be shown again from the list). Undo (Z) brings them back either way.";
    // Unity's label is "Delete the other objects instead of hiding", which
    // is wider than this panel.
    tip(theme::checkbox(ui, &mut st.bool_delete, "Delete the others instead of hiding", ACCENT), tips, help);

    // Join and its own setting, a little apart from the booleans'.
    ui.add_space(6.0);
    let help = "Rebuilds the joined result on a clean voxel grid, welding away the seams where the objects met. Like any remesh, it clears the mask.";
    tip(theme::checkbox(ui, &mut st.join_remesh, "Remesh after Join", ACCENT), tips, help);
    if st.join_remesh {
        // The Remesh card's own setting, so the two cannot disagree.
        let mut log = doc.opts.remesh_resolution.log2();
        let slider =
            theme::slider(ui, "Remesh Resolution", &mut log, 4f32.log2()..=1024f32.log2(), |v| format!("{:.0}", v.exp2()));
        let help = "Voxel density of the remesh after the join: higher is more detailed but slower. The same setting as Resolution in the Remesh card.";
        if tip(slider, tips, help).changed() {
            doc.opts.remesh_resolution = log.exp2().round();
        }
    }
    ui.add_enabled_ui(count > 1, |ui| {
        let style = ButtonStyle { icon: assets.icon("join"), ..Default::default() };
        let help = "Merges every selected object into the bright one, as one mesh. Each shell is kept as it is, masks included. Undo (Z) brings the separate objects back.";
        if tip(theme::button(ui, ui.available_width(), "Join", style), tips, help).clicked() {
            actions.push(Action::Join { remesh: st.join_remesh });
        }
    });
}

/// The question that is up (the Delete key's, or one about unsaved work),
/// over everything else: a dimmed window and a button per answer. Drawn
/// whether or not the panels are shown. Enter and Esc answer it too (the app
/// sends them here while it is up).
pub fn confirm(ctx: &egui::Context, doc: &Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let st = &mut state.graph;
    let Some(question) = &st.confirm else { return };
    // The dialog itself, and each question's words and answers, are in
    // question.rs. Answered, or the object asked about went meanwhile:
    if let Some(action) = question.show(ctx, doc) {
        st.confirm = None;
        actions.extend(action);
    }
}
