//! The SSpheres side of the left panel: the tool switch, the edit modes, the
//! rig's status and undo lines, attaching the rig to an object, and the skin
//! settings with Make Skin. Also the tool's part of the hold-Space wheel.

use egui::{Align2, Color32, Sense, vec2};

use super::theme::{self, ACCENT, ButtonStyle};
use super::{UiState, fmt_count, tip};
use crate::doc::{Action, Doc, Tool};
use crate::ssphere::{Body, EditMode, MIN_NODE_RADIUS, skin};

/// Largest radius the size sliders reach; the wheel and Scale mode go further.
const SLIDER_MAX_RADIUS: f32 = 2.0;

fn line(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(11.0)).color(color)).wrap());
}

/// Sculpt / SSpheres: what the pointer does in the viewport.
pub fn tool_row(ui: &mut egui::Ui, doc: &Doc, state: &UiState, actions: &mut Vec<Action>) {
    let tools = [
        (Tool::Sculpt, "Sculpt", "tool_sculpt", "Shape the active object with brushes."),
        (
            Tool::Transform,
            "Transform",
            "tool_transform",
            "Move, turn and scale whole objects: drag an arrow, a ring or a cube (G).",
        ),
        (
            Tool::SSpheres,
            "SSpheres",
            "tool_sspheres",
            "Block out a form with connected spheres, then turn it into a new object to sculpt.",
        ),
    ];
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, tools.len(), 6.0);
        for (tool, label, icon, help) in tools {
            let style = ButtonStyle {
                icon: state.assets.icon(icon),
                active: (doc.tool == tool).then_some(ACCENT()),
                ..Default::default()
            };
            if tip(theme::button(ui, w, label, style), doc.opts.tooltips, help).clicked() {
                actions.push(Action::Tool(tool));
            }
        }
    });
}

/// A slider over a radius where equal travel is an equal proportional change.
fn radius_slider(ui: &mut egui::Ui, label: &str, radius: f32) -> Option<f32> {
    let mut log = radius.log2();
    let range = MIN_NODE_RADIUS.log2()..=SLIDER_MAX_RADIUS.log2();
    theme::slider(ui, label, &mut log, range, |v| format!("{:.3}", v.exp2())).changed().then(|| log.exp2())
}

/// New SSphere Rig, for wherever a model can be started from: brings the
/// tool up with a first sphere already in the middle of the view.
pub fn new_rig_button(ui: &mut egui::Ui, icon: Option<egui::TextureId>, tips: bool, actions: &mut Vec<Action>) {
    let style = ButtonStyle { icon, ..Default::default() };
    let r = theme::button(ui, ui.available_width(), "New SSphere Rig", style);
    if tip(
        r,
        tips,
        "Starts a skeleton of spheres with one in the middle of the view. Pull more out of it, pose them, then turn them into an object to sculpt.",
    )
    .clicked()
    {
        actions.push(Action::NewRig);
    }
}

pub fn panel(ui: &mut egui::Ui, doc: &mut Doc, state: &UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let scene = &doc.scene;
    let s = &mut doc.spheres;
    let count = s.rig.alive_count();
    let shown = s.skin().map(|(skin, _)| skin.stats);

    theme::category(ui, "Spheres", Color32::from_rgb(255, 153, 46), s.mode().name(), true, |ui| {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 4, 4.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            for mode in EditMode::ALL {
                let active = (s.mode() == mode).then_some(ACCENT());
                let badge = mode.hotkey().to_string();
                let r = theme::tile(ui, vec2(w, 46.0), state.assets.icon(mode.icon()), mode.name(), &badge, active);
                let text = format!("{} ({})\n{}", mode.name(), mode.hotkey(), mode.description());
                if tip(r, tips, &text).clicked() {
                    s.set_mode(mode);
                }
            }
        });
        theme::hint(ui, s.mode().description());

        // One-off results first, then what the rig is doing.
        if let Some(note) = s.note() {
            line(ui, note, theme::OK);
        } else if count == 0 {
            line(ui, "Click in the viewport to place the first sphere. Drag to size it.", theme::HINT());
        } else if let Some(error) = s.skin_error() {
            line(ui, error, theme::ERROR);
        } else if let Some(stats) = shown {
            let draft = if s.skin_is_draft() { " (live draft)" } else { "" };
            let text = format!("{count} spheres   skin {} tris{draft}", fmt_count(stats.triangles));
            line(ui, &text, theme::OK);
        } else {
            line(ui, &format!("{count} spheres."), theme::HINT());
        }
        match s.undo_summary() {
            Some((label, steps)) => line(ui, &format!("Undo (Z): {label}. {steps} step(s) held."), theme::HINT()),
            None => line(ui, "Sphere history empty.", theme::HINT()),
        }
        if count == 0 {
            new_rig_button(ui, state.assets.icon("tool_sspheres"), tips, actions);
        }

        if let Some(radius) = s.selected_radius() {
            let r = radius_slider(ui, "Selected Sphere Size", radius);
            if let Some(radius) = r {
                s.set_selected_radius(radius);
            }
        }
        if let Some(radius) = radius_slider(ui, "Next Sphere Size", s.next_radius()) {
            s.set_next_radius(radius);
        }
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(s.selected.is_some(), |ui| {
                let style = ButtonStyle { badge: "Del", ..Default::default() };
                let r = theme::button(ui, w, "Delete Sphere", style);
                if tip(r, tips, "Deletes the selected sphere and everything below it. Right-click does the same.")
                    .clicked()
                {
                    s.delete_selected();
                }
            });
            ui.add_enabled_ui(count > 0, |ui| {
                let r = theme::button(ui, w, "Clear All", ButtonStyle::default());
                if tip(r, tips, "Removes every sphere. Undo (Z) brings them back.").clicked() {
                    s.clear();
                    s.say(format!("Cleared {count} spheres. Undo (Z) brings them back."));
                }
            });
        });
    });

    let summary = if s.symmetry() { "X" } else { "Off" };
    theme::category(ui, "Symmetry", Color32::from_rgb(242, 115, 158), summary, true, |ui| {
        let mut on = s.symmetry();
        let r = theme::checkbox(ui, &mut on, "Mirror Across X", theme::AXIS[0]);
        if tip(
            r,
            tips,
            "Mirrors the whole rig live across the red plane (X). Turning it off keeps both halves as real spheres you can edit separately.",
        )
        .changed()
        {
            let report = s.set_symmetry(on);
            s.say(report);
        }
        theme::hint(ui, "A sphere on the red plane is shared by both sides; one off it gets a twin.");
    });

    // The object the rig follows, if it is still in the scene.
    let attached = s.attached_to().and_then(|id| scene.get(id));
    let summary = attached.map_or("Free", |o| o.name.as_str());
    theme::category(ui, "Attach", Color32::from_rgb(102, 204, 230), summary, true, |ui| {
        match attached {
            Some(o) => {
                let text = format!("Attached to \"{}\". The spheres follow it and mirror across its plane.", o.name);
                line(ui, &text, theme::OK);
            }
            None => line(ui, "Not attached. A first sphere placed on a model attaches to that model.", theme::HINT()),
        }
        let selected = scene.active();
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let r = theme::button(ui, w, "Attach to Selected", ButtonStyle::default());
            if tip(
                r,
                tips,
                "Ties the spheres to the selected object: they follow it when it is moved, turned or resized, and mirror across its plane. No sphere moves.",
            )
            .clicked()
            {
                let already = s.attached_to() == Some(selected.id);
                let plane_moved = s.attach(Body::of(selected));
                let name = &selected.name;
                s.say(if already && !plane_moved {
                    format!("Already attached to \"{name}\".")
                } else if count == 0 {
                    format!("Attached to \"{name}\". The spheres will follow it and mirror across its plane.")
                } else if plane_moved && s.symmetry() {
                    format!(
                        "Attached to \"{name}\". No sphere moved, but the mirror plane is now that object's, so the mirrored half has. Undo (Z) puts it back."
                    )
                } else {
                    format!("Attached to \"{name}\". No sphere moved; they follow it from now on.")
                });
            }
            ui.add_enabled_ui(attached.is_some(), |ui| {
                let r = theme::button(ui, w, "Detach", ButtonStyle::default());
                if tip(r, tips, "Lets go of the object. The spheres stay where they are.").clicked() && s.detach() {
                    s.say("Detached. The spheres stay where they are.");
                }
            });
        });
        ui.add_enabled_ui(count > 0, |ui| {
            let r = theme::button(ui, ui.available_width(), "Re-centre Mirror Plane", ButtonStyle::default());
            if tip(
                r,
                tips,
                "Moves the red mirror plane onto the attached object (or the selected one) without moving any sphere.",
            )
            .clicked()
            {
                let onto = attached.unwrap_or(selected);
                let moved = s.recentre(Body::of(onto));
                let name = &onto.name;
                s.say(if !moved {
                    format!("The mirror plane is already on \"{name}\".")
                } else if s.symmetry() {
                    format!("Mirror plane moved onto \"{name}\". No sphere moved; the mirrored half now mirrors across it.")
                } else {
                    format!("Mirror plane moved onto \"{name}\". No sphere moved.")
                });
            }
        });
        theme::hint(
            ui,
            "An attached rig goes with its object when that is moved, turned or resized evenly. Stretching the object along one axis leaves the spheres as they are.",
        );
    });

    let summary = shown.map_or(String::new(), |stats| fmt_count(stats.triangles));
    theme::category(ui, "Skin", Color32::from_rgb(115, 217, 128), &summary, true, |ui| {
        tip(
            theme::checkbox(ui, &mut s.show_skin, "Show Skin", ACCENT()),
            tips,
            "Shows the surface the spheres make as a see-through shell that updates while you edit.",
        );
        tip(
            theme::checkbox(ui, &mut s.preview, "Solid Preview", ACCENT()),
            tips,
            "Shows the solid skin with the spheres hidden, to judge the final shape (A). Limbs can still be dragged.",
        );
        let k = &mut s.settings;
        tip(
            theme::slider(ui, "Skin Detail", &mut k.density, skin::MIN_DENSITY..=skin::MAX_DENSITY, |v| {
                format!("{v:.2}x")
            }),
            tips,
            "How many surface points the skin gets. 1 is always fine enough for the thinnest limb; higher is smoother but heavier to sculpt.",
        );
        tip(
            theme::slider(ui, "Joint Blend", &mut k.blend, 0.0..=1.0, |v| format!("{v:.2}")),
            tips,
            "How softly branches melt into the limb they grow from, and how much the inside of a bend is filled.",
        );
        tip(
            theme::slider(ui, "Chain Curve", &mut k.curve, 0.0..=1.0, |v| format!("{v:.2}")),
            tips,
            "How much chains flow in a curve between their spheres. 0 is straight links; sharp joints always stay straight with a rounded joint.",
        );
        let mut smoothing = k.smoothing as f32;
        if tip(
            theme::slider(ui, "Skin Smoothing", &mut smoothing, 0.0..=8.0, |v| format!("{:.0}", v.round())),
            tips,
            "Evens out the surface before it is snapped onto the exact smooth shape.",
        )
        .changed()
        {
            k.smoothing = smoothing.round() as u32;
        }

        theme::heading(ui, "Make It a Model");
        if let Some(tris) = s.final_triangles() {
            line(ui, &format!("About {} triangles at this detail.", fmt_count(tris)), theme::HINT());
        }
        tip(
            theme::checkbox(ui, &mut s.keep_rig, "Keep Spheres Afterwards", ACCENT()),
            tips,
            "Leaves the spheres in place, so you can change them later and make a fresh skin.",
        );
        tip(
            theme::checkbox(ui, &mut s.snap_to_surface, "First Sphere Rests on the Model", ACCENT()),
            tips,
            "The first sphere of a rig sits on the model under the cursor, touching it. Later spheres are free.",
        );
        ui.add_enabled_ui(count > 0, |ui| {
            let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
            if tip(
                theme::button(ui, ui.available_width(), "Make Skin", style),
                tips,
                "Wraps a skin over the spheres and adds it to the scene as a new object you can sculpt.",
            )
            .clicked()
            {
                actions.push(Action::MakeSkin);
            }
        });
        theme::hint(
            ui,
            "The skin does not follow the spheres afterwards: change them and you need a new skin, so settle the overall shape before sculpting detail.",
        );
    });

    theme::category(ui, "SSphere Shortcuts", Color32::from_rgb(230, 204, 102), "", false, |ui| {
        let rows: &[(&str, &str)] = &[
            ("1 - 4", "Draw, Move, Scale, Rotate"),
            ("Drag off a sphere", "Pull out a new one"),
            ("Click empty space", "Add to the selected"),
            ("Drag a link", "Add a joint / pose"),
            ("Scroll on a sphere", "Resize it"),
            ("S + drag", "Size of the next sphere"),
            ("Right-click sphere", "Delete it and its branch"),
            ("Ctrl + click sphere", "Delete it and its branch"),
            ("Delete", "Delete the selected"),
            ("Shift + drag", "One sphere / whole branch"),
            ("Esc", "Cancel a drag, deselect"),
            ("A", "Solid preview"),
            ("X", "Symmetry"),
            ("Space (hold)", "SSphere menu"),
            ("Z / Shift+Z", "Undo / redo"),
            ("Drag on background", "Orbit"),
            ("F F", "Frame the spheres"),
        ];
        for (key, what) in rows {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::hover());
            ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, key, theme::font(11.0), theme::KEY);
            ui.painter().text(
                rect.left_center() + vec2(118.0, 0.0),
                Align2::LEFT_CENTER,
                what,
                theme::font(11.0),
                theme::TEXT(),
            );
        }
    });
}

/// The hold-Space wheel while the tool is up: name, icon, lit.
pub fn radial_tiles(doc: &Doc) -> Vec<(&'static str, &'static str, bool)> {
    let s = &doc.spheres;
    let mut tiles: Vec<_> = EditMode::ALL.iter().map(|&m| (m.name(), m.icon(), s.mode() == m)).collect();
    tiles.push(("Symmetry", "mirror", s.symmetry()));
    tiles.push(("Preview", "eye", s.preview));
    tiles
}

/// Acts on a click on tile `index` of [`radial_tiles`].
pub fn radial_pick(doc: &mut Doc, index: usize) {
    let s = &mut doc.spheres;
    match EditMode::ALL.get(index) {
        Some(&mode) => s.set_mode(mode),
        None if index == EditMode::ALL.len() => {
            let report = s.set_symmetry(!s.symmetry());
            s.say(report);
        }
        None => s.preview = !s.preview,
    }
}

/// What the wheel's hub shows when no tile is hovered.
pub fn radial_current(doc: &Doc) -> &'static str {
    doc.spheres.mode().name()
}

/// The Next Sphere Size slider that goes with the wheel: the size of the
/// sphere the next click or pull will make, so it can be set without a trip
/// to the panel. `center` is the wheel's centre, `radius` its outer radius
/// and `open` how far it has faded in (0 to 1).
pub fn radial_slider(ctx: &egui::Context, doc: &mut Doc, center: egui::Pos2, radius: f32, open: f32) {
    let s = &mut doc.spheres;
    // As `radius_slider`: equal travel is an equal proportional change.
    let mut log = s.next_radius().log2();
    let range = MIN_NODE_RADIUS.log2()..=SLIDER_MAX_RADIUS.log2();
    let readout = |v: f32| format!("{:.3}", v.exp2());
    let wheel = (center, radius, open);
    if super::overlay::wheel_bar(ctx, "radial_sphere_size", wheel, "Next Sphere Size", &mut log, range, readout) {
        s.set_next_radius(log.exp2());
    }
}
