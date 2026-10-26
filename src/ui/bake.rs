//! The UVs & Maps card of the Sculpt panel's Mesh tab: Unwrap UVs, and Bake
//! Maps from a sculpt onto the selected object (app/bake.rs).
//!
//! The usual way through: Clone the sculpt, Quad Remesh the copy (or bring
//! in a low model with UVs from elsewhere), Unwrap UVs on it, then Bake Maps
//! from the sculpt: normal, displacement, ambient occlusion and colour maps
//! for a game engine or a renderer, with the low model exported beside them.

use egui::Color32;

use super::theme::{self, ACCENT, ButtonStyle};
use super::tip;
use crate::doc::{Action, Doc};

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(235, 170, 120);

pub fn card(ui: &mut egui::Ui, doc: &mut Doc, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let has = doc.mesh.has_uvs();
    let summary = if has { "has UVs" } else { "" };
    theme::category(ui, "UVs & Maps", TINT, summary, false, |ui| {
        theme::about(
            ui,
            "UVs lay a model's surface out flat in a square, so pictures (maps) can be painted on it. For a game or a \
             render: Clone the sculpt, Quad Remesh the copy, Unwrap UVs on it, then Bake Maps from the sculpt and \
             export the copy with the maps.",
        );
        let line = if has {
            "This object has UVs. Unwrapping again makes new ones."
        } else {
            "This object has no UVs."
        };
        theme::hint(ui, line);
        let choices = &mut doc.opts.bake;
        let groups = doc.mesh.has_groups();
        ui.add_enabled_ui(groups, |ui| {
            let help = "Cut the UVs along the face groups' borders, so the seams go where you drew them. Without \
                        face groups the unwrap places the seams itself.";
            tip(theme::checkbox(ui, &mut choices.seams_on_groups, "Seams on Face Groups", ACCENT()), tips, help);
        });
        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
        let label = if has { "Unwrap UVs Again" } else { "Unwrap UVs" };
        let help = "Lays the selected object out flat in the square: UVs for baking and painting maps, kept when it \
                    is exported as OBJ or FBX. The model does not move. One undo step (Ctrl+Z).";
        if tip(theme::button(ui, ui.available_width(), label, style), tips, help).clicked() {
            actions.push(Action::UnwrapUvs);
        }
        ui.add_space(8.0);
        theme::heading(ui, "Bake Maps");
        bake(ui, doc, has, tips, actions);
    });
}

/// The bake: which object the detail comes from, the maps' size, how far to
/// look, which maps, and the button (app/bake.rs).
fn bake(ui: &mut egui::Ui, doc: &mut Doc, has: bool, tips: bool, actions: &mut Vec<Action>) {
    let from = crate::app::project::source(&doc.scene, &doc.mesh, doc.opts.bake.from).map(|o| (o.id, o.name.clone()));
    let active = doc.scene.active();
    let others: Vec<(u64, String)> = doc
        .scene
        .objects
        .iter()
        .filter(|o| o.id != active.id && o.mesh != active.mesh)
        .map(|o| (o.id, o.name.clone()))
        .collect();
    if others.is_empty() {
        theme::hint(ui, "Needs the sculpt to bake from as a second object, where this one is.");
        return;
    }
    let c = &mut doc.opts.bake;
    ui.horizontal(|ui| {
        ui.label("From");
        let shown = from.as_ref().map_or("Choose an object", |(_, name)| name.as_str());
        egui::ComboBox::from_id_salt("bake_from").selected_text(shown).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
            for (id, name) in &others {
                let picked = from.as_ref().is_some_and(|(f, _)| f == id);
                if ui.selectable_label(picked, name).clicked() {
                    c.from = Some(*id);
                }
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label("Size");
        let w = ((ui.available_width() - 6.0) / crate::app::bake::SIZES.len() as f32).max(30.0);
        for size in crate::app::bake::SIZES {
            let style = ButtonStyle { height: 22.0, active: (c.size == size).then_some(ACCENT()), ..Default::default() };
            let label = format!("{}K", size / 1024);
            let help = format!("Maps {size} pixels square.");
            if tip(theme::button(ui, w, &label, style), tips, &help).clicked() {
                c.size = size;
            }
        }
    });
    // Log slider, as Project Detail's Reach.
    let range = crate::project::REACH_RANGE;
    let mut log = c.reach.clamp(*range.start(), *range.end()).ln();
    let size = crate::app::project::reach(&doc.scene, &doc.mesh, 1.0);
    let readout = |v: f32| sculpt_geom::units::format(v.exp() * size, sculpt_geom::LengthUnit::Millimetres, true);
    let reach = theme::slider(ui, "Reach", &mut log, range.start().ln()..=range.end().ln(), readout);
    if reach.changed() {
        c.reach = log.exp();
    }
    tip(reach, tips, "How far from this object's surface to look for the sculpt's: as Project Detail's Reach.");
    let maps: [(&mut bool, &str, &str); 4] = [
        (&mut c.normal, "Normal", "The sculpt's detail as a tangent-space normal map (OpenGL, Y up), as Blender, Unity and Godot read it."),
        (&mut c.displacement, "Displacement", "How far the sculpt stands off this surface: a 32-bit EXR, a 16-bit PNG and a note of the range."),
        (&mut c.ao, "Ambient Occlusion", "How much light reaches each place on the sculpt. The slow one: seconds for a 2K map."),
        (&mut c.colour, "Colour", "The sculpt's paint, as a colour map."),
    ];
    for (on, label, help) in maps {
        tip(theme::checkbox(ui, on, label, ACCENT()), tips, help);
    }
    ui.add_enabled_ui(has && from.is_some(), |ui| {
        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
        let label = from.as_ref().map_or("Bake Maps...".to_owned(), |(_, name)| format!("Bake Maps from {name}..."));
        let help = "Asks for a folder, then bakes the ticked maps in the background and writes them there, named after \
                    this object. Export this object as OBJ or FBX to go with them.";
        if tip(theme::button(ui, ui.available_width(), &label, style), tips, help).clicked() {
            actions.push(Action::BakeMaps);
        }
    });
    if !has {
        theme::hint(ui, "Unwrap UVs first: the maps are laid out by them.");
    } else if from.is_none() {
        theme::hint(ui, "No other object overlaps this one. Put the low model where the sculpt is.");
    }
}
