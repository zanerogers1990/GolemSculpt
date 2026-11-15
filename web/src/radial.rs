//! Radial symmetry in the Symmetry card: every stroke repeated evenly round
//! an axis through the model's centre, on top of the mirror axes. The
//! engine does the repeating (`sculpt/symmetry.rs`); this is the desktop's
//! card for it (`ui/symmetry.rs`, `radial`), less the symmetry space, since
//! the web version has one model and it sits at the world's centre.

use eframe::egui::{self, Color32, Ui};
use glam::Vec3;

use super::ui::theme::{self, ButtonStyle};
use super::ui::tip;
use crate::sculpt::{Radial, RadialAxis};

/// The colour of a custom radial axis: none of the three axis colours, so it
/// never reads as one of them.
const CUSTOM_AXIS: Color32 = Color32::from_rgb(255, 204, 64);

fn axis_color(axis: RadialAxis) -> Color32 {
    match axis {
        RadialAxis::X => theme::AXIS[0],
        RadialAxis::Y => theme::AXIS[1],
        RadialAxis::Z => theme::AXIS[2],
        RadialAxis::Custom => CUSTOM_AXIS,
    }
}

/// What the Symmetry card's header says of radial symmetry: "Radial 8 Y",
/// or nothing while it is off.
pub fn summary(radial: &Radial) -> Option<String> {
    radial.on.then(|| format!("Radial {} {}", radial.count, radial.axis.name()))
}

/// Radial symmetry's part of the card. `down_the_view` is straight down
/// the current view, in the model's space: what From View aims a custom
/// axis along.
pub fn section(ui: &mut Ui, radial: &mut Radial, tips: bool, down_the_view: Vec3) {
    theme::heading(ui, "Radial Symmetry");
    let on = theme::checkbox(ui, &mut radial.on, "Radial", axis_color(radial.axis));
    tip(on, tips, "Repeats every stroke evenly around the axis below, through the model's centre. Combines with the mirror axes.");

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
                _ => format!("Repeats strokes around the model's {} axis.", axis.name()),
            };
            if tip(theme::button(ui, w, axis.name(), style), tips, &help).clicked() {
                radial.axis = axis;
            }
        }
    });
    if radial.axis == RadialAxis::Custom {
        custom_axis_row(ui, radial, tips, down_the_view);
    }
}

/// The custom axis as text, with From View beside it.
fn custom_axis_row(ui: &mut Ui, radial: &mut Radial, tips: bool, down_the_view: Vec3) {
    const BUTTON: f32 = 84.0;
    ui.horizontal(|ui| {
        // The text being typed lives here while the box has the keyboard;
        // the rest of the time the box shows the axis itself.
        let id = ui.id().with("radial_custom_axis");
        let shown = format_axis(radial.custom_axis);
        let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| shown.clone());
        let field = theme::text_field(ui, &mut text, ui.available_width() - BUTTON - 6.0);
        // Only what was typed is taken, so a click in and out does not
        // round an axis From View set more finely.
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
        let help = "Points the custom axis straight down the current view, through the model's centre: seen from here, \
                    the repeats land evenly around it.";
        if tip(theme::button(ui, BUTTON, "From View", ButtonStyle::default()), tips, help).clicked() {
            radial.set_custom_axis(down_the_view);
        }
    });
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
