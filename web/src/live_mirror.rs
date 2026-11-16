//! Live Mirror on the web: the desktop's section of the Selected Object card
//! (`src/ui/mirror_copies.rs`, `src/app/mirror_copies.rs`).
//!
//! An object can have live copies of itself across the world's X, Y and Z
//! planes: its own mesh drawn again through a reflected matrix
//! (`scene::Object::placements`), so sculpting either side shows on both and
//! moving the object moves its copies the other way. A stroke may begin on
//! a copy: it works the object through that copy's matrix (`GolemApp::side`).
//! Validate turns the copies into objects of their own, exactly where they
//! are shown; Remove takes them off. Each is one undo step.
//!
//! Left out from the desktop: putting the Transform gizmo on a copy (here it
//! is always on the object, and the copies follow it).

use eframe::egui::{self, Ui};
use glam::{Mat4, Vec3};
use sculpt_symmetry::{MirrorRepeater, Placement};

use super::ui::theme::{self, ButtonStyle};
use super::ui::tip;
use super::{GolemApp, Step};
use crate::mesh::Mesh;
use crate::scene::Props;

/// X, Y and Z by their bit in the mask.
const AXES: [(u8, &str); 3] = [(1, "X"), (2, "Y"), (4, "Z")];

/// What the section asks for.
pub(crate) enum MirrorCommand {
    /// The planes the active object is mirrored across (0: none).
    Set(u8),
    Validate,
}

/// The Live Mirror section, for the Objects card: what was pressed, for
/// `GolemApp::mirror_command` once the card is drawn.
pub(crate) fn section(ui: &mut Ui, name: &str, mirror: Option<MirrorRepeater>, icon: Option<egui::TextureId>, tips: bool) -> Option<MirrorCommand> {
    let mut command = None;
    theme::heading(ui, "Live Mirror (across the world origin)");
    let Some(mirror) = mirror else {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = theme::row_width(ui, 3, 4.0);
            for (i, (bit, axis)) in AXES.into_iter().enumerate() {
                let style = ButtonStyle { icon: icon.filter(|_| i == 0), height: 26.0, ..Default::default() };
                let help = format!(
                    "Adds a live mirrored copy of the active object across the world {axis} plane. It is the same mesh \
                     drawn again, so sculpting, remeshing or cutting either side shows on both, and moving one moves \
                     the other. More planes can be ticked afterwards. A stroke can begin on either side."
                );
                if tip(theme::button(ui, w, &format!("Mirror {axis}"), style), tips, &help).clicked() {
                    command = Some(MirrorCommand::Set(bit));
                }
            }
        });
        theme::hint(ui, "A live copy: sculpt or move either side and the other follows.");
        return command;
    };

    // Mirrored: the ticks are the planes, and change it at once.
    let shown = mirror.axis_mask;
    let mut axes = shown;
    ui.horizontal(|ui| {
        for (i, (bit, axis)) in AXES.into_iter().enumerate() {
            let mut on = axes & bit != 0;
            let help = format!("Mirror across the world {axis} plane.");
            if tip(theme::checkbox(ui, &mut on, axis, theme::AXIS[i]), tips, &help).changed() {
                axes ^= bit;
            }
        }
    });
    // Unticking the last axis leaves the mirror as it is: Remove is for that.
    if axes != 0 && axes != shown {
        command = Some(MirrorCommand::Set(axes));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, 2, 4.0);
        let style = ButtonStyle { height: 26.0, ..Default::default() };
        let help = "Turns the live mirror copies into real, separate objects, exactly where they are. Undo (Ctrl+Z) makes them live again.";
        if tip(theme::button(ui, w, "Validate", style), tips, help).clicked() {
            command = Some(MirrorCommand::Validate);
        }
        let style = ButtonStyle { height: 26.0, ..Default::default() };
        let help = "Removes the active object's live mirror copies. Undo (Ctrl+Z) brings them back.";
        if tip(theme::button(ui, w, "Remove", style), tips, help).clicked() {
            command = Some(MirrorCommand::Set(0));
        }
    });
    let copies = mirror.copy_count();
    let across = MirrorRepeater::axis_name_of(mirror.axis_mask);
    let live = if copies == 1 { "1 live copy".to_owned() } else { format!("{copies} live copies") };
    theme::line(ui, &format!("\"{name}\" is mirrored across {across} ({live}). Validate to make them real."), theme::OK);
    command
}

impl GolemApp {
    /// Runs a command of the Live Mirror section on the active object: one
    /// undo step each.
    pub(crate) fn mirror_command(&mut self, command: MirrorCommand) {
        if self.sculptor.is_active() || self.drag != super::Drag::None {
            return;
        }
        let (id, name) = (self.active, self.active_object().name.clone());
        match command {
            MirrorCommand::Set(axes) => {
                let center = self.active_object().mirror.map_or(Vec3::ZERO, |m| m.center);
                let wanted = MirrorRepeater::new(axes & 7, center);
                if self.active_object().mirror == wanted {
                    return;
                }
                self.change_props(id, |p| p.mirror = wanted);
                self.side = 0;
                self.status = match wanted {
                    None => format!("Removed the mirror of \"{name}\". Undo (Ctrl+Z) brings it back."),
                    Some(m) => {
                        let across = MirrorRepeater::axis_name_of(m.axis_mask);
                        format!("Mirrored \"{name}\" across {across}. Undo (Ctrl+Z) takes it off again.")
                    }
                };
            }
            MirrorCommand::Validate => self.validate_mirror(),
        }
    }

    /// Validate: each live copy of the active object becomes an object of
    /// its own, its mesh the object's reflected in its own space (mask, face
    /// groups and paint going with it), standing where the copy was shown,
    /// just under the object in the list. The object loses its mirror. One
    /// undo step (the desktop's `Scene::bake_mirror`).
    fn validate_mirror(&mut self) {
        let source = self.active_object();
        let Some(repeater) = source.mirror else { return };
        let props = source.props();
        let id = source.id;
        let placement = Placement::new(props.position, props.rotation, props.scale);
        let tidied = super::tidy(&self.mesh);
        let mesh = tidied.as_ref().unwrap_or(&self.mesh);
        let mask = mesh.mask.clone();
        let groups = mesh.group_ids().to_vec();
        let paint = mesh.paint_stack().map(std::borrow::Cow::into_owned);
        let hidden = mesh.hidden_triangles().map(<[bool]>::to_vec);
        let baked = repeater.bake(&mesh.positions, None, &mesh.indices, &placement);

        let mut steps = 0;
        let mut made = 0;
        for bake in baked {
            let (points, corners) = (bake.geometry.positions, bake.geometry.indices);
            let mut copy = crate::mesh::paint::built_with(points, corners, Some(mask.clone()), &groups, paint.as_ref());
            if let Some(hidden) = &hidden {
                let faces: Vec<u32> = (0..hidden.len() as u32).filter(|&t| hidden[t as usize]).collect();
                if !faces.is_empty() {
                    copy.hide(&faces);
                }
            }
            let Placement { position, rotation, scale } = bake.placement;
            let props = Props {
                name: format!("{} Mirror {}", props.name, bake.copy.axis_name()),
                visible: props.visible,
                position,
                rotation,
                scale,
                mirror: None,
            };
            let new = self.add_object(copy, props, false);
            steps += 1;
            made += 1;
            // Just under the object (and the copies made before it) in the list.
            if let (Some(from), Some(row)) =
                (self.objects.iter().position(|o| o.id == new), self.objects.iter().position(|o| o.id == id))
            {
                let object = self.objects.remove(from);
                let to = (row + made).min(self.objects.len());
                self.objects.insert(to, object);
                if let Some(Step::Exists { index, .. }) = self.undo.last_mut() {
                    *index = to;
                }
            }
        }
        self.change_props(id, |p| p.mirror = None);
        steps += 1;
        // One undo step for all of it.
        let at = self.undo.len().saturating_sub(steps);
        let many = self.undo.split_off(at);
        self.undo.push(Step::Many(many));
        self.side = 0;
        let (what, them) = if made == 1 {
            ("1 mirror copy is now a separate object".to_owned(), "it")
        } else {
            (format!("{made} mirror copies are now separate objects"), "them")
        };
        self.status = format!("Validated: {what}. Undo (Ctrl+Z) makes {them} live again.");
    }

    /// The matrix the brushes work the active object through: its own, or
    /// that of the live copy a stroke is on.
    pub(crate) fn work_matrix(&self) -> Mat4 {
        self.active_object().placed_matrix(self.side)
    }

    /// The active object's live copies as more places to draw its mesh.
    pub(crate) fn active_copies(&self) -> Vec<Mat4> {
        let o = self.active_object();
        if !o.visible {
            return Vec::new();
        }
        o.placements().into_iter().skip(1).map(|(_, m)| m).collect()
    }

    /// Each place `mesh` is drawn for `object`, transformed into the world,
    /// with its triangles turned where a place mirrors: what a saved file
    /// holds of the object and its live copies, named as Validate would.
    pub(crate) fn placed_parts(name: &str, object: &crate::scene::Object, mesh: &Mesh) -> Vec<(String, Vec<Vec3>, Vec<u32>)> {
        let indices = mesh.visible_indices().into_owned();
        let mut out = Vec::new();
        for (copy, place) in object.placements() {
            let positions = mesh.positions.iter().map(|&p| place.transform_point3(p)).collect();
            let mut corners = indices.clone();
            if place.determinant() < 0.0 {
                for t in corners.chunks_exact_mut(3) {
                    t.swap(1, 2);
                }
            }
            let label = if copy == 0 {
                name.to_owned()
            } else {
                format!("{name} Mirror {}", MirrorRepeater::axis_name_of(copy))
            };
            out.push((label, positions, corners));
        }
        out
    }
}
