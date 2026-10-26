//! The panel while the tool is up: the controls of the Unity app's Array card
//! (`ArrayUIBuilder.cs`), with the same ranges, defaults and wording. Only the
//! controls that mean something for the kind of array the selected object has
//! are shown.
//!
//! Every change is one step of the tool's own history; sliders join a run of
//! ticks into one. The size sliders work on a log scale, so halving and
//! doubling are the same distance either side of "same size", and they snap to
//! it near the middle.

use egui::Color32;
use sculpt_toolkit::Ctx;
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};

use crate::finish::Apply;
use crate::layout;
use crate::state::ArrayState;
use crate::tool::ArrayTool;
use crate::types::{ArrayKind, ArraySettings};

pub const ACCENT_ARRAY: Color32 = Color32::from_rgb(92, 204, 255);
/// How near the middle of a size slider counts as "same size".
const SNAP_TO_EVEN: f32 = 0.035;
/// The smallest size the sliders reach (the settings go down to a twentieth).
const SLIDER_MIN_SIZE: f32 = 0.1;

fn slider_range() -> std::ops::RangeInclusive<f32> {
    SLIDER_MIN_SIZE.ln()..=ArraySettings::MAX_SCALE.ln()
}

fn to_slider(size: f32) -> f32 {
    size.max(0.01).ln()
}

fn from_slider(value: f32) -> f32 {
    if value.abs() < SNAP_TO_EVEN { 1.0 } else { value.exp() }
}

fn size_text(size: f32, below: &str, above: &str) -> String {
    if (size - 1.0).abs() < 1e-4 {
        "same size".to_owned()
    } else {
        format!("{size:.2}x ({})", if size < 1.0 { below } else { above })
    }
}

/// What the header says: "5 x Curve", or nothing.
pub fn summary(state: &ArrayState) -> String {
    if state.is_none() {
        return String::new();
    }
    let s = &state.settings;
    let kind = match s.kind {
        ArrayKind::Line => "Line",
        ArrayKind::Radial => "Circle",
        _ if s.closed => "Loop",
        _ => "Curve",
    };
    format!("{} x {kind}", s.count)
}

pub fn show(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx) {
    let object = tool.target(cx);
    let state = tool.arrays.state(object);
    theme::category(ui, "Array", ACCENT_ARRAY, &summary(&state), true, |ui| {
        let busy = tool.building.is_some();
        ui.add_enabled_ui(!busy, |ui| make_row(tool, ui, cx, &state));
        status_line(tool, ui, cx);
        ui.add_enabled_ui(!busy, |ui| {
            if !state.is_none() {
                settings(tool, ui, cx, &state);
            }
            done_row(tool, ui, cx, &state);
            if !state.is_none() {
                make_real(tool, ui, cx);
            }
        });
        theme::hint(
            ui,
            "An array stays with its object and is saved in the .sclp file, so it comes back live. Ctrl+Z steps it while \
             Array is up. Make it real to turn the copies into geometry.",
        );
    });

    theme::category(ui, "Array Shortcuts", Color32::from_rgb(120, 196, 230), "", false, |ui| {
        theme::shortcuts(
            ui,
            &[
                ("Drag a point", "Move it"),
                ("Square point", "Start of the path"),
                ("Click the path", "Add a point"),
                ("Shift + drag last", "Extend the path"),
                ("Right-click a point", "Delete it"),
                ("Delete", "Delete the picked"),
                ("Click a copy", "Turn or move them all"),
                ("Drag a copy along", "Space them all"),
                ("Click an object", "Switch to it"),
                ("Esc", "Cancel, stop, leave"),
                ("Ctrl + Z, Ctrl + Shift + Z", "Undo, redo"),
                ("Y", "Array on / off"),
                ("Drag on background", "Orbit"),
            ],
        );
    });
}

/// Line, Circle, Draw Curve.
fn make_row(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx, state: &ArrayState) {
    theme::heading(ui, "Repeat the selected object");
    let kind = state.settings.kind;
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        let active = |on: bool| ButtonStyle { active: on.then_some(ACCENT()), ..Default::default() };

        let r = theme::button(ui, w, "Line", active(kind == ArrayKind::Line));
        if theme::tip(r, cx.tips, "A straight row of copies. Drag the end point in the viewport to aim and stretch it.")
            .clicked()
        {
            tool.make_line(cx);
        }
        let r = theme::button(ui, w, "Circle", active(kind == ArrayKind::Radial));
        if theme::tip(r, cx.tips, "Copies in a ring round a centre point. Drag the centre to change the radius.")
            .clicked()
        {
            tool.make_circle(cx);
        }
        let r = theme::button(ui, w, "Draw Curve", active(tool.armed || kind == ArrayKind::Curve));
        let help = "Then drag in the viewport: the copies follow the curve you draw - a spine, a chain, a bracelet. \
                    End the stroke where it started to close it into a loop.";
        if theme::tip(r, cx.tips, help).clicked() {
            let armed = !tool.armed;
            tool.set_armed(cx, armed);
        }
    });
}

/// What just happened, for a few seconds; then what to do next.
fn status_line(tool: &ArrayTool, ui: &mut egui::Ui, cx: &Ctx) {
    if let Some(building) = &tool.building {
        theme::line(ui, &format!("Making the copies real: {}", building.progress()), theme::OK);
    } else if cx.time < tool.note.until && !tool.note.text.is_empty() {
        theme::line(ui, &tool.note.text, if tool.note.ok { theme::OK } else { theme::ERROR });
    } else {
        theme::hint(ui, tool.hint(cx));
    }
}

/// Count, sizes, and the knobs of the kind of array there is.
fn settings(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx, state: &ArrayState) {
    let s = state.settings;

    let mut count = s.count as f32;
    let r = theme::slider(
        ui,
        "Copies",
        &mut count,
        ArraySettings::MIN_COUNT as f32..=ArraySettings::MAX_COUNT as f32,
        |v| format!("{}", v.round() as i32),
    );
    let help = "How many times the object is drawn, counting the object itself. They are live copies: sculpt any \
                one and all of them change.";
    if theme::tip(r, cx.tips, help).changed() {
        let n = (count.round() as i32).clamp(ArraySettings::MIN_COUNT, ArraySettings::MAX_COUNT);
        if n != s.count {
            tool.edit(cx, "Array Count", true, |a| a.settings.count = n);
        }
    }

    let help = "Size of the last copy. Left tapers the run toward its end, right grows it, the middle keeps every \
                copy the same size.";
    if let Some(v) = size_slider(ui, cx.tips, "End Size", s.end_scale, ("tapers", "grows"), help) {
        tool.edit(cx, "Array End Size", true, |a| a.settings.end_scale = v);
    }
    let help = "Swells (right) or pinches (left) the middle of the run, leaving both ends alone - a spine thickest \
                at the ribs.";
    if let Some(v) = size_slider(ui, cx.tips, "Middle Size", s.mid_scale, ("pinched", "swollen"), help) {
        tool.edit(cx, "Array Middle Size", true, |a| a.settings.mid_scale = v);
    }

    let even = (s.end_scale - 1.0).abs() < 1e-4 && (s.mid_scale - 1.0).abs() < 1e-4;
    ui.add_enabled_ui(!even, |ui| {
        let r = theme::button(ui, ui.available_width(), "All the Same Size", ButtonStyle::default());
        if theme::tip(r, cx.tips, "Puts End Size and Middle Size back to 1: no taper.").clicked() {
            tool.edit(cx, "Array Sizes", false, |a| {
                a.settings.end_scale = 1.0;
                a.settings.mid_scale = 1.0;
            });
        }
    });

    let help =
        "On: each copy turns with the path (or round the circle). Off: every copy faces the way the object does.";
    if let Some(on) = toggle(ui, cx.tips, "Copies Turn to Follow", s.follow, help) {
        tool.edit(cx, "Array Follow", false, |a| a.settings.follow = on);
    }
    if s.is_path() {
        let help = "On: smaller copies sit closer together, so a tapered run stays evenly packed. Off: evenly spaced \
                    whatever their size.";
        if let Some(on) = toggle(ui, cx.tips, "Space by Size", s.space_by_size, help) {
            tool.edit(cx, "Array Spacing", false, |a| a.settings.space_by_size = on);
        }
        spacing(tool, ui, cx, state);
    }
    if s.kind == ArrayKind::Curve {
        let help = "Joins the end of the curve back to its start and spreads the copies round the whole loop - a \
                    bracelet, a collar.";
        if let Some(on) = toggle(ui, cx.tips, "Closed Loop", s.closed, help) {
            let label = if on { "Close Array Curve" } else { "Open Array Curve" };
            tool.edit(cx, label, false, |a| a.settings.closed = on);
        }
    }
    if s.kind == ArrayKind::Radial {
        circle_controls(tool, ui, cx, &s);
    }
    mirror_row(tool, ui, cx);
    copies_back(tool, ui, cx);
}

/// How far apart the copies of a line or curve are, against the gap that
/// spreads them along the whole path: the middle is that, left packs them
/// closer, right spaces them out (running on past the path's end). On the same
/// log scale as the sizes, and snapping to the middle the same way.
fn spacing(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx, state: &ArrayState) {
    let s = state.settings;
    let fits = ArraySettings { spacing: 0.0, ..s };
    let Some(fit) = layout::spacing(&fits, &state.points, 1).map(|at| at.gap).filter(|&gap| gap > 1e-6) else {
        return;
    };
    let ratio = if s.has_spacing() { s.spacing / fit } else { 1.0 };
    let mut value = to_slider(ratio);
    let r = theme::slider(ui, "Gap Between Copies", &mut value, SLIDER_MIN_SIZE.ln()..=ArraySettings::MAX_SCALE.ln(), |v| {
        let ratio = from_slider(v);
        if ratio == 1.0 {
            "fills the path".to_owned()
        } else {
            format!("{ratio:.2}x ({})", if ratio < 1.0 { "closer" } else { "further apart" })
        }
    });
    let help = "Spaces every copy out or packs them closer, so links can slot into each other. The middle spreads                 them along the whole path; anywhere else they keep their gap when you change Copies, and carry on                 straight past the end of the path. Dragging a copy along the path with the gizmo does the same.";
    if theme::tip(r, cx.tips, help).changed() {
        let ratio = from_slider(value);
        let new = if ratio == 1.0 { 0.0 } else { (ratio * fit).min(ArraySettings::MAX_SPACING) };
        if (new - s.spacing).abs() > 1e-6 * fit {
            tool.edit(cx, "Array Gap", true, |a| a.settings.spacing = new);
        }
    }
    ui.add_enabled_ui(s.has_spacing(), |ui| {
        let r = theme::button(ui, ui.available_width(), "Spread Along the Path", ButtonStyle::default());
        let help = "Puts the gap back to what spreads the copies evenly from the start of the path to its end.";
        if theme::tip(r, cx.tips, help).clicked() {
            tool.edit(cx, "Array Gap", false, |a| a.settings.spacing = 0.0);
        }
    });
}

/// Mirrors the array across the world's planes: the object's own live mirror,
/// the one the Mirror section of the Objects card edits, within reach.
fn mirror_row(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx) {
    theme::heading(ui, "Mirror to the other side");
    let axes = tool.mirror_axes(cx);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
            let style = ButtonStyle { active: (axes >> axis & 1 != 0).then_some(ACCENT()), ..Default::default() };
            let help = format!(
                "Repeats the object and every copy on the other side of the scene's centre along world {name} - a \
                 strap down one side is on the other too. This is the object's own Mirror, so it stays live."
            );
            if theme::tip(theme::button(ui, w, name, style), cx.tips, &help).clicked() {
                tool.toggle_mirror(cx, axis);
            }
        }
    });
}

/// Undoes any move made by clicking a copy and dragging the gizmo's arrows.
fn copies_back(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx) {
    ui.add_enabled_ui(tool.has_copy_offset(cx), |ui| {
        let r = theme::button(ui, ui.available_width(), "Copies Back onto the Path", ButtonStyle::default());
        let help = "Undoes any move made by clicking a copy and dragging the gizmo's arrows: every copy sits on the \
                    path again.";
        if theme::tip(r, cx.tips, help).clicked() {
            tool.copies_onto_path(cx);
        }
    });
}

/// A log-scale size slider that snaps to "same size". The new size when it
/// was changed.
fn size_slider(
    ui: &mut egui::Ui,
    tips: bool,
    caption: &str,
    size: f32,
    words: (&str, &str),
    help: &str,
) -> Option<f32> {
    let mut value = to_slider(size);
    let r = theme::slider(ui, caption, &mut value, slider_range(), |v| size_text(from_slider(v), words.0, words.1));
    if !theme::tip(r, tips, help).changed() {
        return None;
    }
    let new = from_slider(value).clamp(ArraySettings::MIN_SCALE, ArraySettings::MAX_SCALE);
    ((new - size).abs() > 1e-4).then_some(new)
}

/// A tick box. The new value when it was clicked.
fn toggle(ui: &mut egui::Ui, tips: bool, caption: &str, value: bool, help: &str) -> Option<bool> {
    let mut on = value;
    let r = theme::checkbox(ui, &mut on, caption, ACCENT());
    (theme::tip(r, tips, help).changed() && on != value).then_some(on)
}

/// The axis the copies go round, and how much of the circle they cover.
fn circle_controls(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx, s: &ArraySettings) {
    theme::heading(ui, "Go round the world axis");
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
            let style = ButtonStyle { active: (s.axis == axis as i32).then_some(ACCENT()), ..Default::default() };
            let help = format!("The copies go round the world {name} axis, through the centre point.");
            if theme::tip(theme::button(ui, w, name, style), cx.tips, &help).clicked() && s.axis != axis as i32 {
                tool.edit(cx, "Array Axis", false, |a| a.set_axis(axis as i32));
            }
        }
    });

    // The slider is the size of the sweep; which way round it goes is the box under it.
    let reversed = s.sweep < 0.0;
    let mut sweep = s.sweep.abs();
    let r = theme::slider(ui, "Sweep", &mut sweep, 10.0..=360.0, |v| {
        if v >= 357.0 { "full circle".to_owned() } else { format!("{} degrees", v.round() as i32) }
    });
    let help = "How much of the circle the copies cover. All the way right is the full circle; less makes an arc - a \
                fan, a crest.";
    if theme::tip(r, cx.tips, help).changed() {
        let size = if sweep >= 357.0 { 360.0 } else { sweep.round() };
        let new = if reversed { -size } else { size };
        if (new - s.sweep).abs() > 1e-3 {
            tool.edit(cx, "Array Sweep", true, |a| a.settings.sweep = new);
        }
    }
    let help = "Go round the other way: the copy after the object is on its other side. A circle opened from the Unity \
                app that goes round Y or Z is set this way, so that every copy stays where it was.";
    if let Some(on) = toggle(ui, cx.tips, "Other way round", reversed, help) {
        let (size, sign) = (s.sweep.abs(), if on { -1.0 } else { 1.0 });
        tool.edit(cx, "Array Direction", false, |a| a.settings.sweep = sign * size);
    }
}

/// Back to Sculpt, and Delete Point.
fn done_row(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx, state: &ArrayState) {
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let style = ButtonStyle { active: Some(ACCENT()), badge: "Y", ..Default::default() };
        let r = theme::button(ui, w, "Done", style);
        let help = "Put the tool away and sculpt. The copies stay on screen and follow the object; press Y to edit the \
                    path again.";
        if theme::tip(r, cx.tips, help).clicked() {
            cx.finish();
        }
        let deletable = tool.picked.is_some_and(|p| p > 0) && state.settings.is_path() && state.points.len() > 2;
        ui.add_enabled_ui(deletable, |ui| {
            let r = theme::button(ui, w, "Delete Point", ButtonStyle::default());
            let help = "Removes the picked path point (Delete, or right-click a point).";
            let clicked = theme::tip(r, cx.tips, help).clicked();
            if let (true, Some(index)) = (clicked, tool.picked) {
                tool.delete_point(cx, index);
            }
        });
    });
}

/// One Object, Separate Objects, Remove Array.
fn make_real(tool: &mut ArrayTool, ui: &mut egui::Ui, cx: &mut Ctx) {
    theme::heading(ui, "Make it real");
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let r = theme::button(ui, w, "One Object", ButtonStyle::default());
        let help = "Turns the copies into real geometry inside the selected object - ready to Remesh into one fused \
                    piece. Undo (Ctrl+Z) gives the live array back.";
        if theme::tip(r, cx.tips, help).clicked() {
            tool.apply(cx, Apply::One);
        }
        let r = theme::button(ui, w, "Separate Objects", ButtonStyle::default());
        let help = "Turns every copy into its own independent object, so each can be sculpted differently. Undo (Ctrl+Z) \
                    gives the live array back.";
        if theme::tip(r, cx.tips, help).clicked() {
            tool.apply(cx, Apply::Separate);
        }
    });
    let r = theme::button(ui, ui.available_width(), "Remove Array", ButtonStyle::default());
    if theme::tip(r, cx.tips, "Takes the copies away and leaves the object as it is. Undo (Ctrl+Z) brings them back.")
        .clicked()
    {
        tool.remove(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ArraySettings;

    #[test]
    fn the_size_slider_snaps_to_the_middle_and_is_logarithmic() {
        assert_eq!(from_slider(0.0), 1.0);
        assert_eq!(from_slider(0.03), 1.0);
        assert!((from_slider(0.05) - 0.05f32.exp()).abs() < 1e-6);
        // Half and double are the same distance either side.
        assert!((to_slider(0.5) + to_slider(2.0)).abs() < 1e-6);
        let range = slider_range();
        assert!((from_slider(*range.start()) - 0.1).abs() < 1e-5);
        assert!((from_slider(*range.end()) - ArraySettings::MAX_SCALE).abs() < 1e-4);
    }

    #[test]
    fn the_size_readout_says_tapers_or_grows() {
        assert_eq!(size_text(1.0, "tapers", "grows"), "same size");
        assert_eq!(size_text(0.5, "tapers", "grows"), "0.50x (tapers)");
        assert_eq!(size_text(2.0, "pinched", "swollen"), "2.00x (swollen)");
    }

    #[test]
    fn the_header_names_the_array() {
        assert_eq!(summary(&ArrayState::none()), "");
        let mut state = ArrayState { settings: ArraySettings::new(ArrayKind::Curve), points: vec![] };
        assert_eq!(summary(&state), "5 x Curve");
        state.settings.closed = true;
        assert_eq!(summary(&state), "5 x Loop");
        state.settings = ArraySettings::new(ArrayKind::Radial);
        assert_eq!(summary(&state), "8 x Circle");
        state.settings = ArraySettings::new(ArrayKind::Line);
        assert_eq!(summary(&state), "5 x Line");
    }
}
