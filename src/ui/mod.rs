//! The interface: two floating panels over a full-window viewport, a view
//! gizmo, the brush cursor and the radial brush menu. Wording follows the
//! Unity app. Each panel shows a tab's worth of its cards at a time (see
//! `tabs`), and either folds away to a tab at the screen's edge (see
//! `panels`); what it alone would have said is then said over the view (see
//! `status`). The colours are a theme the user picks (see `settings`).

mod alpha;
pub mod alpha_preview;
pub mod assets;
mod brush_size;
mod brushes;
mod dyntopo;
mod falloff;
pub mod groups;
mod layers;
mod levels;
mod look;
mod masking;
mod mirror_copies;
mod modes;
mod overlay;
mod panels;
mod pose;
pub mod question;
mod scene_graph;
mod settings;
mod spheres;
pub mod status;
mod stroke;
mod symmetry;
pub mod tabs;
pub mod theme;
mod tools;

use egui::{Align2, Color32, Id, Order, Pos2, Sense, Vec2, pos2, vec2};

use crate::doc::{Action, Doc, MaskOp, Tool};
use crate::sculpt::Brush;
use assets::Assets;
pub use overlay::{Cursor, brush_wheel_ring, brush_wheel_tile};
/// The Brush Library card's controls, for the interface's self-check.
pub mod library_card {
    pub use super::brushes::{place, watch};
}

/// Cursor ring colours, for the code that decides which one applies.
pub mod overlay_colors {
    pub use super::overlay::{ADD, SMOOTH, SUBTRACT};
}
use panels::{Part, Side};
use theme::{ButtonStyle, ACCENT};

pub struct UiState {
    pub assets: Assets,
    /// Message, colour and the time it appeared.
    pub toast: Option<(String, Color32, f64)>,
    pub status: (String, Color32),
    /// When `status` was written, and whether its copy over the view (drawn
    /// while the Scene panel is folded away) has been tapped away since.
    pub status_seen: status::Seen,
    /// Where the radial brush menu is open (points), if it is.
    pub radial: Option<Pos2>,
    /// The object list: rename, drag-to-reorder and its text fields.
    pub graph: scene_graph::State,
    /// The transform gizmo as the app laid it out for this frame, in window
    /// pixels. Empty when it is not up.
    pub gizmo: Vec<crate::gizmo::Shape>,
    /// The recent files and their tiles in the File card.
    pub recent: crate::recent::Recent,
    /// The Pose brush's chains as the app laid them out for this frame, in
    /// the interface's points: the grabbed point, then each joint. Empty
    /// when there is nothing to show.
    pub pose: Vec<Vec<Pos2>>,
}

impl UiState {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            assets: Assets::load(ctx),
            toast: None,
            status: ("Import adds a model (.obj / .stl).".into(), theme::HINT()),
            status_seen: Default::default(),
            radial: None,
            graph: Default::default(),
            gizmo: Vec::new(),
            recent: Default::default(),
            pose: Vec::new(),
        }
    }
}

fn fmt_count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.0}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

fn tip(response: egui::Response, on: bool, text: &str) -> egui::Response {
    if on { response.on_hover_text(text) } else { response }
}

/// Builds the whole interface for one frame. Returns the commands the user
/// triggered; the app runs them after the UI pass.
pub fn build(
    ui: &mut egui::Ui,
    doc: &mut Doc,
    state: &mut UiState,
    cursor: &Cursor,
    time: f64,
) -> Vec<Action> {
    let ctx = ui.ctx().clone();
    let mut actions = Vec::new();
    let screen = ctx.content_rect();
    settings::apply_theme(&ctx, &mut doc.opts);

    overlay::transform_gizmo(&ctx, &state.gizmo);
    overlay::cursor(&ctx, cursor);
    pose::overlay(&ctx, &state.pose);
    tools::overlay(&ctx, doc);

    if !doc.opts.clean_view {
        // Each panel, unless it is folded away, and the tab that folds it.
        let (tips, mut folded) = (doc.opts.tooltips, doc.opts.folded);
        sculpt_toolkit::floating::set_room(&ctx, panels::room_for_cards(screen, folded));
        panels::panel(ui, Side::Left, folded[0], "", &mut |ui, part| {
            if part == Part::Body {
                sculpt_panel(ui, doc, state, &mut actions);
            }
        });
        // Each tab of the Scene panel keeps its own place in its scroll.
        let page = doc.opts.tabs.scene.name();
        panels::panel(ui, Side::Right, folded[1], page, &mut |ui, part| match part {
            Part::Head => scene_head(ui, doc, state),
            Part::Body => scene_panel(ui, doc, state, &mut actions),
        });
        for side in [Side::Left, Side::Right] {
            let away = &mut folded[side.index()];
            *away ^= panels::tab(&ctx, side, *away, tips);
        }

        overlay::view_gizmo(&ctx, doc, panels::gizmo_origin(screen, folded[1]), &mut actions);

        // Bottom bar: the two ways of looking at the surface toggled constantly.
        egui::Area::new(Id::new("bottom_bar"))
            .order(Order::Middle)
            .fade_in(false)
            .fixed_pos(panels::bottom_bar_origin(screen, folded[0]))
            .show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    theme::checkbox(ui, &mut doc.opts.flat_shading, "Flat Shading", ACCENT());
                    theme::checkbox(ui, &mut doc.opts.wireframe, "Wireframe", ACCENT());
                    theme::checkbox(ui, &mut doc.opts.groups.colours, "Group Colours", ACCENT());
                });
            });
        doc.opts.folded = folded;
        // What a folded panel alone would have said: the status line first of all.
        status::overlay(&ctx, doc, state, folded, time);
    }

    overlay::radial_menu(&ctx, doc, state);
    tools::radial(&ctx, doc, state);
    tools::jobs(&ctx, doc);
    scene_graph::confirm(&ctx, doc, state, &mut actions);
    overlay::toast(&ctx, state, time);
    if doc.opts.clean_view {
        overlay::clean_hint(&ctx, crate::turntable::HINT, doc.turntable.hint_alpha(time));
        // No panels, so no status line: an error is said all the same.
        status::clean_view(&ctx, state, doc.opts.tooltips, time);
    }
    crate::turntable::recording_overlay(&ctx, doc);
    actions
}

fn sculpt_panel(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let icon = |name: &str| state.assets.icon(name);

    // Title and mesh size.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, "Sculpt", theme::bold(19.0), theme::TEXT());
    ui.painter().text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        format!(
            "{} tris   {} verts",
            fmt_count(doc.mesh.triangle_count()),
            fmt_count(doc.mesh.vertex_count())
        ),
        theme::font(11.0),
        theme::HINT(),
    );

    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        ui.add_enabled_ui(doc.can_undo(), |ui| {
            let style = ButtonStyle { icon: icon("undo"), badge: "Z", ..Default::default() };
            if tip(theme::button(ui, w, "Undo", style), tips, "Step back (Z or Ctrl+Z)").clicked() {
                actions.push(Action::Undo);
            }
        });
        ui.add_enabled_ui(doc.can_redo(), |ui| {
            let style = ButtonStyle { icon: icon("redo"), badge: "Shift+Z", ..Default::default() };
            if tip(theme::button(ui, w, "Redo", style), tips, "Step forward (Shift+Z or Ctrl+Y)").clicked() {
                actions.push(Action::Redo);
            }
        });
    });

    spheres::tool_row(ui, doc, state, actions);
    // The hosted tools: a button each, and the panel of the one that is up.
    tools::buttons(ui, doc, state);
    if tools::panel(ui, doc, state) {
        return;
    }
    if doc.tool == Tool::SSpheres {
        return spheres::panel(ui, doc, state, actions);
    }
    if doc.tool == Tool::Transform {
        let count = doc.scene.selected().len();
        let summary = if count > 1 { format!("{count} objects") } else { doc.scene.active().name.clone() };
        theme::category(ui, "Transform", theme::lit(ACCENT()), &summary, true, |ui| {
            theme::hint(ui, "Drag an arrow to move, a ring to turn, a cube on an arm to stretch that way, or the white cube to resize.");
            theme::hint(ui, "Click another object to select it; Shift or Ctrl + click adds it to the selection or takes it out. Several selected objects go together, about their shared centre. Drag anywhere else to orbit.");
            theme::hint(ui, "If part of the object is masked, the gizmo reshapes the rest and leaves the masked part where it is.");
            // Which way the arrows and rings point.
            theme::hint(ui, "Gizmo axes (arrows and rings)");
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 3, 6.0);
                for axes in crate::gizmo::Orientation::ALL {
                    let style = ButtonStyle { active: (doc.opts.gizmo_axes == axes).then_some(ACCENT()), ..Default::default() };
                    let help = format!("{} The scale cubes always follow the object's own axes.", axes.tip());
                    if tip(theme::button(ui, w, axes.name(), style), doc.opts.tooltips, &help).clicked() {
                        doc.opts.gizmo_axes = axes;
                    }
                }
            });
            let style = ButtonStyle { badge: "G", ..Default::default() };
            if tip(theme::button(ui, ui.available_width(), "Back to Sculpt", style), doc.opts.tooltips, "Put the gizmo away and sculpt (G).").clicked() {
                actions.push(Action::Tool(Tool::Sculpt));
            }
        });
    }

    let mut showing = doc.opts.tabs.sculpt;
    tabs::sculpt_strip(ui, &mut showing, tips);
    if tabs::page(ui, showing == tabs::Sculpt::Brush, "brush", |ui| brush_page(ui, doc, state, actions)) {
        showing = tabs::Sculpt::Brush;
    }
    if tabs::page(ui, showing == tabs::Sculpt::Mesh, "mesh", |ui| mesh_page(ui, doc, state, actions)) {
        showing = tabs::Sculpt::Mesh;
    }
    doc.opts.tabs.sculpt = showing;
}

/// The Sculpt panel's Brush tab: the brushes, the settings of the one in
/// hand, and the stroke.
fn brush_page(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let icon = |name: &str| state.assets.icon(name);
    let (library, s) = (&mut doc.sculptor.library, &mut doc.sculptor.settings);
    let brush = s.brush;
    // The brush in hand is one of the library's, over that kernel.
    let in_hand = library.selected(s);
    let name = library.entries()[in_hand].name.clone();
    let summary = if s.mask_mode { "Mask" } else { name.as_str() };
    theme::category(ui, "Brushes", theme::lit(ACCENT()), summary, true, |ui| {
        // However many brushes there are, in rows of six.
        const PER_ROW: usize = 6;
        let w = theme::row_width(ui, PER_ROW, 4.0);
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for row in Brush::ALL.chunks(PER_ROW) {
            ui.horizontal(|ui| {
                for &b in row {
                    // The thirteen base brushes; the rest are in the card below.
                    let base = crate::sculpt::library::Library::base(b);
                    let active = (base == in_hand && !s.mask_mode).then_some(ACCENT());
                    // A brush with no number key has no badge.
                    let badge = b.hotkey().map(String::from).unwrap_or_default();
                    let r = theme::tile(ui, vec2(w, 40.0), icon(b.icon()), "", &badge, active);
                    // A brush of the library in hand: its kernel's tile is
                    // ringed, so the grid still shows what kind it is.
                    if b == brush && base != in_hand && !s.mask_mode {
                        ui.painter().rect_stroke(r.rect, 5.0, egui::Stroke::new(1.5, ACCENT()), egui::StrokeKind::Inside);
                    }
                    let text = match b.hotkey() {
                        Some(key) => format!("{} ({key})\n{}", b.name(), b.description()),
                        None => format!("{}\n{}", b.name(), b.description()),
                    };
                    if tip(r, tips, &text).clicked() {
                        // Picking a brush always leaves mask mode, so the
                        // click never looks dead.
                        library.select(base, s);
                    }
                }
            });
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 4.0);
            let positive = s.current().positive;
            let polar = s.brush.has_polarity() && !s.mask_mode;
            let add = ButtonStyle {
                icon: icon("polarity_add"),
                active: (polar && positive).then_some(ACCENT()),
                ..Default::default()
            };
            if tip(theme::button(ui, w, "Add", add), tips, "Brush adds material. Hold Ctrl or use the right button to flip.").clicked() {
                s.current_mut().positive = true;
                s.mask_mode = false;
            }
            let sub = ButtonStyle {
                icon: icon("polarity_sub"),
                active: (polar && !positive).then_some(theme::SUB),
                ..Default::default()
            };
            if tip(theme::button(ui, w, "Sub", sub), tips, "Brush removes material.").clicked() {
                s.current_mut().positive = false;
                s.mask_mode = false;
            }
            let mask = ButtonStyle {
                icon: icon("brush_mask"),
                active: s.mask_mode.then_some(theme::MASK),
                badge: "M",
                ..Default::default()
            };
            if tip(theme::button(ui, w, "Mask", mask), tips, "Paint a mask that protects the surface from brushes (M). Ctrl erases.").clicked() {
                s.mask_mode = !s.mask_mode;
            }
        });
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        brush_size::slider(ui, s, tips);
        let strength = if brush == Brush::Move && !s.mask_mode {
            "How far the surface follows the pen: at 100% it stays under it, lower is a gentler pull. Hold F and drag."
        } else {
            "How hard the brush works. Hold F and drag."
        };
        let bs = s.current_mut();
        tip(
            theme::slider(ui, "Strength", &mut bs.strength, 0.01..=1.0, |v| format!("{:.0}%", v * 100.0)),
            tips,
            strength,
        );
        let mut focal = bs.focal_shift * 100.0;
        if tip(
            theme::slider(ui, "Focal Shift (Hard - Soft)", &mut focal, -100.0..=100.0, |v| format!("{v:.0}")),
            tips,
            "Moves the brush's weight toward its centre or its edge. Hold D and drag.",
        )
        .changed()
        {
            bs.focal_shift = focal / 100.0;
        }
        brush_size::toggle(ui, s, tips);
    });

    brushes::card(ui, library, s, &icon, tips, actions);

    // A name of the user's own can be longer than the header has room for beside its buttons.
    let title = if s.mask_mode { "Mask Settings".to_owned() } else { format!("{} Settings", brushes::shortened(&name, 16)) };
    // One card whatever brush is in hand: it floats under one name.
    theme::category_as(ui, "Brush Settings", &title, Color32::from_rgb(89, 209, 230), "", true, |ui| {
        if s.mask_mode {
            theme::slider(ui, "Mask Hardness", &mut s.mask_hardness, 0.0..=1.0, |v| format!("{v:.2}"));
            stroke::pressure_strength(ui, s.current_mut(), tips);
            theme::hint(ui, "Paint to protect. Ctrl or the right button erases.");
            return;
        }
        let bs = s.current_mut();
        let accumulates = matches!(brush, Brush::Clay | Brush::Crease | Brush::Inflate | Brush::Standard);
        if accumulates {
            let label = if bs.accumulate { "Accumulate" } else { "Accumulate (Off)" };
            tip(
                theme::checkbox(ui, &mut bs.accumulate, label, ACCENT()),
                tips,
                "On: every pass over the same spot adds more. Off: the stroke builds to a fixed height.",
            );
        }
        // Layer has no Accumulate, but this still sets how fast it rises.
        if accumulates || brush == Brush::Layer {
            theme::slider(ui, "Build-Up Strength", &mut bs.build_up, 0.1..=3.0, |v| format!("{v:.2}"));
        }
        // Not for Pose: a limb is posed whole, far side and all.
        if brush != Brush::Pose {
            let label = if bs.front_facing { "Front Facing Only" } else { "Front Facing Only (Off)" };
            tip(
                theme::checkbox(ui, &mut bs.front_facing, label, ACCENT()),
                tips,
                "Ignore surface that faces away from the camera, so thin shapes are not hit on the far side.",
            );
        }
        // Keeps the stroke to the face group it begins on (Mesh tab, Face Groups).
        groups::stay_in_group(ui, bs, tips);
        match brush {
            Brush::Clay => {
                theme::slider(ui, "Clay Depth", &mut s.clay_depth, 0.1..=1.5, |v| format!("{v:.2}"));
                theme::slider(ui, "Tip Softness (Flat - Domed)", &mut s.clay_softness, 0.05..=1.0, |v| format!("{v:.2}"));
                theme::slider(ui, "Surface Relax", &mut s.clay_relax, 0.0..=1.0, |v| format!("{v:.2}"));
            }
            Brush::Crease => {
                theme::slider(ui, "Pinch", &mut s.crease_pinch, 0.0..=1.0, |v| format!("{v:.2}"));
                theme::slider(ui, "Depth", &mut s.crease_depth, 0.05..=1.0, |v| format!("{v:.2}"));
            }
            Brush::Flatten => {
                theme::slider(ui, "Plane Offset (Lower - Higher)", &mut s.flatten_offset, -0.5..=0.5, |v| format!("{v:.2}"));
                modes::flatten(ui, s.current_mut(), tips);
            }
            Brush::Smooth => modes::smooth(ui, s.current_mut(), tips),
            Brush::Pinch => theme::hint(ui, "Draws the surface in to the line of the stroke. Ctrl or the right button spreads it apart."),
            Brush::Nudge => theme::hint(ui, "Strength is how far the surface follows the pen: at 100% what is under the middle of the ring stays under it."),
            Brush::EraseLayer => brushes::erase_layer_note(ui, &doc.mesh),
            Brush::Layer => {
                theme::slider(ui, "Layer Height", &mut s.layer_height, 0.02..=1.0, |v| format!("{v:.2}"));
            }
            Brush::Move => {
                tip(
                    theme::checkbox(ui, &mut s.move_connected, "Connected Only", ACCENT()),
                    tips,
                    "Grab only the surface joined to the point under the pointer: moving one finger leaves the one next to it alone.",
                );
                theme::hint(ui, "Ctrl or the right button pushes the surface in or pulls it out, the way it faces: drag up for out, down for in. At the outline, drag the way it faces.");
            }
            Brush::Pose => pose::settings(ui, s, tips),
            _ => theme::hint(ui, "No extra settings for this brush."),
        }
        stroke::brush_section(ui, s, tips);
        falloff::section(ui, s, tips);
        alpha::section(ui, s, tips, actions);
    });

    // What holds for every brush; the brush's own spacing and pen settings are in its card above.
    theme::category(ui, "Stroke", Color32::from_rgb(168, 133, 255), "", false, |ui| stroke::card(ui, &mut doc.sculptor, tips));
}

/// The Sculpt panel's Mesh tab: what is done to the whole mesh.
fn mesh_page(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    symmetry::card(ui, doc, actions);

    let summary = fmt_count(doc.mesh.triangle_count());
    theme::category(ui, "Remesh", Color32::from_rgb(115, 217, 128), &summary, true, |ui| {
        theme::hint(ui, "Rebuilds the surface with even triangles. Use it after big shape changes, or to add detail.");
        // Log slider: equal travel doubles the resolution.
        let mut log = doc.opts.remesh_resolution.log2();
        if theme::slider(ui, "Resolution", &mut log, 4f32.log2()..=1024f32.log2(), |v| format!("{:.0}", v.exp2())).changed() {
            doc.opts.remesh_resolution = log.exp2().round();
        }
        let style = ButtonStyle { badge: "R", active: Some(ACCENT()), ..Default::default() };
        if tip(
            theme::button(ui, ui.available_width(), "Remesh", style),
            tips,
            "Voxel remesh at this resolution (R).",
        )
        .clicked()
        {
            actions.push(Action::Remesh);
        }
        dyntopo::section(ui, &mut doc.sculptor.dyntopo, tips);
        levels::dyntopo_note(ui, &doc.mesh);

        ui.add_space(6.0);
        theme::hint(ui, "Quad Remesh rebuilds it as even quads that follow the form. Export keeps the quads.");
        let range = crate::quadremesh::TARGET_RANGE;
        let mut log = doc.opts.quad_target.log2();
        if theme::slider(ui, "Quads", &mut log, range.start().log2()..=range.end().log2(), |v| fmt_count(v.exp2() as usize)).changed() {
            // Two significant figures: 5,200 rather than 5,187.
            let step = 10f32.powf(log.exp2().log10().floor() - 1.0);
            doc.opts.quad_target = ((log.exp2() / step).round() * step).clamp(*range.start(), *range.end());
        }
        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
        if tip(
            theme::button(ui, ui.available_width(), "Quad Remesh", style),
            tips,
            "Rebuild as about this many quads.",
        )
        .clicked()
        {
            actions.push(Action::QuadRemesh);
        }

        ui.add_space(6.0);
        decimate(ui, doc, tips, actions);
    });
    levels::card(ui, doc, actions);
    layers::card(ui, doc, &state.assets, actions);
    groups::card(ui, doc, actions);

    theme::category(ui, "Masking", theme::MASK, "", false, |ui| {
        theme::hint(ui, "Masked areas are protected from every brush.");
        let ops = [
            [("Invert", MaskOp::Invert), ("Clear", MaskOp::Clear)],
            [("Blur", MaskOp::Blur), ("Sharpen", MaskOp::Sharpen)],
            [("Grow", MaskOp::Grow), ("Shrink", MaskOp::Shrink)],
        ];
        for row in ops {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                for (label, op) in row {
                    if theme::button(ui, w, label, ButtonStyle::default()).clicked() {
                        actions.push(Action::Mask(op));
                    }
                }
            });
        }
        masking::by_shape(ui, tips, actions);
    });

    // Faces switched off on the selected object (the box and lasso hide tools).
    let hidden = doc.mesh.hidden_triangle_count();
    let summary = if hidden == 0 { String::new() } else { format!("{} hidden", fmt_count(hidden)) };
    theme::category(ui, "Hidden Parts", Color32::from_rgb(120, 190, 235), &summary, false, |ui| {
        theme::hint(ui, "A hidden part is not drawn, and brushes pass over it. Remesh, Join and Export still use the whole model.");
        ui.add_enabled_ui(hidden > 0, |ui| {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                let style = ButtonStyle { icon: state.assets.icon("region_show_all"), ..Default::default() };
                if tip(theme::button(ui, w, "Show All", style), tips, "Bring back everything hidden on the selected object.")
                    .clicked()
                {
                    actions.push(Action::ShowAllFaces);
                }
                let style = ButtonStyle { icon: state.assets.icon("region_invert"), ..Default::default() };
                if tip(theme::button(ui, w, "Invert Visible", style), tips, "Swap the hidden and the showing parts of the selected object.")
                    .clicked()
                {
                    actions.push(Action::InvertVisible);
                }
            });
        });
    });
}

/// Two significant figures: 5,200 rather than 5,187.
fn rounded_to_two_figures(value: f32) -> f32 {
    if !(value > 0.0 && value.is_finite()) {
        return 0.0;
    }
    let step = 10f32.powf(value.log10().floor() - 1.0);
    (value / step).round() * step
}

/// Decimate, at the foot of the Remesh card: the count to come down to, a
/// row of fractions of what the object has now that set it, and the button.
fn decimate(ui: &mut egui::Ui, doc: &mut Doc, tips: bool, actions: &mut Vec<Action>) {
    theme::hint(ui, "Decimate takes triangles off and keeps the shape, for a game model that has to fit a budget (200k to 500k triangles, say).");
    let range = crate::decimate::TARGET_RANGE;
    let now = doc.mesh.triangle_count();
    let mut log = doc.opts.decimate_target.clamp(*range.start(), *range.end()).log2();
    let percent = |target: f32| if now > 0 { format!(" ({:.0}%)", (target / now as f32 * 100.0).min(999.0)) } else { String::new() };
    let slider = theme::slider(ui, "Target Triangles", &mut log, range.start().log2()..=range.end().log2(), |v| {
        format!("{}{}", fmt_count(v.exp2() as usize), percent(v.exp2()))
    });
    let help = "How many triangles Decimate brings the selected object down to. The number in brackets is what share of the triangles it has now.";
    if tip(slider, tips, help).changed() {
        doc.opts.decimate_target = rounded_to_two_figures(log.exp2()).clamp(*range.start(), *range.end());
    }
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 4, 4.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        for share in [0.75f32, 0.5, 0.25, 0.1] {
            let label = format!("{:.0}%", share * 100.0);
            let help = format!("Sets Target Triangles to {label} of the triangles the selected object has now.");
            if tip(theme::button(ui, w, &label, ButtonStyle::default()), tips, &help).clicked() {
                doc.opts.decimate_target = rounded_to_two_figures(now as f32 * share).clamp(*range.start(), *range.end());
            }
        }
    });
    let target = doc.opts.decimate_target.round() as usize;
    ui.add_enabled_ui(now > target, |ui| {
        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
        let help = "Brings the selected object down to about Target Triangles, taking them from where the surface is flat and keeping them where it bends. A closed model stays closed. Fully masked surface is left as it is; hidden parts are decimated too and stay hidden. It works in the background on a large model, with its progress and a Cancel at the bottom of the view. It asks first if the object has symmetry on, which it does not keep exactly. Undo (Z) brings the triangles back.";
        if tip(theme::button(ui, ui.available_width(), "Decimate", style), tips, help).clicked() {
            actions.push(Action::Decimate);
        }
    });
    if now > 0 && now <= target {
        theme::hint(ui, "The selected object is at or under the target already.");
    }
}

/// What stays at the top of the Scene panel whichever tab is showing: the
/// tabs, and the status line.
fn scene_head(ui: &mut egui::Ui, doc: &mut Doc, state: &UiState) {
    tabs::scene_strip(ui, &mut doc.opts.tabs.scene, doc.opts.tooltips);
    ui.add(
        egui::Label::new(
            egui::RichText::new(&state.status.0).font(theme::font(11.0)).color(state.status.1),
        )
        .wrap(),
    );
}

fn scene_panel(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    use tabs::Scene;
    let showing = doc.opts.tabs.scene;
    let mut put_back = None;
    let objects = tabs::page(ui, showing == Scene::Objects, "objects", |ui| {
        scene_graph::show(ui, doc, state, actions);
        // Cards the hosted tools keep here whichever tool is up (Measure & Size).
        tools::cards(ui, doc, state);
    });
    let look = tabs::page(ui, showing == Scene::Look, "look", |ui| look_page(ui, doc, state, actions));
    let file = tabs::page(ui, showing == Scene::File, "file", |ui| file_page(ui, doc, state, actions));
    let settings = tabs::page(ui, showing == Scene::Settings, "settings", |ui| settings::page(ui, doc));
    for (tab, back) in [(Scene::Objects, objects), (Scene::Look, look), (Scene::File, file), (Scene::Settings, settings)] {
        if back {
            put_back = Some(tab);
        }
    }
    if let Some(tab) = put_back {
        doc.opts.tabs.scene = tab;
    }
}

/// The Scene panel's File tab: the file, and the videos made of the model.
fn file_page(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let file = doc
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unsaved".into());
    theme::category(ui, "File", Color32::from_rgb(153, 184, 230), &file, true, |ui| {
        let row = |ui: &mut egui::Ui, actions: &mut Vec<Action>, items: &[(&str, Action, &str)]| {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, items.len(), 6.0);
                for (label, action, help) in items {
                    if tip(theme::button(ui, w, label, ButtonStyle::default()), tips, help).clicked() {
                        actions.push(action.clone());
                    }
                }
            });
        };
        row(ui, actions, &[(
            "New Scene",
            Action::NewScene,
            "Start again with a fresh sphere, in place of this scene (Ctrl+N). It asks first if anything would be lost; Undo brings this scene back.",
        )]);
        row(ui, actions, &[
            ("Open...", Action::Open, "Open a saved sculpt (.sclp, or a .sculpt from the Unity app), in place of the scene or added to it (Ctrl+O)."),
            ("Save", Action::Save, "Save this sculpt (Ctrl+S)."),
            ("Save As...", Action::SaveAs, "Save under a new name (Ctrl+Shift+S)."),
        ]);
        row(ui, actions, &[
            (
                "Import...",
                Action::Import,
                "Add an .obj or .stl model to the scene. An STL comes in at its real size (millimetres, Z up); an OBJ is fitted to the first object's size. Either lands at the centre. A .sclp, or a .sculpt from the Unity app, adds its objects where they were saved.",
            ),
            (
                "Export OBJ...",
                Action::ExportObj,
                "Write the selected object as a Wavefront OBJ, placed as it is in the scene (position, rotation and scale included). A painted object goes with its colour at every point; its materials need PLY or FBX.",
            ),
            (
                "Export FBX...",
                Action::ExportFbx,
                "Write every selected object into one Autodesk FBX, for Blender, Unity and the like: each an object of its own under its name, placed as it is in the scene, Y up, at real size (1 unit is 1 cm). A painted object goes with its colour, and each face with the material most of it is painted with. Ctrl + click rows of the Objects list to select more than one.",
            ),
        ]);
        row(ui, actions, &[(
            "Export PLY...",
            Action::ExportPly,
            "Write the selected object as a PLY, placed as it is in the scene: the format most programs read painted colour from (MeshLab, colour 3D printing). A painted object goes with its colour and, for each material it is painted with, how much of it is at each point.",
        )]);
        tip(
            theme::checkbox(ui, &mut doc.opts.fbx_material_weights, "FBX: material weights in UVs", ACCENT()),
            tips,
            "Export FBX also writes how much of each painted material is at each point into the object's UV sets 1 to 4 (two materials a set), for a game shader of your own to blend textures by. Unity reads them as uv2 to uv5. Off: the FBX has its colour and one material per face, which is all Blender and Unity use by themselves.",
        );
        // The list that shows the selection is on another tab, so say here what the file would hold.
        let picked = doc.scene.selected().len();
        if picked > 1 {
            theme::hint(ui, &format!("{picked} objects are selected. Export FBX writes them all into one file; OBJ and STL write the bright one only."));
        }
        row(ui, actions, &[(
            "Export STL for Printing (mm)...",
            Action::ExportStl,
            "Write the selected object as a binary STL for a slicer: placed as it is in the scene, upright (Z up), in millimetres at real size (1 unit is 10 mm). If the object has holes it says so first, and asks.",
        )]);
        row(ui, actions, &[(
            "Check for Printing",
            Action::CheckForPrinting,
            "Look the selected object over for holes and for edges shared by more than two faces, either of which can stop it printing. The answer is in the line at the top of this panel.",
        )]);
        row(ui, actions, &[(
            "Send to Blender",
            Action::SendToBlender,
            "Send every selected object to Blender as it stands, at its real size, and open Blender if it is not open. No file to save or find. In Blender, Send to GolemSculpt (the GolemSculpt tab of the sidebar, N) brings an object back: one that came from here returns as itself, with its new shape, and anything made there arrives as a new object.",
        )]);
        crate::recent::tiles(ui, &mut state.recent, tips, actions);
        // Last, and away from the buttons that are pressed all day.
        row(ui, actions, &[(
            "Exit",
            Action::Exit,
            "Close GolemSculpt, as the window's close button does (Ctrl+Q). It asks first if anything would be lost.",
        )]);
    });
    crate::turntable::card(ui, doc);
    crate::timelapse::card(ui, doc);
}

/// The Scene panel's Look tab: material, lighting, backdrop and effects.
fn look_page(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let model_size = crate::look::model_size(&doc.scene, &doc.mesh);
    let o = &mut doc.opts;
    theme::category(ui, "Material", Color32::from_rgb(242, 217, 102), "", true, |ui| {
        tip(
            theme::checkbox(ui, &mut o.matcap_on, "Matcap", ACCENT()),
            tips,
            "Shade with a picture of a lit sphere: quick, and the same from every angle.",
        );
        if o.matcap_on {
            let mut category = "";
            let cols = 4;
            let w = theme::row_width(ui, cols, 4.0);
            let count = state.assets.matcaps.len();
            let mut i = 0;
            while i < count {
                let cat = state.assets.matcaps[i].category.as_str();
                if cat != category {
                    category = cat;
                    theme::heading(ui, cat);
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let mut n = 0;
                    while i < count && n < cols && state.assets.matcaps[i].category == category {
                        let m = &state.assets.matcaps[i];
                        let (rect, response) = ui.allocate_exact_size(Vec2::splat(w), Sense::click());
                        ui.painter().rect_filled(rect, 7.0, theme::FIELD());
                        ui.painter().image(
                            m.thumb.id(),
                            rect.shrink(3.0),
                            egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        if o.matcap == i {
                            ui.painter().rect_stroke(
                                rect,
                                7.0,
                                egui::Stroke::new(2.0, ACCENT()),
                                egui::StrokeKind::Inside,
                            );
                        } else if response.hovered() {
                            ui.painter().rect_filled(rect, 7.0, Color32::from_white_alpha(20));
                        }
                        if tip(response, tips, &m.name).clicked() {
                            actions.push(Action::Matcap(i));
                        }
                        i += 1;
                        n += 1;
                    }
                });
            }
        }
        look::material(ui, o, model_size, actions);
        theme::heading(ui, "Cavity");
        theme::checkbox(ui, &mut o.cavity, "Enabled", ACCENT());
        if o.cavity {
            theme::slider(ui, "Ridge", &mut o.cavity_ridge, 0.0..=2.0, |v| format!("{v:.2}"));
            theme::slider(ui, "Valley", &mut o.cavity_valley, 0.0..=2.0, |v| format!("{v:.2}"));
        }
    });
    look::lighting(ui, &mut doc.opts, actions);
    look::effects(ui, &mut doc.opts);
}
