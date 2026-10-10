//! The interface: two floating panels over a full-window viewport, a view
//! gizmo, the brush cursor and the radial brush menu. Layout and wording
//! follow the Unity app.

pub mod assets;
mod brush_size;
mod falloff;
mod look;
mod mirror_copies;
mod overlay;
mod pose;
pub mod question;
mod scene_graph;
mod spheres;
mod symmetry;
pub mod theme;
mod tools;

use egui::{Align2, Color32, Id, Order, Pos2, Sense, Vec2, pos2, vec2};

use crate::doc::{Action, Doc, MaskOp, Tool};
use crate::sculpt::Brush;
use assets::Assets;
pub use overlay::Cursor;

/// Cursor ring colours, for the code that decides which one applies.
pub mod overlay_colors {
    pub use super::overlay::{ADD, SMOOTH, SUBTRACT};
}
use theme::{ButtonStyle, ACCENT};

const LEFT_WIDTH: f32 = 270.0;
const RIGHT_WIDTH: f32 = 260.0;
const MARGIN: f32 = 8.0;

pub struct UiState {
    pub assets: Assets,
    /// Message, colour and the time it appeared.
    pub toast: Option<(String, Color32, f64)>,
    pub status: (String, Color32),
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
            status: ("Import adds a model (.obj / .stl).".into(), theme::HINT),
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

    overlay::transform_gizmo(&ctx, &state.gizmo);
    overlay::cursor(&ctx, cursor);
    pose::overlay(&ctx, &state.pose);
    tools::overlay(&ctx, doc);

    if !doc.opts.clean_view {
        let max_height = (screen.height() - 2.0 * MARGIN).max(300.0);
        let panel = |ui: &mut egui::Ui, width: f32, body: &mut dyn FnMut(&mut egui::Ui)| {
            // A floating area only offers its previous size; ask for the full
            // window height so the panel can grow with its content.
            ui.set_max_height(max_height);
            egui::Frame::new()
                .fill(theme::PANEL)
                .corner_radius(10.0)
                .stroke(theme::hairline())
                .inner_margin(egui::Margin { left: 10, right: 10, top: 10, bottom: 10 })
                .show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    egui::ScrollArea::vertical()
                        .max_height(max_height - 20.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(width - 20.0);
                            body(ui)
                        });
                });
        };

        egui::Area::new(Id::new("sculpt_panel"))
            .order(Order::Middle)
            .fade_in(false)
            .fixed_pos(pos2(MARGIN, MARGIN))
            .show(&ctx, |ui| {
                panel(ui, LEFT_WIDTH, &mut |ui| sculpt_panel(ui, doc, state, &mut actions));
            });
        egui::Area::new(Id::new("scene_panel"))
            .order(Order::Middle)
            .fade_in(false)
            .fixed_pos(pos2(screen.right() - MARGIN - RIGHT_WIDTH, MARGIN))
            .show(&ctx, |ui| {
                panel(ui, RIGHT_WIDTH, &mut |ui| scene_panel(ui, doc, state, &mut actions));
            });

        overlay::view_gizmo(
            &ctx,
            doc,
            pos2(screen.right() - MARGIN - RIGHT_WIDTH - 14.0 - 108.0, 14.0),
            &mut actions,
        );

        // Bottom bar: the two things toggled constantly.
        egui::Area::new(Id::new("bottom_bar"))
            .order(Order::Middle)
            .fade_in(false)
            .fixed_pos(pos2(MARGIN + LEFT_WIDTH + 12.0, screen.bottom() - 12.0 - 22.0))
            .show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let label = if doc.opts.tooltips { "Tooltips: On" } else { "Tooltips: Off" };
                    theme::checkbox(ui, &mut doc.opts.tooltips, label, ACCENT);
                    theme::checkbox(ui, &mut doc.opts.flat_shading, "Flat Shading", ACCENT);
                    theme::checkbox(ui, &mut doc.opts.wireframe, "Wireframe", ACCENT);
                });
            });
    }

    overlay::radial_menu(&ctx, doc, state);
    tools::radial(&ctx, doc, state);
    tools::jobs(&ctx, doc);
    scene_graph::confirm(&ctx, doc, state, &mut actions);
    overlay::toast(&ctx, state, time);
    if doc.opts.clean_view {
        overlay::clean_hint(&ctx, crate::turntable::HINT, doc.turntable.hint_alpha(time));
    }
    crate::turntable::recording_overlay(&ctx, doc);
    actions
}

fn sculpt_panel(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let icon = |name: &str| state.assets.icon(name);

    // Title and mesh size.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, "Sculpt", theme::bold(19.0), theme::TEXT);
    ui.painter().text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        format!(
            "{} tris   {} verts",
            fmt_count(doc.mesh.triangle_count()),
            fmt_count(doc.mesh.vertex_count())
        ),
        theme::font(11.0),
        theme::HINT,
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
        theme::category(ui, "Transform", Color32::from_rgb(76, 153, 255), &summary, true, |ui| {
            theme::hint(ui, "Drag an arrow to move, a ring to turn, a cube on an arm to stretch that way, or the white cube to resize.");
            theme::hint(ui, "Click another object to select it; Shift or Ctrl + click adds it to the selection or takes it out. Several selected objects go together, about their shared centre. Drag anywhere else to orbit.");
            theme::hint(ui, "If part of the object is masked, the gizmo reshapes the rest and leaves the masked part where it is.");
            // Which way the arrows and rings point.
            theme::hint(ui, "Gizmo axes (arrows and rings)");
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 3, 6.0);
                for axes in crate::gizmo::Orientation::ALL {
                    let style = ButtonStyle { active: (doc.opts.gizmo_axes == axes).then_some(ACCENT), ..Default::default() };
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

    let s = &mut doc.sculptor.settings;
    let brush = s.brush;
    let summary = if s.mask_mode { "Mask" } else { brush.name() };
    theme::category(ui, "Brushes", Color32::from_rgb(76, 153, 255), summary, true, |ui| {
        let w = theme::row_width(ui, 5, 4.0);
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for row in Brush::ALL.chunks(5) {
            ui.horizontal(|ui| {
                for &b in row {
                    let active = (b == s.brush && !s.mask_mode).then_some(ACCENT);
                    let badge = b.hotkey().to_string();
                    let r = theme::tile(ui, vec2(w, 40.0), icon(b.icon()), "", &badge, active);
                    let text = format!("{} ({})\n{}", b.name(), b.hotkey(), b.description());
                    if tip(r, tips, &text).clicked() {
                        s.brush = b;
                        // Picking a brush always leaves mask mode, so the
                        // click never looks dead.
                        s.mask_mode = false;
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
                active: (polar && positive).then_some(ACCENT),
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
        let bs = s.current_mut();
        tip(
            theme::slider(ui, "Strength", &mut bs.strength, 0.01..=1.0, |v| format!("{:.0}%", v * 100.0)),
            tips,
            "How hard the brush works. Hold F and drag.",
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

    let title = if s.mask_mode { "Mask Settings".to_owned() } else { format!("{} Settings", brush.name()) };
    theme::category(ui, &title, Color32::from_rgb(89, 209, 230), "", true, |ui| {
        if s.mask_mode {
            theme::slider(ui, "Mask Hardness", &mut s.mask_hardness, 0.0..=1.0, |v| format!("{v:.2}"));
            theme::hint(ui, "Paint to protect. Ctrl or the right button erases.");
            return;
        }
        let bs = s.current_mut();
        let accumulates =
            !matches!(brush, Brush::Move | Brush::Pose | Brush::SnakeHook | Brush::Smooth | Brush::Flatten | Brush::Layer);
        if accumulates {
            let label = if bs.accumulate { "Accumulate" } else { "Accumulate (Off)" };
            tip(
                theme::checkbox(ui, &mut bs.accumulate, label, ACCENT),
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
                theme::checkbox(ui, &mut bs.front_facing, label, ACCENT),
                tips,
                "Ignore surface that faces away from the camera, so thin shapes are not hit on the far side.",
            );
        }
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
                theme::slider(ui, "Plane Offset (Scrape - Fill)", &mut s.flatten_offset, -0.5..=0.5, |v| format!("{v:.2}"));
            }
            Brush::Layer => {
                theme::slider(ui, "Layer Height", &mut s.layer_height, 0.02..=1.0, |v| format!("{v:.2}"));
            }
            Brush::Move => {
                tip(
                    theme::checkbox(ui, &mut s.move_connected, "Connected Only", ACCENT),
                    tips,
                    "Grab only the surface joined to the point under the pointer: moving one finger leaves the one next to it alone.",
                );
            }
            Brush::Pose => pose::settings(ui, s, tips),
            _ => theme::hint(ui, "No extra settings for this brush."),
        }
        falloff::section(ui, s, tips);
    });

    theme::category(ui, "Stroke", Color32::from_rgb(168, 133, 255), "", false, |ui| {
        tip(
            theme::checkbox(ui, &mut s.build_up_on_hold, "Build Up on Hold", ACCENT),
            tips,
            "Keep adding while the pointer rests in one place.",
        );
        tip(
            theme::checkbox(ui, &mut s.lazy_mouse, "Lazy Mouse", ACCENT),
            tips,
            "The brush trails the pointer on a short rope, steadying long curves.",
        );
        if s.lazy_mouse {
            theme::slider(ui, "Lazy Radius", &mut s.lazy_radius, 1.0..=150.0, |v| format!("{v:.0} px"));
            theme::slider(ui, "Lazy Smoothing", &mut s.lazy_strength, 0.05..=1.0, |v| format!("{v:.2}"));
        }
        theme::heading(ui, "Stylus Pressure");
        theme::slider(ui, "Light-Touch Floor", &mut s.pressure_floor, 0.0..=0.5, |v| format!("{v:.2}"));
        theme::slider(ui, "Curve", &mut s.pressure_curve, 0.5..=3.0, |v| format!("{v:.2}"));
    });

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

    symmetry::card(ui, doc);

    let summary = fmt_count(doc.mesh.triangle_count());
    theme::category(ui, "Remesh", Color32::from_rgb(115, 217, 128), &summary, true, |ui| {
        theme::hint(ui, "Rebuilds the surface with even triangles. Use it after big shape changes, or to add detail.");
        // Log slider: equal travel doubles the resolution.
        let mut log = doc.opts.remesh_resolution.log2();
        if theme::slider(ui, "Resolution", &mut log, 4f32.log2()..=1024f32.log2(), |v| format!("{:.0}", v.exp2())).changed() {
            doc.opts.remesh_resolution = log.exp2().round();
        }
        let style = ButtonStyle { badge: "R", active: Some(ACCENT), ..Default::default() };
        if tip(
            theme::button(ui, ui.available_width(), "Remesh", style),
            tips,
            "Voxel remesh at this resolution (R).",
        )
        .clicked()
        {
            actions.push(Action::Remesh);
        }

        ui.add_space(6.0);
        theme::hint(ui, "Quad Remesh rebuilds it as even quads that follow the form. Export keeps the quads.");
        let range = crate::quadremesh::TARGET_RANGE;
        let mut log = doc.opts.quad_target.log2();
        if theme::slider(ui, "Quads", &mut log, range.start().log2()..=range.end().log2(), |v| fmt_count(v.exp2() as usize)).changed() {
            // Two significant figures: 5,200 rather than 5,187.
            let step = 10f32.powf(log.exp2().log10().floor() - 1.0);
            doc.opts.quad_target = ((log.exp2() / step).round() * step).clamp(*range.start(), *range.end());
        }
        let style = ButtonStyle { active: Some(ACCENT), ..Default::default() };
        if tip(
            theme::button(ui, ui.available_width(), "Quad Remesh", style),
            tips,
            "Rebuild as about this many quads.",
        )
        .clicked()
        {
            actions.push(Action::QuadRemesh);
        }
    });

    theme::category(ui, "Interface Size", Color32::from_rgb(242, 158, 89), &format!("{:.0}%", doc.opts.ui_scale * 100.0), false, |ui| {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 4, 4.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            for scale in [1.0f32, 1.25, 1.5, 2.0] {
                let style = ButtonStyle {
                    active: ((doc.opts.ui_scale - scale).abs() < 0.01).then_some(ACCENT),
                    ..Default::default()
                };
                if theme::button(ui, w, &format!("{:.0}%", scale * 100.0), style).clicked() {
                    doc.opts.ui_scale = scale;
                }
            }
        });
        theme::hint(ui, "Ctrl + = and Ctrl + - also work.");
    });

    theme::category(ui, "Keyboard Shortcuts", Color32::from_rgb(230, 204, 102), "", false, |ui| {
        let rows: &[(&str, &str)] = &[
            ("1 - 0", "Pick a brush"),
            ("Shift (hold)", "Smooth"),
            ("Ctrl / Right button", "Invert the brush"),
            ("S + drag", "Brush size"),
            ("F + drag", "Brush strength"),
            ("D + drag", "Focal shift"),
            ("Scroll on model", "Brush size"),
            ("Space (hold)", "Brush menu"),
            ("M", "Mask painting"),
            ("X", "Mirror X"),
            ("R", "Remesh"),
            ("G", "Transform gizmo"),
            ("L", "Measure"),
            ("Z / Shift+Z", "Undo / redo"),
            ("Alt + drag", "Orbit"),
            ("Drag on background", "Orbit"),
            ("Middle drag", "Pan"),
            ("Alt + Shift + drag", "Pan"),
            ("Ctrl + Alt + drag", "Zoom"),
            ("Scroll off model", "Zoom"),
            ("F F", "Frame the selection"),
            ("Double-click object", "Select it"),
            ("Ctrl + click a row", "Add to the selection"),
            ("H / Shift + H", "Hide selected / the rest"),
            ("Alt + H", "Show everything"),
            ("/", "Local View: solo selected"),
            ("Delete", "Delete the object (asks)"),
            ("Tab", "Hide panels (clean view)"),
            ("O", "Spin the view"),
            ("F11", "Fullscreen"),
            ("Ctrl + S", "Save"),
        ];
        for (key, what) in rows {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::hover());
            ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, key, theme::font(11.0), theme::KEY);
            ui.painter().text(
                rect.left_center() + vec2(118.0, 0.0),
                Align2::LEFT_CENTER,
                what,
                theme::font(11.0),
                theme::TEXT,
            );
        }
    });
}

fn scene_panel(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, "Scene", theme::bold(19.0), theme::TEXT);
    ui.add(
        egui::Label::new(
            egui::RichText::new(&state.status.0).font(theme::font(11.0)).color(state.status.1),
        )
        .wrap(),
    );

    scene_graph::show(ui, doc, state, actions);
    // Cards the hosted tools keep here whichever tool is up (Measure & Size).
    tools::cards(ui, doc, state);

    let file = doc
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unsaved".into());
    theme::category(ui, "File", Color32::from_rgb(153, 184, 230), &file, true, |ui| {
        let mut row = |ui: &mut egui::Ui, items: &[(&str, Action, &str)]| {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, items.len(), 6.0);
                for (label, action, help) in items {
                    if tip(theme::button(ui, w, label, ButtonStyle::default()), tips, help).clicked() {
                        actions.push(action.clone());
                    }
                }
            });
        };
        row(ui, &[
            ("Open...", Action::Open, "Open a saved sculpt (.sclp), in place of the scene or added to it (Ctrl+O)."),
            ("Save", Action::Save, "Save this sculpt (Ctrl+S)."),
            ("Save As...", Action::SaveAs, "Save under a new name (Ctrl+Shift+S)."),
        ]);
        row(ui, &[
            (
                "Import...",
                Action::Import,
                "Add an .obj or .stl model to the scene. An STL comes in at its real size (millimetres, Z up); an OBJ is fitted to the first object's size. Either lands at the centre. A .sclp adds its objects where they were saved.",
            ),
            (
                "Export OBJ...",
                Action::ExportObj,
                "Write the selected object as a Wavefront OBJ, placed as it is in the scene (position, rotation and scale included).",
            ),
        ]);
        row(ui, &[(
            "Export STL for Printing (mm)...",
            Action::ExportStl,
            "Write the selected object as a binary STL for a slicer: placed as it is in the scene, upright (Z up), in millimetres at real size (1 unit is 10 mm).",
        )]);
        crate::recent::tiles(ui, &mut state.recent, tips, actions);
    });

    let model_size = crate::look::model_size(&doc.scene, &doc.mesh);
    let o = &mut doc.opts;
    theme::category(ui, "Material", Color32::from_rgb(242, 217, 102), "", true, |ui| {
        tip(
            theme::checkbox(ui, &mut o.matcap_on, "Matcap", ACCENT),
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
                        ui.painter().rect_filled(rect, 7.0, theme::FIELD);
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
                                egui::Stroke::new(2.0, ACCENT),
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
        theme::checkbox(ui, &mut o.cavity, "Enabled", ACCENT);
        if o.cavity {
            theme::slider(ui, "Ridge", &mut o.cavity_ridge, 0.0..=2.0, |v| format!("{v:.2}"));
            theme::slider(ui, "Valley", &mut o.cavity_valley, 0.0..=2.0, |v| format!("{v:.2}"));
        }
    });
    look::lighting(ui, &mut doc.opts, actions);
    look::effects(ui, &mut doc.opts);
    crate::turntable::card(ui, doc);
    crate::timelapse::card(ui, doc);

    theme::category(ui, "Performance", Color32::from_rgb(158, 158, 173), &format!("{:.1} ms", doc.stats.frame_ms), false, |ui| {
        let (undo, redo) = doc.history.steps();
        let lines = [
            format!("Frame: {:.2} ms", doc.stats.frame_ms),
            format!("Sculpting: {:.2} ms per frame", doc.stats.sculpt_ms),
            format!("Sent to GPU: {:.0} KB per update", doc.stats.upload_kb),
            format!("History: {undo} undo, {redo} redo, {:.0} MB", doc.history.bytes() as f64 / 1e6),
            doc.stats.adapter.clone(),
        ];
        for line in lines {
            ui.add(egui::Label::new(egui::RichText::new(line).font(theme::font(11.0)).color(theme::HINT)).wrap());
        }
    });
}
