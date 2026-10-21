//! The Symmetry card of the Sculpt panel: where the symmetry sits, the mirror
//! axes, radial symmetry and the guides, laid out as the Unity app's is.
//!
//! The card shows the selected object's own symmetry (`scene::Symmetry`) and
//! changes that, so selecting another object changes what the card says. Only
//! Show Symmetry Guides is the whole viewport's: in the Unity app a tick that
//! hid one object's planes and left another's standing read as a tick that
//! did not work.

use egui::{Color32, vec2};
use glam::Vec3;
use sculpt_symmetry::{Axis, ops};

use super::theme::{self, ACCENT, ButtonStyle};
use super::tip;
use crate::doc::{Action, Doc};
use crate::scene::Space;
use crate::sculpt::{Radial, RadialAxis};

/// The colour of a custom radial axis: none of the three axis colours, so it
/// never reads as one of them. The viewport guide is drawn in the same.
pub const CUSTOM_AXIS: Color32 = Color32::from_rgb(255, 204, 64);

/// The colour radial symmetry about `axis` is shown in, on its button and in
/// the viewport.
pub fn axis_color(axis: RadialAxis) -> Color32 {
    match axis {
        RadialAxis::X => theme::AXIS[0],
        RadialAxis::Y => theme::AXIS[1],
        RadialAxis::Z => theme::AXIS[2],
        RadialAxis::Custom => CUSTOM_AXIS,
    }
}

pub fn card(ui: &mut egui::Ui, doc: &mut Doc, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let active = doc.scene.active();
    let before = doc.scene.symmetry(active.id);
    let mut symmetry = before;
    // Straight down the view, in the space the symmetry sits in (the selected
    // object's own, or the world's): what From View aims the custom axis along.
    let forward = doc.camera.rotation() * Vec3::NEG_Z;
    let down_the_view = if before.space == Space::World { forward } else { active.rotation.inverse() * forward };
    let of = format!("Of the selected object, \"{}\".", active.name);
    theme::category(ui, "Symmetry", Color32::from_rgb(242, 115, 158), &before.summary(), true, |ui| {
        theme::hint(ui, &of);
        space_row(ui, &mut symmetry.space, tips);
        theme::heading(ui, "Mirror strokes across");
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            for (a, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                ui.allocate_ui(vec2(w, 22.0), |ui| {
                    let r = theme::checkbox(ui, &mut symmetry.mirror[a], name, theme::AXIS[a]);
                    let help = ["Mirror left and right (X key).", "Mirror top and bottom.", "Mirror front and back."];
                    tip(r, tips, help[a]);
                });
            }
        });
        radial(ui, &mut symmetry.radial, symmetry.space, tips, down_the_view);
        let guides = theme::checkbox(ui, &mut doc.opts.symmetry_guides, "Show Symmetry Guides", ACCENT());
        tip(guides, tips, "Draws the selected object's mirror planes and radial sectors on it.");
        repair(ui, before.space, &mut doc.opts.symmetry_tolerance, tips, actions);
    });
    if symmetry != before {
        doc.scene.set_symmetry(doc.scene.active_id(), symmetry);
    }
}

/// Local or World: where the planes and the radial axis sit (Nomad's names).
fn space_row(ui: &mut egui::Ui, space: &mut Space, tips: bool) {
    theme::heading(ui, "Symmetry space");
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        for choice in Space::ALL {
            let style = ButtonStyle { active: (*space == choice).then_some(ACCENT()), ..Default::default() };
            let help = match choice {
                Space::Local => {
                    "Symmetry through the object's own centre along its own axes: it moves and turns with the object."
                }
                Space::World => {
                    "Symmetry through the world's centre along the world's axes: it stays put while the object moves \
                     through it. It pairs with Mirror in the Objects card: parts placed either side of the world's \
                     centre sculpt symmetrically."
                }
            };
            if tip(theme::button(ui, w, choice.name(), style), tips, help).clicked() {
                *space = choice;
            }
        }
    });
}

fn radial(ui: &mut egui::Ui, radial: &mut Radial, space: Space, tips: bool, down_the_view: Vec3) {
    // Whose centre and axes the repeats go round.
    let whose = if space == Space::World { "the world's" } else { "the object's" };
    theme::heading(ui, "Radial Symmetry");
    let on = theme::checkbox(ui, &mut radial.on, "Radial", axis_color(radial.axis));
    let help = format!(
        "Repeats every stroke evenly around the axis below, through {whose} centre. Combines with the mirror axes. \
         Lathed objects are turned around Y."
    );
    tip(on, tips, &help);

    // Its settings only while it is on.
    if !radial.on {
        return;
    }
    let mut count = radial.count as f32;
    let range = Radial::MIN_COUNT as f32..=Radial::MAX_COUNT as f32;
    let slider = theme::slider(ui, "Radial Repeats", &mut count, range, |v| format!("{:.0}", v.round()));
    if tip(slider, tips, "How many times each stroke is repeated around the axis.").changed() {
        radial.count = (count.round() as u32).clamp(Radial::MIN_COUNT, Radial::MAX_COUNT);
    }

    theme::hint(ui, "Radial Axis");
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 4, 6.0);
        for axis in RadialAxis::ALL {
            let style = ButtonStyle { active: (radial.axis == axis).then(|| axis_color(axis)), ..Default::default() };
            let help = match axis {
                RadialAxis::Custom => "Repeats strokes around an axis you set below: typed in, or taken from the view.".to_owned(),
                _ if space == Space::World => format!("Repeats strokes around the world's {} axis.", axis.name()),
                _ => format!("Repeats strokes around the object's own {} axis.", axis.name()),
            };
            if tip(theme::button(ui, w, axis.name(), style), tips, &help).clicked() {
                radial.axis = axis;
            }
        }
    });
    if radial.axis == RadialAxis::Custom {
        custom_axis_row(ui, radial, whose, tips, down_the_view);
    }
}

/// The custom axis as text, with From View beside it.
fn custom_axis_row(ui: &mut egui::Ui, radial: &mut Radial, whose: &str, tips: bool, down_the_view: Vec3) {
    const BUTTON: f32 = 84.0;
    ui.horizontal(|ui| {
        // The text being typed lives here while the box has the keyboard;
        // the rest of the time the box shows the axis itself.
        let id = ui.id().with("radial_custom_axis");
        let shown = format_axis(radial.custom_axis);
        let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| shown.clone());
        let field = theme::text_field(ui, &mut text, ui.available_width() - BUTTON - 6.0);
        // Only what was typed is taken. The box shows three decimals, and
        // reading those back after a click in and out would nudge an axis
        // that From View had set more finely.
        if field.lost_focus() && text != shown && !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            // Text that is not a direction leaves the axis as it was.
            if let Some(axis) = parse_axis(&text) {
                radial.custom_axis = axis;
            }
        }
        if field.has_focus() {
            ui.data_mut(|d| d.insert_temp(id, text));
        } else {
            ui.data_mut(|d| d.remove::<String>(id));
        }
        let help = format!(
            "Points the custom axis straight down the current view, through {whose} centre: seen from here, the \
             repeats land evenly around it."
        );
        if tip(theme::button(ui, BUTTON, "From View", ButtonStyle::default()), tips, &help).clicked() {
            radial.set_custom_axis(down_the_view);
        }
    });
}

/// Symmetry Repair, laid out as the Unity app's panel of that name is: the
/// plane, Match Tolerance, Check Symmetry, then the two ways of making one
/// side the mirror image of the other, and Symmetry Cleanup. For a model
/// whose two sides no longer match.
///
/// Make Symmetric stands above Cut & Mirror because it is the one to try
/// first: it only moves vertices, so the triangles, the mask and the levels
/// are all as they were. It is also the one that can refuse, and its refusal
/// names the row under it.
///
/// The plane chosen is the card's own and no object's (the Unity app's
/// Symmetry Plane): it starts at X and is not kept between runs.
fn repair(ui: &mut egui::Ui, space: Space, tolerance: &mut f32, tips: bool, actions: &mut Vec<Action>) {
    // Folded away until wanted: it is for mending, not for every session.
    if !theme::fold(ui, "Symmetry Repair", false, false) {
        return;
    }
    theme::about(
        ui,
        "For a selected object whose two sides no longer match. Check Symmetry counts the vertices that pair up across the plane. Make Symmetric moves one side onto the mirror image of the other and keeps every triangle and the mask; Cut & Mirror throws one side away and rebuilds it as the mirror image of the other. Symmetry Cleanup puts the vertices near the plane exactly on it.",
    );
    theme::hint(ui, "Symmetry Plane");
    let id = egui::Id::new("cut_and_mirror_axis");
    let mut axis = ui.data(|d| d.get_temp::<usize>(id)).and_then(Axis::from_index).unwrap_or(Axis::X);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        for choice in Axis::ALL {
            let active = (axis == choice).then_some(theme::AXIS[choice.index()]);
            let style = ButtonStyle { active, ..Default::default() };
            let help = format!(
                "The buttons below work across the object's own {} plane: the one Mirror {} sculpts across in Local \
                 space.",
                choice.name(),
                choice.name()
            );
            if tip(theme::button(ui, w, choice.name(), style), tips, &help).clicked() {
                axis = choice;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(id, axis.index()));

    match_tolerance(ui, tolerance, tips);
    let check = theme::button(ui, ui.available_width(), "Check Symmetry", ButtonStyle::default());
    let help = "Says on the status line how many of the selected object's vertices pair up across the plane, and how \
                many have no partner. Changes nothing.";
    if tip(check, tips, help).clicked() {
        actions.push(Action::CheckSymmetry { axis });
    }

    theme::heading(ui, "Make Symmetric");
    let make = |from_positive| Action::MakeSymmetric { axis, from_positive };
    side_buttons(ui, axis, tips, actions, make, |copied, moved| {
        format!(
            "Moves the {moved} side's vertices onto the mirror image of the {copied} side, each onto its own partner \
             across the object's own {} plane. No triangle changes. With a mask painted only the unmasked part \
             moves. Where too much of the model has no partner nothing is changed and the status line says so: \
             Cut & Mirror is for that. One undo step (Z).",
            axis.name()
        )
    });

    theme::heading(ui, "Cut & Mirror");
    let cut = |from_positive| Action::CutAndMirror { axis, from_positive };
    side_buttons(ui, axis, tips, actions, cut, |kept, rebuilt| {
        format!(
            "Keeps the {kept} side of the selected object and rebuilds its {rebuilt} side as the exact mirror \
             image of it, across the object's own {} plane. What was on the {rebuilt} side is thrown away. The \
             mask comes across; hidden parts are cut too and show again. One undo step (Z).",
            axis.name()
        )
    });

    ui.add_space(4.0);
    let cleanup = theme::button(ui, ui.available_width(), "Symmetry Cleanup", ButtonStyle::default());
    let help = "Puts the vertices that sit close to the plane exactly on it, and merges the doubled ones that leaves: \
                two halves joined along the plane become one surface. Neither side is moved as a whole. One undo \
                step (Z).";
    if tip(cleanup, tips, help).clicked() {
        actions.push(Action::SymmetryCleanup { axis });
    }
    if space == Space::World {
        theme::hint(ui, "These work across the object's own plane, not the world's.");
    }
}

/// The Match Tolerance slider. Equal travel doubles the tolerance, so the
/// setting it starts at sits where there is room to go either way.
fn match_tolerance(ui: &mut egui::Ui, tolerance: &mut f32, tips: bool) {
    let range = ops::MIN_TOLERANCE_SCALE..=ops::MAX_TOLERANCE_SCALE;
    let mut log = tolerance.clamp(*range.start(), *range.end()).log2();
    let ends = range.start().log2()..=range.end().log2();
    let slider = theme::slider(ui, "Match Tolerance", &mut log, ends, |v| tolerance_text(v.exp2()));
    let help = "How far apart two vertices may be and still count as a mirrored pair, from tight to loose. Widen it \
                if halves that were built separately are not matching up. For Cut & Mirror it is how near the plane \
                a vertex must be to be kept as part of the seam.";
    if tip(slider, tips, help).changed() {
        *tolerance = log.exp2().clamp(*range.start(), *range.end());
    }
}

/// Match Tolerance as the slider shows it: a multiple of the tolerance the
/// mesh itself suggests, to two decimals without trailing zeros.
fn tolerance_text(scale: f32) -> String {
    let text = format!("{scale:.2}");
    format!("{}x", text.trim_end_matches('0').trim_end_matches('.'))
}

/// A button for each side of the plane that can be the one a repair copies:
/// "+X to -X" and "-X to +X". `help` is given the side copied and the side
/// changed, in that order.
fn side_buttons(
    ui: &mut egui::Ui,
    axis: Axis,
    tips: bool,
    actions: &mut Vec<Action>,
    action: impl Fn(bool) -> Action,
    help: impl Fn(&str, &str) -> String,
) {
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        for from_positive in [true, false] {
            let (from, to) = cut_sides(axis, from_positive);
            let label = format!("{from} to {to}");
            if tip(theme::button(ui, w, &label, ButtonStyle::default()), tips, &help(&from, &to)).clicked() {
                actions.push(action(from_positive));
            }
        }
    });
}

/// The side a repair copies and the side it changes, as the buttons name
/// them: "+X" and "-X".
fn cut_sides(axis: Axis, from_positive: bool) -> (String, String) {
    let (plus, minus) = (format!("+{}", axis.name()), format!("-{}", axis.name()));
    if from_positive { (plus, minus) } else { (minus, plus) }
}

fn format_axis(axis: Vec3) -> String {
    // Three decimals at most, without trailing zeros.
    let part = |v: f32| ((v * 1000.0).round() / 1000.0 + 0.0).to_string();
    format!("{}, {}, {}", part(axis.x), part(axis.y), part(axis.z))
}

/// "x, y, z", with commas, spaces or semicolons between. `None` when it does
/// not read as a direction.
fn parse_axis(text: &str) -> Option<Vec3> {
    let parts: Vec<f32> =
        text.split([',', ' ', ';']).filter(|p| !p.is_empty()).map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let &[x, y, z] = parts.as_slice() else { return None };
    let axis = Vec3::new(x, y, z);
    (axis.is_finite() && axis.length_squared() > 1e-12).then(|| axis.normalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_axis_reads_with_commas_spaces_or_semicolons() {
        assert_eq!(parse_axis("0, 0, 2"), Some(Vec3::Z));
        assert_eq!(parse_axis("  -3 0 0 "), Some(Vec3::NEG_X));
        assert_eq!(parse_axis("0;1;0"), Some(Vec3::Y));
        let tilted = parse_axis("1, 1, 0").unwrap();
        assert!((tilted - Vec3::new(1.0, 1.0, 0.0).normalize()).length() < 1e-6, "{tilted}");
        // Not a direction: the axis in use stays as it is.
        for bad in ["", "1, 2", "1, 2, 3, 4", "up", "0, 0, 0", "1, nan, 0", "1, inf, 0"] {
            assert_eq!(parse_axis(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_cut_and_mirror_buttons_name_the_side_kept_first() {
        assert_eq!(cut_sides(Axis::X, true), ("+X".to_owned(), "-X".to_owned()));
        assert_eq!(cut_sides(Axis::Z, false), ("-Z".to_owned(), "+Z".to_owned()));
    }

    #[test]
    fn match_tolerance_reads_as_a_multiple_without_trailing_zeros() {
        assert_eq!(tolerance_text(1.0), "1x");
        assert_eq!(tolerance_text(ops::MIN_TOLERANCE_SCALE), "0.25x");
        assert_eq!(tolerance_text(ops::MAX_TOLERANCE_SCALE), "8x");
        assert_eq!(tolerance_text(2.828_427), "2.83x");
        assert_eq!(tolerance_text(0.5), "0.5x");
    }

    #[test]
    fn an_axis_is_shown_to_three_decimals_and_reads_back() {
        assert_eq!(format_axis(Vec3::Y), "0, 1, 0");
        assert_eq!(format_axis(Vec3::new(0.6, 0.0, -0.8)), "0.6, 0, -0.8");
        assert_eq!(format_axis(Vec3::new(-0.00001, 0.57735026, 1.0)), "0, 0.577, 1");
        let axis = Vec3::new(0.2, -0.5, 0.7).normalize();
        assert!(parse_axis(&format_axis(axis)).unwrap().distance(axis) < 2e-3);
    }
}
