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
pub mod decimate;
pub mod deform;
mod dyntopo;
mod falloff;
pub mod groups;
mod layers;
mod levels;
mod look;
pub mod materials;
mod masking;
mod mirror_copies;
mod modes;
pub mod older;
mod overlay;
mod paint;
mod panels;
mod pose;
mod project;
pub mod question;
mod scene_graph;
mod settings;
mod shortcuts;
mod spheres;
pub mod status;
mod stroke;
mod symmetry;
pub mod tabs;
pub mod theme;
mod tools;
mod rebuild_notes;
mod remesh_smooth;
mod history_list;
mod status_row;
mod view_handles;

use egui::{Align2, Color32, Id, Order, Pos2, Sense, Vec2, pos2, vec2};

use crate::doc::{Action, Doc, MaskOp, Tool};
use crate::sculpt::Brush;
use assets::Assets;
pub use overlay::{Cursor, brush_wheel_ring, brush_wheel_tile};
/// The brush's size as the Size slider shows it, for the S key's readout.
pub use brush_size::shown as brush_size_shown;
/// The Brushes card's controls, for the interface's self-check.
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
    /// Space has been let go: the wheel closes at the end of the next frame,
    /// once it has seen a tap that came in with the release.
    pub radial_closing: bool,
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
    /// The Materials card's thumbnails (see materials.rs).
    pub materials: materials::Thumbs,
    /// The Older Versions window, while it is up (see older.rs).
    pub older: older::State,
    /// Whether the scene has changes that are not saved: the File card's
    /// header stars its name, as the window's title does (app/title.rs).
    pub unsaved: bool,
    /// The brushes taken up lately, newest first, by name: the Brushes
    /// card's Recent row (brushes.rs). Kept in the settings file.
    pub recent_brushes: Vec<String>,
    /// Whether the size and strength plate is pinned at the top of the
    /// Sculpt panel this frame (`sculpt_head`), so the Brush tab leaves it out.
    pub plate_pinned: bool,
}

impl UiState {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            assets: Assets::load(ctx),
            toast: None,
            // Nothing to say until something happens: the line takes no room.
            status: (String::new(), theme::HINT()),
            status_seen: Default::default(),
            radial: None,
            radial_closing: false,
            graph: Default::default(),
            gizmo: Vec::new(),
            recent: Default::default(),
            pose: Vec::new(),
            materials: Default::default(),
            unsaved: false,
            recent_brushes: Vec::new(),
            plate_pinned: false,
            older: Default::default(),
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
    // However the brush was taken up (the list, the wheel, a number key), it is the most recent.
    brushes::note_in_hand(&mut state.recent_brushes, &doc.sculptor.library, &doc.sculptor.settings);

    overlay::transform_gizmo(&ctx, &state.gizmo);
    overlay::cursor(&ctx, cursor);
    pose::overlay(&ctx, &state.pose);
    tools::overlay(&ctx, doc);

    if !doc.opts.clean_view {
        // Each panel, unless it is folded away, and the tab that folds it.
        let (tips, mut folded) = (doc.opts.tooltips, doc.opts.folded);
        sculpt_toolkit::floating::set_room(&ctx, panels::room_for_cards(screen, folded));
        // Each tab of the Sculpt panel keeps its own place in its scroll too.
        let page = doc.opts.tabs.sculpt.name();
        panels::panel(ui, Side::Left, folded[0], page, &mut |ui, part| match part {
            Part::Head => sculpt_head(ui, doc, state, &mut actions),
            Part::Body => sculpt_panel(ui, doc, state, &mut actions),
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

        // Bottom bar: the ways of looking at the surface toggled constantly.
        // Its first row sits where it always has; in a window too narrow for
        // all of it between the panels, the rest goes on a row above.
        let origin = panels::bottom_bar_origin(screen, folded[0]);
        egui::Area::new(Id::new("bottom_bar"))
            .order(Order::Middle)
            .fade_in(false)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(origin + vec2(0.0, 22.0))
            .show(&ctx, |ui| {
                ui.set_max_width(panels::bottom_bar_width(screen, folded));
                // What every stroke works under, above the switches (status_row.rs).
                status_row::show(ui, doc);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let flat = theme::checkbox(ui, &mut doc.opts.flat_shading, "Flat Shading", ACCENT());
                    tip(flat, tips, "Shade every triangle flat, so the facets of the mesh show.");
                    let wire = theme::checkbox(ui, &mut doc.opts.wireframe, "Wireframe", ACCENT());
                    tip(wire, tips, "Draw the edges of the triangles over the model.");
                    let backs = theme::checkbox(ui, &mut doc.opts.show_backfaces, "Show Backfaces", ACCENT());
                    tip(
                        backs,
                        tips,
                        "Draw the inside of the surface too, tinted blue-grey. Off, where the model is open (a hole, a cut, \
                         hidden faces) you look straight through it to what is behind.",
                    );
                    let groups = theme::checkbox(ui, &mut doc.opts.groups.colours, "Group Colours", ACCENT());
                    tip(groups, tips, "Colour each face group of the model differently (Mesh tab, Face Groups card).");
                });
            });
        doc.opts.folded = folded;
        // What a folded panel alone would have said: the status line first of all.
        status::overlay(&ctx, doc, state, folded, time);
    }

    overlay::radial_menu(&ctx, doc, state, &mut actions);
    tools::radial(&ctx, doc, state);
    tools::jobs(&ctx, doc);
    scene_graph::confirm(&ctx, doc, state, &mut actions);
    older::show(&ctx, doc, state, &mut actions);
    overlay::toast(&ctx, state, time);
    if doc.opts.clean_view {
        overlay::clean_hint(&ctx, crate::turntable::HINT, doc.turntable.hint_alpha(time));
        // No panels, so no status line: an error is said all the same.
        status::clean_view(&ctx, state, doc.opts.tooltips, time);
    }
    crate::turntable::recording_overlay(&ctx, doc);
    actions
}

/// The Sculpt panel's cards, under what is pinned at its top (`sculpt_head`).
fn sculpt_panel(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    // A hosted tool or SSpheres up: the tools to change to, then its panel.
    if !tabbed(doc) {
        tools::buttons(ui, doc, state);
        if tools::panel(ui, doc, state) {
            return;
        }
        if doc.tool == Tool::SSpheres {
            spheres::panel(ui, doc, state, actions);
        }
        return;
    }
    if doc.tool == Tool::Transform {
        let count = doc.scene.selected().len();
        let summary = if count > 1 { format!("{count} objects") } else { doc.scene.active().name.clone() };
        theme::category(ui, "Transform", theme::lit(ACCENT()), &summary, true, |ui| {
            theme::about(
                ui,
                "Drag an arrow to move, a ring to turn, a cube on an arm to stretch that way, or the white cube to resize.\n\n\
                 Click another object to select it; Shift or Ctrl + click adds it to the selection or takes it out. Several \
                 selected objects go together, about their shared centre. Drag anywhere else to orbit.\n\n\
                 If part of the object is masked, the gizmo reshapes the rest and leaves the masked part where it is.",
            );
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
        // Bend, Twist and the other deformers the gizmo drives (app/deform.rs).
        deform::card(ui, doc, &state.assets, actions);
    }

    // The tab strip is in the head; the tab's cards, here.
    let mut showing = doc.opts.tabs.sculpt;
    if tabs::page(ui, showing == tabs::Sculpt::Brush, "brush", |ui| brush_page(ui, doc, state, actions)) {
        showing = tabs::Sculpt::Brush;
    }
    if tabs::page(ui, showing == tabs::Sculpt::Mesh, "mesh", |ui| mesh_page(ui, doc, state, actions)) {
        showing = tabs::Sculpt::Mesh;
    }
    if tabs::page(ui, showing == tabs::Sculpt::Tools, "tools", |ui| tools_page(ui, doc, state)) {
        showing = tabs::Sculpt::Tools;
    }
    doc.opts.tabs.sculpt = showing;
}

/// Whether the Sculpt panel shows its tabs: while sculpting or with the
/// Transform tool up. A hosted tool or SSpheres has the panel to itself.
fn tabbed(doc: &Doc) -> bool {
    matches!(doc.tool, Tool::Sculpt | Tool::Transform)
}

/// How much room the Sculpt panel keeps for its cards under what is pinned
/// at its top, in the interface's points. With less, the size and strength
/// plate goes back to the top of the Brush tab and scrolls with the rest.
const ROOM_FOR_CARDS: f32 = 220.0;

/// What stays at the top of the Sculpt panel however far its cards are
/// scrolled: the title, Undo and Redo, the Sculpt / Transform / SSpheres
/// row, the Brush / Mesh / Tools tabs, and on the Brush tab the size and
/// strength plate, when the panel is tall enough to keep that and still
/// show cards under it.
fn sculpt_head(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let icon = |name: &str| state.assets.icon(name);

    // Title, mesh size, and the History list's button at the end of the line.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, "GolemSculpt", theme::bold(19.0), theme::TEXT());
    let list = egui::Rect::from_min_size(pos2(rect.right() - history_list::WIDTH, rect.top() + 1.0), vec2(history_list::WIDTH, 24.0));
    history_list::button(&mut ui.new_child(egui::UiBuilder::new().max_rect(list)), doc, tips, actions);
    ui.painter().text(
        rect.right_center() - vec2(history_list::WIDTH + 8.0, 0.0),
        Align2::RIGHT_CENTER,
        // Triangles only: the line has the History button at its end now.
        format!("{} tris", fmt_count(doc.mesh.live_triangle_count())),
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

    let mut pinned = false;
    if tabbed(doc) {
        let mut showing = doc.opts.tabs.sculpt;
        tabs::sculpt_strip(ui, &mut showing, tips);
        doc.opts.tabs.sculpt = showing;
        if showing == tabs::Sculpt::Brush {
            // As tall as it was last time it was drawn here.
            let id = Id::new("pinned_plate_height");
            let plate = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(330.0);
            if ui.available_height() - plate >= ROOM_FOR_CARDS {
                let top = ui.cursor().top();
                brush_plate(ui, &mut doc.sculptor.settings, &icon, tips);
                let height = ui.cursor().top() - top;
                ui.data_mut(|d| d.insert_temp(id, height));
                pinned = true;
            }
        }
    }
    state.plate_pinned = pinned;
}

/// The Sculpt panel's Tools tab: the tools that work on the model in their
/// own way, each a button that brings it up (and its key, where it has one).
fn tools_page(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState) {
    tools::buttons(ui, doc, state);
    theme::hint(ui, "A tool takes the panel while it is up; Esc, its key again or picking a brush puts it away.");
}

/// What every brush has: which way it works, its size, strength and focal
/// shift. Pinned at the top of the Sculpt panel on the Brush tab when the
/// panel has room for it (`sculpt_head`), else at the top of the tab.
fn brush_plate(ui: &mut egui::Ui, s: &mut crate::sculpt::Settings, icon: &dyn Fn(&str) -> Option<egui::TextureId>, tips: bool) {
    let brush = s.brush;
    theme::plate(ui, |ui| {
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
        } else if brush == Brush::Paint && !s.mask_mode {
            "Flow: how much of the Opacity each dab of paint lays. Low builds paint up softly as you go over a place. Hold F and drag."
        } else if brush == Brush::SnakeHook && !s.mask_mode {
            "How far the pulled clay follows the pen: at 100% and full pressure it stays with it; lower, or a lighter touch, \
             lets it lag and thin out. Hold F and drag."
        } else {
            "How hard the brush works. The slider gives the gentle end the most room: on Standard, Inflate and Layer \
             one pass at 10% barely shows, at 50% it is a clear stroke and at 100% a strong one. Hold F and drag."
        };
        // The Paint brush's Strength is its Flow (sculpt/paint.rs).
        let label = if brush == Brush::Paint && !s.mask_mode { "Flow" } else { "Strength" };
        let bs = s.current_mut();
        tip(
            theme::slider(ui, label, &mut bs.strength, 0.01..=1.0, |v| format!("{:.0}%", v * 100.0)),
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

    // Size, strength and focal shift: pinned at the top of the panel when it
    // has room (`sculpt_head`), else here at the top of the tab.
    if !state.plate_pinned {
        brush_plate(ui, s, &icon, tips);
    }

    // Lazy Mouse and the pen's curve, right under Size and Strength: they change how every stroke feels.
    let summary = stroke::summary(s);
    let pen = &doc.sculptor.pen_meter;
    theme::category(ui, "Stroke", Color32::from_rgb(168, 133, 255), &summary, false, |ui| stroke::card(ui, s, pen, tips));

    brushes::card(ui, (library, s), &state.recent_brushes, &icon, tips, actions);

    // A name of the user's own can be longer than the header has room for beside its buttons.
    let title = if s.mask_mode { "Mask Settings".to_owned() } else { format!("{} Settings", brushes::shortened(&name, 16)) };
    // One card whatever brush is in hand: it floats under one name.
    theme::category_as(ui, "Brush Settings", &title, Color32::from_rgb(89, 209, 230), "", true, |ui| {
        if s.mask_mode {
            tip(
                theme::slider(ui, "Mask Hardness", &mut s.mask_hardness, 0.0..=1.0, |v| format!("{v:.2}")),
                tips,
                "How crisp the painted mask's edge is: low fades it out softly toward the rim of the ring, high masks \
                 the ring more evenly and faster.",
            );
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
                "On: every pass over the same spot adds more, with no limit. Off: a stroke builds up to a height \
                 and no further, however often you go over the same place in it; lift the pen and stroke again \
                 to build on top.",
            );
        }
        // Layer has no Accumulate, but this still sets how fast it rises.
        if accumulates || brush == Brush::Layer {
            tip(
                theme::slider(ui, "Build-Up Strength", &mut bs.build_up, 0.1..=3.0, |v| format!("{v:.2}")),
                tips,
                "How fast the brush builds, on top of Strength: higher gets to the full effect in fewer passes. On \
                 Layer, how fast the step rises to its height.",
            );
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
                tip(
                    theme::slider(ui, "Clay Depth", &mut s.clay_depth, 0.1..=1.5, |v| format!("{v:.2}")),
                    tips,
                    "How much clay a stroke lays on: with Accumulate off, the height it builds up to, as a share of \
                     the brush's size.",
                );
                tip(
                    theme::slider(ui, "Tip Softness (Flat - Domed)", &mut s.clay_softness, 0.05..=1.0, |v| format!("{v:.2}")),
                    tips,
                    "The shape of the clay under the ring: low lays a flat-topped strip, high a rounded one.",
                );
                tip(
                    theme::slider(ui, "Surface Relax", &mut s.clay_relax, 0.0..=1.0, |v| format!("{v:.2}")),
                    tips,
                    "Smooths away the small ridges the dabs leave at the edge of the stroke, where the surface bends \
                     sharply, without wiping out what the stroke built. 0 leaves them.",
                );
                modes::steady_plane(ui, s.current_mut(), tips);
            }
            Brush::Inflate => modes::relax_normals(ui, s.current_mut(), tips),
            Brush::Crease => {
                tip(
                    theme::slider(ui, "Pinch", &mut s.crease_pinch, 0.0..=1.0, |v| format!("{v:.2}")),
                    tips,
                    "How hard the sides of the cut are drawn in toward its line: higher is a tighter, sharper crease.",
                );
                tip(
                    theme::slider(ui, "Depth", &mut s.crease_depth, 0.05..=1.0, |v| format!("{v:.2}")),
                    tips,
                    "How deep the crease cuts, for the brush's size.",
                );
            }
            Brush::Flatten => {
                tip(
                    theme::slider(ui, "Plane Offset (Lower - Higher)", &mut s.flatten_offset, -0.5..=0.5, |v| format!("{v:.2}")),
                    tips,
                    "Moves the plane Flatten works to below or above the surface under the ring: lower cuts in, \
                     higher leaves a raised plateau.",
                );
                modes::flatten(ui, s.current_mut(), tips);
            }
            Brush::Smooth => modes::smooth(ui, s.current_mut(), tips),
            Brush::Pinch => theme::about(ui, "Draws the surface in to the line of the stroke. Ctrl or the right button spreads it apart."),
            Brush::Nudge => theme::about(ui, "Strength is how far the surface follows the pen: at 100% what is under the middle of the ring stays under it."),
            Brush::EraseLayer => brushes::erase_layer_note(ui, &doc.mesh),
            Brush::Paint => paint::brush_settings(ui, &mut s.paint, &doc.opts.paint, &doc.mesh, &mut doc.opts.tabs, tips, actions),
            Brush::Layer => {
                tip(
                    theme::slider(ui, "Layer Height", &mut s.layer_height, 0.02..=1.0, |v| format!("{v:.2}")),
                    tips,
                    "How high the step Layer raises, as a share of the brush's size. A stroke never goes past it.",
                );
            }
            Brush::Move => {
                tip(
                    theme::checkbox(ui, &mut s.move_connected, "Connected Only", ACCENT()),
                    tips,
                    "Grab only the surface joined to the point under the pointer: moving one finger leaves the one next to it alone.",
                );
                theme::about(ui, "Ctrl or the right button pushes the surface in or pulls it out, the way it faces: drag up for out, down for in. At the outline, drag the way it faces.");
            }
            Brush::Pose => pose::settings(ui, s, tips),
            // Auto Smooth and the rest, below, are theirs.
            Brush::Standard | Brush::SnakeHook => {}
        }
        // What the brush does to leave the surface clean behind it.
        modes::tidy(ui, s.current_mut(), brush, tips);
        stroke::brush_section(ui, s, tips);
        falloff::section(ui, s, tips);
        alpha::section(ui, s, tips, actions);
        // Every setting above back as the brush came, in one press (the same as Reset in its menu).
        ui.add_space(4.0);
        let back = if library.entries()[in_hand].origin == crate::sculpt::library::Origin::User {
            "Put every setting of this brush back as it was when you made it: strength, falloff, alpha,              dabs, pen pressure, Lazy Mouse and the rest. The brush's size is shared and stays as it is."
        } else {
            "Put every setting of this brush back as it came with the app: strength, falloff, alpha, dabs,              pen pressure, Lazy Mouse and the rest. The brush's size is shared and stays as it is."
        };
        if tip(theme::button(ui, ui.available_width(), "Reset Brush", ButtonStyle::default()), tips, back).clicked() {
            actions.push(Action::Brush(crate::sculpt::library::Ask::Reset(in_hand)));
        }
    });
}

/// The Sculpt panel's Mesh tab: what is done to the whole mesh.
fn mesh_page(ui: &mut egui::Ui, doc: &mut Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    symmetry::card(ui, doc, actions);

    let summary = fmt_count(doc.mesh.live_triangle_count());
    theme::category(ui, "Remesh", Color32::from_rgb(115, 217, 128), &summary, true, |ui| {
        // Log slider: equal travel doubles the resolution.
        let mut log = doc.opts.remesh_resolution.log2();
        let range = 4f32.log2()..=crate::remesh::HIGHEST.log2();
        if theme::slider(ui, "Resolution", &mut log, range, |v| format!("{:.0}", v.exp2())).changed() {
            doc.opts.remesh_resolution = log.exp2().round();
        }
        rebuild_notes::remesh_estimate(ui, doc, doc.opts.remesh_resolution);
        remesh_smooth::slider(ui, &mut doc.opts, tips);
        let style = ButtonStyle { badge: "R", active: Some(ACCENT()), ..Default::default() };
        if tip(
            theme::button(ui, ui.available_width(), "Remesh", style),
            tips,
            "Rebuilds the surface with even triangles at this resolution. It keeps both sides as they are, whatever the symmetry. Use it after big shape changes, or to add detail (R).",
        )
        .clicked()
        {
            actions.push(Action::Remesh);
        }
        rebuild_notes::hint(ui, doc, rebuild_notes::Rebuild::Surface, false);
        dyntopo::section(ui, &mut doc.sculptor.dyntopo, tips);
        levels::dyntopo_note(ui, &doc.mesh);

        ui.add_space(6.0);
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
            "Rebuilds the surface as about this many even quads that follow the form. Export keeps the quads.",
        )
        .clicked()
        {
            actions.push(Action::QuadRemesh);
        }
        rebuild_notes::hint(ui, doc, rebuild_notes::Rebuild::Surface, false);
        rebuild_notes::quad_symmetry_hint(ui, doc);

        ui.add_space(6.0);
        decimate::card(ui, doc, tips, actions);
    });
    levels::card(ui, doc, actions);
    layers::card(ui, doc, &state.assets, actions);
    paint::card(ui, doc, &state.assets, actions);
    groups::card(ui, doc, actions);

    theme::category(ui, "Masking", theme::MASK, "", false, |ui| {
        theme::hint(ui, "Masked areas are protected from every brush.");
        let ops = [
            [
                ("Invert", MaskOp::Invert, "What is masked becomes free and what is free becomes masked. One undo step."),
                ("Clear", MaskOp::Clear, "Takes the whole mask away. One undo step."),
            ],
            [
                ("Blur", MaskOp::Blur, "Softens the mask's edge; press again for softer. One undo step."),
                ("Sharpen", MaskOp::Sharpen, "Makes a soft mask edge crisper; press again for crisper. One undo step."),
            ],
            [
                ("Grow", MaskOp::Grow, "Spreads the mask out by one ring of points. One undo step."),
                ("Shrink", MaskOp::Shrink, "Draws the mask in by one ring of points. One undo step."),
            ],
        ];
        for row in ops {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                for (label, op, help) in row {
                    if tip(theme::button(ui, w, label, ButtonStyle::default()), tips, help).clicked() {
                        actions.push(Action::Mask(op));
                    }
                }
            });
        }
        masking::by_shape(ui, tips, actions);
        paint::masking(ui, doc, tips, actions);
    });

    // Faces switched off on the selected object (the box and lasso hide tools).
    let hidden = doc.mesh.hidden_triangle_count();
    let summary = if hidden == 0 { String::new() } else { format!("{} hidden", fmt_count(hidden)) };
    theme::category(ui, "Hidden Parts", Color32::from_rgb(120, 190, 235), &summary, false, |ui| {
    project::card(ui, doc, actions);
        theme::about(ui, "A hidden part is not drawn, and brushes pass over it. Remesh, Join and Export still use the whole model.");
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

/// What stays at the top of the Scene panel whichever tab is showing: the
/// tabs, and the status line.
fn scene_head(ui: &mut egui::Ui, doc: &mut Doc, state: &UiState) {
    tabs::scene_strip(ui, &mut doc.opts.tabs.scene, doc.opts.tooltips);
    if state.status.0.is_empty() {
        return;
    }
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
        .unwrap_or_else(|| "Untitled".into());
    // Starred while there are changes that are not saved, as in the window's title.
    let file = if state.unsaved { format!("{file} *") } else { file };
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
            ("Open...", Action::Open, "Open a saved sculpt (.sclp, or a .sculpt from the Unity app) in place of the scene (Ctrl+O). It asks first only if that would put unsaved work aside; Undo brings this scene back. To add a sculpt's objects to this scene instead, use Import."),
            ("Save", Action::Save, "Save this sculpt (Ctrl+S)."),
            ("Save As...", Action::SaveAs, "Save under a new name (Ctrl+Shift+S)."),
        ]);
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let help = "Add an .obj or .stl model to the scene. An STL comes in at its real size (millimetres, Z up); an OBJ is fitted to the first object's size. Either lands at the centre. A .sclp, or a .sculpt from the Unity app, adds its objects where they were saved.";
            if tip(theme::button(ui, w, "Import...", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::Import);
            }
            let help = "Write the selected object to a file for another program: OBJ, FBX, PLY, or STL for a 3D printer.";
            let export = tip(theme::button(ui, w, "Export...", ButtonStyle::default()), tips, help);
            export_menu(&export, doc, actions);
        });
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
        row(ui, actions, &[(
            "Older Versions...",
            Action::OlderVersions,
            "Earlier saves of this scene, and the recovery copies of the past week, even after Exit Without Saving. Add an older version's objects to the scene to get back something that was deleted, or open it in place of the scene.",
        )]);
        crate::recent::tiles(ui, &mut state.recent, tips, actions);
        // Last, and away from the buttons that are pressed all day.
        row(ui, actions, &[(
            "Exit",
            Action::Exit,
            "Close GolemSculpt, as the window's close button does (Ctrl+Q). It asks first if anything would be lost.",
        )]);
    });
    crate::still::card(ui, doc);
    crate::turntable::card(ui, doc);
    crate::timelapse::card(ui, doc);
}

/// The menu under the File card's Export button: a line per format, each
/// opening its own save box, and the one setting an export has.
fn export_menu(button: &egui::Response, doc: &mut Doc, actions: &mut Vec<Action>) {
    egui::Popup::menu(button).show(|ui| {
        ui.set_min_width(230.0);
        let formats = [
            (
                "OBJ...",
                Action::ExportObj,
                "Write the selected object as a Wavefront OBJ, placed as it is in the scene (position, rotation and scale included). A painted object goes with its colour at every point; its materials need PLY or FBX.",
            ),
            (
                "FBX (Blender, Unity)...",
                Action::ExportFbx,
                "Write every selected object into one Autodesk FBX, for Blender, Unity and the like: each an object of its own under its name, placed as it is in the scene, Y up, at real size (1 unit is 1 cm). A painted object goes with its colour, and each face with the material most of it is painted with. Ctrl + click rows of the Objects list to select more than one.",
            ),
            (
                "PLY (painted colour)...",
                Action::ExportPly,
                "Write the selected object as a PLY, placed as it is in the scene: the format most programs read painted colour from (MeshLab, colour 3D printing). A painted object goes with its colour and, for each material it is painted with, how much of it is at each point.",
            ),
            (
                "STL for Printing (mm)...",
                Action::ExportStl,
                "Write the selected object as a binary STL for a slicer: placed as it is in the scene, upright (Z up), in millimetres at real size (1 unit is 10 mm). If the object has holes it says so first, and asks.",
            ),
        ];
        for (label, action, help) in formats {
            if ui.button(label).on_hover_text(help).clicked() {
                actions.push(action);
                ui.close();
            }
        }
        ui.separator();
        let help = "Export FBX also writes how much of each painted material is at each point into the object's UV sets 1 to 4 (two materials a set), for a game shader of your own to blend textures by. Unity reads them as uv2 to uv5. Off: the FBX has its colour and one material per face, which is all Blender and Unity use by themselves.";
        ui.checkbox(&mut doc.opts.fbx_material_weights, "FBX: material weights in UVs").on_hover_text(help);
        // The list that shows the selection is on another tab, so say here what the file would hold.
        let picked = doc.scene.selected().len();
        if picked > 1 {
            theme::hint(ui, &format!("{picked} objects are selected. FBX writes them all into one file; OBJ, PLY and STL write the bright one only."));
        }
    });
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
    let o = &mut doc.opts;
    materials::card(ui, &o.materials, &mut o.paint, &mut state.materials, tips, actions);
    look::lighting(ui, &mut doc.opts, actions);
    look::effects(ui, &mut doc.opts, actions);
}
