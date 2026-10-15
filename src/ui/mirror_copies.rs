//! The Live Mirror section of the Selected Object card: live copies of the selected object
//! across the world's X, Y and Z planes, drawn from its own mesh, so sculpting
//! or moving either side shows on both. Wording follows the Unity app's
//! Mirror & Combine card.
//!
//! Until the object is mirrored there is a button per world plane, and one
//! press makes the live copy. Once it is mirrored, ticks show that mirror's
//! axes and a tick changes it at once, like Nomad's repeater panel; unticking
//! the last axis would delete the mirror without saying so, and Remove is for
//! that. Validate turns the copies
//! into objects of their own. Below them, when there are copies, is the side
//! the transform gizmo is on: the object or one of its copies.

use egui::Ui;
use sculpt_symmetry::MirrorRepeater;

use super::assets::Assets;
use super::theme::{self, ACCENT, ButtonStyle};
use super::tip;
use crate::doc::{Action, Doc};
use crate::scene::MirrorCommand;

/// X, Y and Z by their bit in the mask.
const AXES: [(u8, &str); 3] = [(1, "X"), (2, "Y"), (4, "Z")];

pub fn show(ui: &mut Ui, doc: &Doc, assets: &Assets, tips: bool, actions: &mut Vec<Action>) {
    let scene = &doc.scene;
    let object = scene.active();
    theme::heading(ui, "Live Mirror (across the world origin)");
    let Some(mirror) = object.mirror else {
        // Nothing mirrored yet: a button per plane, and one press makes the copy.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = theme::row_width(ui, 3, 4.0);
            for (i, (bit, name)) in AXES.into_iter().enumerate() {
                let style = ButtonStyle { icon: (i == 0).then(|| assets.icon("mirror")).flatten(), height: 26.0, ..Default::default() };
                let help = format!(
                    "Adds a live mirrored copy of the selected object across the world {name} plane. It is the same mesh \
                     drawn again, so sculpting, remeshing or cutting either side shows on both, and moving one moves \
                     the other. More planes can be ticked afterwards. Click a copy to work from its side."
                );
                if tip(theme::button(ui, w, &format!("Mirror {name}"), style), tips, &help).clicked() {
                    actions.push(Action::Mirror(MirrorCommand::Set(bit)));
                }
            }
        });
        theme::hint(ui, "A live copy: sculpt or move either side and the other follows.");
        return;
    };

    // Mirrored: the ticks are the planes it is mirrored across, and change it at once.
    let shown = mirror.axis_mask;
    let mut axes = shown;
    ui.horizontal(|ui| {
        for (i, (bit, name)) in AXES.into_iter().enumerate() {
            let mut on = axes & bit != 0;
            let help = format!("Mirror across the world {name} plane.");
            if tip(theme::checkbox(ui, &mut on, name, theme::AXIS[i]), tips, &help).changed() {
                axes ^= bit;
            }
        }
    });
    // Unticking the last axis leaves the mirror as it is: Remove is for that.
    if axes != 0 && axes != shown {
        actions.push(Action::Mirror(MirrorCommand::Set(axes)));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, 2, 4.0);
        let style = ButtonStyle { height: 26.0, ..Default::default() };
        let help = "Turns the live mirror copies into real, separate objects, exactly where they are. Undo (Z) makes them live again.";
        if tip(theme::button(ui, w, "Validate", style), tips, help).clicked() {
            actions.push(Action::Mirror(MirrorCommand::Validate));
        }
        let style = ButtonStyle { height: 26.0, ..Default::default() };
        let help = "Removes the selected object's live mirror copies. Undo (Z) brings them back.";
        if tip(theme::button(ui, w, "Remove", style), tips, help).clicked() {
            actions.push(Action::Mirror(MirrorCommand::Remove));
        }
    });
    let (name, copies) = (&object.name, mirror.copy_count());
    let across = MirrorRepeater::axis_name_of(mirror.axis_mask);
    let live = if copies == 1 { "1 live copy" } else { &format!("{copies} live copies") };
    theme::line(ui, &format!("\"{name}\" is mirrored across {across} ({live}). Validate to make them real."), theme::OK);
    sides(ui, doc, tips, &mirror, actions);
}

/// Which side of the object the transform gizmo is on: the object, or one of
/// its copies.
fn sides(ui: &mut Ui, doc: &Doc, tips: bool, mirror: &MirrorRepeater, actions: &mut Vec<Action>) {
    let selected = doc.scene.selected_side();
    theme::hint(ui, "The Transform gizmo is on:");
    let mut chips = vec![(0u8, "Object".to_owned())];
    chips.extend(mirror.copies().iter().map(|c| (c.flip_mask, format!("Mirror {}", c.axis_name()))));
    for row in chips.chunks(3) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = theme::row_width(ui, 3, 4.0);
            for (copy, label) in row {
                let active = (*copy == selected).then_some(ACCENT());
                let style = ButtonStyle { active, height: 24.0, ..Default::default() };
                let help = if *copy == 0 {
                    "Puts the gizmo on the object itself.".to_owned()
                } else {
                    "Puts the gizmo on this copy: dragging it moves the object the other way. Clicking the copy in the viewport with the Transform tool does the same.".to_owned()
                };
                if tip(theme::button(ui, w, label, style), tips, &help).clicked() {
                    actions.push(Action::Mirror(MirrorCommand::Side(*copy)));
                }
            }
        });
    }
}
