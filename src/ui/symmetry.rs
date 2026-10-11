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
use sculpt_symmetry::Axis;

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
                    if a == 0 {
                        tip(r, tips, "Mirror left and right (X).");
                    }
                });
            }
        });
        radial(ui, &mut symmetry.radial, symmetry.space, tips, down_the_view);
        let guides = theme::checkbox(ui, &mut doc.opts.symmetry_guides, "Show Symmetry Guides", ACCENT);
        tip(guides, tips, "Draws the selected object's mirror planes and radial sectors on it.");
        cut_and_mirror(ui, before.space, tips, actions);
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
            let style = ButtonStyle { active: (*space == choice).then_some(ACCENT), ..Default::default() };
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

/// Cut & Mirror: the plane to cut at, and a button for each side that can be
/// the one kept. The repair for a model whose two sides no longer match.
///
/// The plane chosen is the card's own and no object's (the Unity app's
/// Symmetry Plane): it starts at X and is not kept between runs.
fn cut_and_mirror(ui: &mut egui::Ui, space: Space, tips: bool, actions: &mut Vec<Action>) {
    theme::heading(ui, "Cut & Mirror");
    theme::hint(ui, "Throws one side of the selected object away and rebuilds it as the mirror image of the other.");
    let id = egui::Id::new("cut_and_mirror_axis");
    let mut axis = ui.data(|d| d.get_temp::<usize>(id)).and_then(Axis::from_index).unwrap_or(Axis::X);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        for choice in Axis::ALL {
            let active = (axis == choice).then_some(theme::AXIS[choice.index()]);
            let style = ButtonStyle { active, ..Default::default() };
            let help = format!(
                "Cut at the object's own {} plane: the one Mirror {} sculpts across in Local space.",
                choice.name(),
                choice.name()
            );
            if tip(theme::button(ui, w, choice.name(), style), tips, &help).clicked() {
                axis = choice;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(id, axis.index()));
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        for from_positive in [true, false] {
            let (kept, rebuilt) = cut_sides(axis, from_positive);
            let help = format!(
                "Keeps the {kept} side of the selected object and rebuilds its {rebuilt} side as the exact mirror \
                 image of it, across the object's own {} plane. What was on the {rebuilt} side is thrown away. The \
                 mask comes across; hidden parts are cut too and show again. One undo step (Z).",
                axis.name()
            );
            let label = format!("{kept} to {rebuilt}");
            if tip(theme::button(ui, w, &label, ButtonStyle::default()), tips, &help).clicked() {
                actions.push(Action::CutAndMirror { axis, from_positive });
            }
        }
    });
    if space == Space::World {
        theme::hint(ui, "This cuts at the object's own plane, not the world's.");
    }
}

/// The side Cut & Mirror keeps and the side it rebuilds, as the buttons name
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
    fn an_axis_is_shown_to_three_decimals_and_reads_back() {
        assert_eq!(format_axis(Vec3::Y), "0, 1, 0");
        assert_eq!(format_axis(Vec3::new(0.6, 0.0, -0.8)), "0.6, 0, -0.8");
        assert_eq!(format_axis(Vec3::new(-0.00001, 0.57735026, 1.0)), "0, 0.577, 1");
        let axis = Vec3::new(0.2, -0.5, 0.7).normalize();
        assert!(parse_axis(&format_axis(axis)).unwrap().distance(axis) < 2e-3);
    }
}
