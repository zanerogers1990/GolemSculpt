//! The Subdivision card of the Sculpt panel's Mesh tab: Subdivide, the list
//! of levels with the triangles each has, stepping between them, deleting
//! them, and what they cost in memory.
//!
//! The card shows the selected object's levels (`mesh/levels.rs`) and only
//! asks for things: every button pushes a command the app carries out after
//! the interface pass (`app/levels.rs`), so a press is one undo step however
//! the frame happens to fall.
//!
//! The levels are a list of rows and not a slider. A step between levels
//! rebuilds the mesh and is an undo step, so a control that passes through
//! every level on the way to the one wanted would do a great deal of work
//! and leave a trail of steps; a row is pressed once, and shows the triangle
//! count of its level while it waits.

use egui::Color32;

use super::dyntopo::WARNING;
use super::theme::{self, ACCENT, ButtonStyle};
use super::{fmt_count, tip};
use crate::doc::{Action, Doc};
use crate::mesh::Mesh;
use crate::mesh::levels;

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(170, 150, 235);

/// "184 MB", or "1.2 GB" from a thousand megabytes up.
pub(super) fn megabytes(bytes: usize) -> String {
    let mb = bytes as f64 / (1 << 20) as f64;
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{:.0} MB", mb.max(1.0)) }
}

pub fn card(ui: &mut egui::Ui, doc: &mut Doc, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let mesh = &doc.mesh;
    let place = mesh.levels.as_ref().map(|l| (l.current(), l.count()));
    let summary = place.map_or(String::new(), |(level, count)| format!("Level {} of {count}", level + 1));
    theme::category(ui, "Subdivision", TINT, &summary, true, |ui| {
        theme::hint(
            ui,
            "Levels of the same model, each with four times the triangles of the one below. Shape the big forms on a \
             low level and the detail on a high one: what is sculpted on one is kept on the others.",
        );
        subdivide(ui, mesh, &mut doc.opts.subdivide_flat, tips, actions);
        let Some((level, count)) = place else { return };
        ui.add_space(6.0);
        rows(ui, mesh, level, count, tips, actions);
        steppers(ui, level, count, tips, actions);
        deleters(ui, level, count, tips, actions);
        if mesh.levels.as_ref().is_some_and(|l| l.scheme() == sculpt_multires::Scheme::CatmullClark) {
            theme::hint(ui, "This model is quads on every level, and export writes the quads of the level it is on.");
        }
        let bytes = mesh.levels.as_ref().map_or(0, |l| l.whole_bytes());
        theme::hint(ui, &format!("The levels take {} of memory beside the model itself.", megabytes(bytes)));
        theme::hint(
            ui,
            "A big change made on a low level comes back up with the fine detail riding on it. One point pulled out by \
             itself on a low level comes back up as a softer, lower swelling: the finer levels round it off.",
        );
    });
}

/// The Subdivide button, what it would make, and the Flat tick.
fn subdivide(ui: &mut egui::Ui, mesh: &Mesh, flat: &mut bool, tips: bool, actions: &mut Vec<Action>) {
    let refusal = levels::subdivide_refusal(mesh);
    ui.add_enabled_ui(refusal.is_none(), |ui| {
        let style = ButtonStyle { badge: "Ctrl+D", active: Some(ACCENT()), ..Default::default() };
        let help = "Adds a level with four times the triangles above the finest one and moves to it (Ctrl + D). The \
                    model is rounded a little as it is divided, as clay is when it is smoothed; tick Flat to keep every \
                    point where it is. A quad remesh divides into quads. Undo (Z) takes the level away again.";
        if tip(theme::button(ui, ui.available_width(), "Subdivide", style), tips, help).clicked() {
            actions.push(Action::Subdivide);
        }
    });
    match refusal {
        Some(why) => theme::hint(ui, &why),
        None => {
            let (now, after) = (mesh.triangle_count(), levels::triangles_after(mesh));
            theme::hint(ui, &format!("{} to {} triangles.", fmt_count(now), fmt_count(after)));
        }
    }
    let help = "Subdivide only cuts each face into four and moves nothing: for hard-edged shapes that must keep their \
                flat faces and sharp corners. A level made flat stays flat-faced when a lower level is changed.";
    tip(theme::checkbox(ui, flat, "Flat (cut the faces, move nothing)", ACCENT()), tips, help);
}

/// One row a level, finest at the top as the finest is the top of the stack:
/// its number, how it was made and its triangles. The row of the level in
/// hand is lit; a press on another goes there.
fn rows(ui: &mut egui::Ui, mesh: &Mesh, level: usize, count: usize, tips: bool, actions: &mut Vec<Action>) {
    let Some(stack) = mesh.levels.as_ref() else { return };
    for row in (0..count).rev() {
        let flat = if stack.is_flat(row) { ", flat" } else { "" };
        let label = format!("Level {}   {} triangles{flat}", row + 1, fmt_count(stack.triangle_count(row)));
        let style = ButtonStyle { active: (row == level).then(ACCENT), height: 24.0, ..Default::default() };
        let help = if row == level {
            "The level the object is on: what you sculpt, export or measure now is this level."
        } else {
            "Go to this level. What was sculpted on the level you leave goes with you."
        };
        if tip(theme::button(ui, ui.available_width(), &label, style), tips, help).clicked() && row != level {
            actions.push(Action::Level(row));
        }
    }
}

/// Lower and Higher: one level at a time, with their keys.
fn steppers(ui: &mut egui::Ui, level: usize, count: usize, tips: bool, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        ui.add_enabled_ui(level > 0, |ui| {
            let style = ButtonStyle { badge: "PgDn", ..Default::default() };
            let help = "One level coarser (Page Down): fewer, bigger triangles, for changing the big forms. The detail \
                        of the finer levels is kept and comes back when you go up again.";
            if tip(theme::button(ui, w, "Lower", style), tips, help).clicked() {
                actions.push(Action::LevelBy(-1));
            }
        });
        ui.add_enabled_ui(level + 1 < count, |ui| {
            let style = ButtonStyle { badge: "PgUp", ..Default::default() };
            let help = "One level finer (Page Up). What you changed on this level is carried up: the finer detail moves \
                        and turns with the surface under it.";
            if tip(theme::button(ui, w, "Higher", style), tips, help).clicked() {
                actions.push(Action::LevelBy(1));
            }
        });
    });
}

/// Delete Lower and Delete Higher.
fn deleters(ui: &mut egui::Ui, level: usize, count: usize, tips: bool, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        ui.add_enabled_ui(level > 0, |ui| {
            let help = "Throws away the levels below this one, so this becomes the coarsest. The model does not change. \
                        Undo (Z) brings them back.";
            if tip(theme::button(ui, w, "Delete Lower", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::DeleteLevels { lower: true });
            }
        });
        ui.add_enabled_ui(level + 1 < count, |ui| {
            let help = "Throws away the levels above this one, and the detail that was sculpted on them, so this becomes \
                        the finest. Undo (Z) brings them back.";
            if tip(theme::button(ui, w, "Delete Higher", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::DeleteLevels { lower: false });
            }
        });
    });
}

/// Under the Dynamic Topology section of the Remesh card: why it does
/// nothing on the selected object, when that is because of its levels.
pub fn dyntopo_note(ui: &mut egui::Ui, mesh: &Mesh) {
    if mesh.level_count() > 1 {
        let text = "Off for this object while it has subdivision levels: its levels need its triangles to stay as \
                    they are. Subdivide adds detail instead.";
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(11.0)).italics().color(WARNING)).wrap());
    }
}
