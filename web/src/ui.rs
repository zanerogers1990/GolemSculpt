//! The web version's interface, laid out as the desktop app's: a Sculpt
//! panel floating down the left (its Brush / Mesh / Tools tabs), a Scene
//! panel down the right (Objects / Look / File / Settings), fold tabs on
//! their inner edges, the view gizmo top right and the display switches
//! along the bottom.
//!
//! It is built from the desktop's own pieces wherever they stand alone: the
//! tool kit's theme and widgets, and the desktop files compiled here as they
//! are (`#[path]` below): the panel frames, the tab strips, the Size slider,
//! the Stroke card, the brush modes and the Falloff card. What is written
//! here follows the desktop's `ui/mod.rs`, `ui/brushes.rs` and
//! `ui/overlay.rs`, cut down to what the web version has.

use std::collections::HashMap;

use eframe::egui::{self, Align2, Color32, Id, Order, Rect, Sense, Stroke, TextureId, Ui, pos2, vec2};
use glam::Vec3;

use super::{GolemApp, Mode};
use crate::mesh::primitives::Primitive;
use crate::sculpt::library::Origin;
use crate::sculpt::{Brush, Settings};

/// The tool kit's theme, under the name the desktop's panel files use.
pub mod theme {
    pub use sculpt_toolkit::theme::*;
}
use theme::{ACCENT, ButtonStyle};

/// A tooltip, when tooltips are on (the desktop's `ui::tip`).
pub fn tip(response: egui::Response, on: bool, text: &str) -> egui::Response {
    if on { sculpt_toolkit::translate::hover(response, text) } else { response }
}

/// The height of the view gizmo's handles, which the panels make room for.
mod view_handles {
    pub const SIZE: f32 = 26.0;
}

// The web version uses only some of what the desktop file offers.
#[allow(dead_code)]
#[path = "../../src/ui/brush_size.rs"]
mod brush_size;
#[allow(dead_code)]
#[path = "../../src/ui/dyntopo.rs"]
mod dyntopo;
#[path = "../../src/ui/falloff.rs"]
mod falloff;
#[path = "../../src/ui/modes.rs"]
mod modes;
#[path = "../../src/ui/panels.rs"]
mod panels;
#[path = "../../src/ui/stroke.rs"]
mod stroke;
// The web version uses only some of what the desktop file offers.
#[allow(dead_code)]
#[path = "../../src/ui/tabs.rs"]
pub mod tabs;

use panels::{Part, Side};

/// The kinds the Brushes card sorts brushes into, as on the desktop, less
/// the brushes the web version has not got (paint, Pose).
const GROUPS: [(&str, &[Brush]); 5] = [
    ("Clay & Build", &[Brush::Clay, Brush::Standard, Brush::Layer, Brush::Inflate]),
    ("Smooth & Flatten", &[Brush::Smooth, Brush::Flatten]),
    ("Cut & Pinch", &[Brush::Crease, Brush::Pinch]),
    ("Move", &[Brush::Move, Brush::SnakeHook, Brush::Nudge]),
    ("Layers", &[Brush::EraseLayer]),
];
const COLUMNS: usize = 2;
const GAP: f32 = 4.0;
const ROW: f32 = 24.0;

/// The groups the Tools tab sorts the tools into, by the tools' ids, each
/// under a heading that folds it away, as on the desktop. A tool's name,
/// icon, key and tooltip are its own (`Tool::info`).
const TOOLS: [(&str, &[&str]); 3] = [
    ("Make", &["tube", "muscle", "strap", "lathe", "hands", "array", "extract"]),
    ("Mask, Hide & Cut", &["mask", "hide", "trim", "split"]),
    ("Measure", &["measure"]),
];

/// The matcaps on the Look tab: name, picture, who made it.
pub const MATCAPS: [(&str, &[u8]); 8] = [
    ("Teal Matte", include_bytes!("../../assets/matcaps/GolemSculpt/Teal Matte.png")),
    ("Clay 1", include_bytes!("../../assets/matcaps/Ben Simonds/MatCap_BS1.jpg")),
    ("Clay 2", include_bytes!("../../assets/matcaps/Ben Simonds/MatCap_BS2.jpg")),
    ("Clay 3", include_bytes!("../../assets/matcaps/Ben Simonds/MatCap_BS3.jpg")),
    ("Clay 4", include_bytes!("../../assets/matcaps/Ben Simonds/MatCap_BS4.jpg")),
    ("Clay 5", include_bytes!("../../assets/matcaps/Ben Simonds/MatCap_BS5.jpg")),
    ("Clay 6", include_bytes!("../../assets/matcaps/Ben Simonds/MatCap_BS6.jpg")),
    ("Clay 7", include_bytes!("../../assets/matcaps/Ben Simonds/MatCap_BS7.jpg")),
];

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../../assets/icons/", $name, ".png")) as &[u8])),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "brush_move", "brush_clay", "brush_smooth", "brush_crease", "brush_inflate", "brush_flatten",
    "brush_standard", "brush_layer", "brush_snakehook", "brush_pinch", "brush_nudge", "brush_mask", "brush_erase_layer",
    "polarity_add", "polarity_sub", "undo", "redo", "tool_transform", "eye", "eye_off", "trash", "prim_sphere", "prim_cube", "prim_cylinder",
    "prim_capsule", "prim_cone", "tool_sculpt", "tool_tube", "tool_muscle", "tool_strap", "mirror",
    "tool_lathe", "tool_hands", "tool_array", "tool_measure", "check", "close", "region_box_mask",
    "region_lasso_mask", "region_box_hide", "region_lasso_hide", "region_box_trim", "region_lasso_trim",
    "region_line_trim", "region_box_split", "region_lasso_split", "region_line_split", "region_invert",
    "region_show_all",
];

/// The interface's pictures: the icons and the matcaps, as egui textures.
pub struct Pictures {
    icons: HashMap<&'static str, egui::TextureHandle>,
    pub matcaps: Vec<(egui::TextureHandle, image::RgbaImage)>,
}

impl Pictures {
    pub fn load(ctx: &egui::Context) -> Pictures {
        let texture = |name: &str, image: &image::RgbaImage| {
            let size = [image.width() as usize, image.height() as usize];
            ctx.load_texture(name, egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()), egui::TextureOptions::LINEAR)
        };
        let decode = |bytes: &[u8]| image::load_from_memory(bytes).ok().map(|i| i.to_rgba8());
        let icons = ICONS.iter().filter_map(|&(name, bytes)| Some((name, texture(name, &decode(bytes)?)))).collect();
        let matcaps = MATCAPS
            .iter()
            .filter_map(|&(name, bytes)| {
                let image = decode(bytes)?;
                Some((texture(name, &image), image))
            })
            .collect();
        Pictures { icons, matcaps }
    }

    pub fn icon(&self, name: &str) -> Option<TextureId> {
        self.icons.get(name).map(|t| t.id())
    }
}

/// What the interface remembers between frames.
pub struct UiState {
    /// The colour theme (`theme::THEMES`) and the interface's size, kept between visits.
    pub theme: usize,
    pub scale: f32,
    pub folded: [bool; 2],
    pub sculpt_tab: tabs::Sculpt,
    pub scene_tab: tabs::Scene,
    pub tips: bool,
    pub pictures: Pictures,
}

impl UiState {
    pub fn new(ctx: &egui::Context) -> UiState {
        let theme = crate::browser::pref("theme").parse().unwrap_or(0usize).min(theme::THEMES.len() - 1);
        let scale = crate::browser::pref("scale").parse().unwrap_or(1.0f32).clamp(0.75, 2.0);
        theme::set_theme(ctx, theme);
        ctx.set_zoom_factor(scale);
        UiState {
            theme,
            scale,
            folded: [false; 2],
            sculpt_tab: tabs::Sculpt::Brush,
            scene_tab: tabs::Scene::Objects,
            tips: true,
            pictures: Pictures::load(ctx),
        }
    }
}

fn fmt_count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.0}k", n as f32 / 1e3),
        _ => format!("{:.2}M", n as f32 / 1e6),
    }
}

impl GolemApp {
    /// The whole interface for one frame, over the viewport.
    pub(super) fn interface(&mut self, root: &mut Ui) {
        let ctx = root.ctx().clone();
        let screen = ctx.content_rect();
        // Ctrl + = and Ctrl + - change the size too: keep the setting in step.
        let zoom = ctx.zoom_factor();
        if (zoom - self.ui.scale).abs() > 0.001 {
            self.ui.scale = zoom;
            crate::browser::set_pref("scale", &zoom.to_string());
        }
        let mut folded = self.ui.folded;
        sculpt_toolkit::floating::set_room(&ctx, panels::room_for_cards(screen, folded));

        let page = self.ui.sculpt_tab.name();
        panels::panel(root, Side::Left, folded[0], page, &mut |ui, part| match part {
            Part::Head => self.sculpt_head(ui),
            Part::Body => self.sculpt_body(ui),
        });
        let page = self.ui.scene_tab.name();
        panels::panel(root, Side::Right, folded[1], page, &mut |ui, part| match part {
            Part::Head => {
                let mut showing = self.ui.scene_tab;
                tabs::scene_strip(ui, &mut showing, self.ui.tips);
                self.ui.scene_tab = showing;
            }
            Part::Body => self.scene_body(ui),
        });
        for side in [Side::Left, Side::Right] {
            let away = &mut folded[side.index()];
            *away ^= panels::tab(&ctx, side, *away, self.ui.tips);
        }
        self.ui.folded = folded;

        self.view_gizmo(&ctx, panels::gizmo_origin(screen, folded[1]));
        self.bottom_bar(&ctx, panels::bottom_bar_origin(screen, folded[0]), panels::bottom_bar_width(screen, folded));
    }

    // ---- the Sculpt panel ---------------------------------------------------------

    fn sculpt_head(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, "GolemSculpt", theme::bold(19.0), theme::TEXT());
        ui.painter().text(
            rect.right_center(),
            Align2::RIGHT_CENTER,
            format!("{} tris", fmt_count(self.mesh.live_triangle_count())),
            theme::font(11.0),
            theme::HINT(),
        );

        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let p = &self.ui.pictures;
            let (undo_icon, redo_icon) = (p.icon("undo"), p.icon("redo"));
            let (can_undo, can_redo) = (self.can_undo(), self.can_redo());
            ui.add_enabled_ui(can_undo, |ui| {
                let style = ButtonStyle { icon: undo_icon, badge: "Ctrl+Z", ..Default::default() };
                if tip(theme::button(ui, w, "Undo", style), tips, "Step back (Ctrl+Z)").clicked() {
                    self.undo_redo(true);
                }
            });
            ui.add_enabled_ui(can_redo, |ui| {
                let style = ButtonStyle { icon: redo_icon, badge: "Ctrl+Y", ..Default::default() };
                if tip(theme::button(ui, w, "Redo", style), tips, "Step forward (Ctrl+Shift+Z or Ctrl+Y)").clicked() {
                    self.undo_redo(false);
                }
            });
        });

        if self.mode == Mode::Sculpt {
            let mut showing = self.ui.sculpt_tab;
            tabs::sculpt_strip(ui, &mut showing, tips);
            self.ui.sculpt_tab = showing;
            if showing == tabs::Sculpt::Brush {
                let icon = |name: &str| self.ui.pictures.icon(name);
                brush_plate(ui, &mut self.sculptor.settings, &icon, tips);
            }
        }
    }

    fn sculpt_body(&mut self, ui: &mut Ui) {
        if self.mode == Mode::Tubes {
            self.tool_buttons(ui);
            self.tubes_panel(ui);
            return;
        }
        if self.mode == Mode::Transform {
            self.tool_buttons(ui);
            self.transform_card(ui);
            return;
        }
        match self.ui.sculpt_tab {
            tabs::Sculpt::Brush => self.brush_page(ui),
            tabs::Sculpt::Mesh => self.mesh_page(ui),
            tabs::Sculpt::Tools => {
                self.tool_buttons(ui);
                theme::hint(ui, "A tool takes the panel while it is up; Esc, its key again or Back to Sculpt puts it away.");
            }
        }
    }

    /// The tools, a button each (and Sculpt, to go back), as the desktop's
    /// Tools tab: in groups, each with its icon and its desktop key.
    fn tool_buttons(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        // The tools' panels draw with the same icons, and tooltips when they are on.
        self.tubes.tips = tips;
        if self.tubes.icons.is_empty() {
            for &(name, _) in ICONS {
                if let Some(id) = self.ui.pictures.icon(name) {
                    self.tubes.icons.insert(name, id);
                }
            }
        }
        let full = ui.available_width();
        let style = ButtonStyle {
            icon: self.ui.pictures.icon("tool_sculpt"),
            active: (self.mode == Mode::Sculpt).then_some(ACCENT()),
            badge: "Esc",
            ..Default::default()
        };
        if tip(theme::button(ui, full, "Sculpt", style), tips, "Back to the brushes (Esc).").clicked() {
            self.mode = Mode::Sculpt;
        }
        let style = ButtonStyle {
            icon: self.ui.pictures.icon("tool_transform"),
            active: (self.mode == Mode::Transform).then_some(ACCENT()),
            badge: "W",
            ..Default::default()
        };
        if tip(theme::button(ui, full, "Transform", style), tips, "Move, turn and scale the selected objects (W).").clicked() {
            self.toggle_transform();
        }
        let up = (self.mode == Mode::Tubes).then_some(self.tubes.which);
        let mut picked = None;
        for (title, ids) in TOOLS {
            let members: Vec<usize> = ids.iter().filter_map(|id| self.tubes.index_of(id)).collect();
            let holds_up = up.is_some_and(|i| members.contains(&i));
            if !theme::fold(ui, title, true, holds_up) {
                continue;
            }
            for pair in members.chunks(2) {
                ui.horizontal(|ui| {
                    let w = theme::row_width(ui, 2, 6.0);
                    for &i in pair {
                        let Some(info) = self.tubes.info(i) else { continue };
                        let style = ButtonStyle {
                            icon: self.ui.pictures.icon(info.icon),
                            active: (up == Some(i)).then_some(ACCENT()),
                            badge: info.hotkey.map_or("", |key| key.name()),
                            ..Default::default()
                        };
                        if tip(theme::button(ui, w, info.name, style), tips, info.tip).clicked() {
                            picked = Some(i);
                        }
                    }
                });
            }
        }
        if let Some(i) = picked {
            self.pick_tool(i);
        }
    }

    pub(super) fn pick_tool(&mut self, i: usize) {
        if self.mode == Mode::Tubes && self.tubes.which == i {
            self.mode = Mode::Sculpt;
        } else {
            self.tubes.which = i;
            self.mode = Mode::Tubes;
        }
    }

    fn brush_page(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let pictures = &self.ui.pictures;
        let icon = |name: &str| pictures.icon(name);
        let (library, s, pen) = (&mut self.sculptor.library, &mut self.sculptor.settings, &self.sculptor.pen_meter);

        theme::category(ui, "Stroke", Color32::from_rgb(168, 133, 255), &stroke::summary(s), false, |ui| {
            stroke::card(ui, s, pen, tips)
        });

        // The Brushes card: every brush the web version has, by kind.
        let in_hand = library.selected(s);
        let summary = if s.mask_mode { "Mask".to_owned() } else { library.entries()[in_hand].name.clone() };
        let mut pick = None;
        theme::category(ui, "Brushes", theme::lit(ACCENT()), &summary, true, |ui| {
            ui.spacing_mut().item_spacing = vec2(GAP, GAP);
            let width = theme::row_width(ui, COLUMNS, GAP);
            let entries = library.entries();
            for (title, kernels) in GROUPS {
                let list: Vec<usize> = kernels
                    .iter()
                    .flat_map(|&k| (0..entries.len()).filter(move |&i| entries[i].kernel == k))
                    .filter(|&i| entries[i].origin != Origin::User)
                    .collect();
                if theme::fold(ui, title, true, false) {
                    for line in list.chunks(COLUMNS) {
                        ui.horizontal(|ui| {
                            for &i in line {
                                let e = &entries[i];
                                let lit = !s.mask_mode && in_hand == i;
                                if row(ui, width, icon(e.kernel.icon()), &e.name, lit).clicked() {
                                    pick = Some(i);
                                }
                            }
                        });
                    }
                }
            }
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            theme::about(ui, "Tap a brush to take it up. Every brush keeps its own settings, and changes stick as you make them.");
        });
        if let Some(i) = pick {
            library.select(i, s);
            s.mask_mode = false;
        }

        let name = library.entries()[library.selected(s)].name.clone();
        let brush = s.brush;
        let title = if s.mask_mode { "Mask Settings".to_owned() } else { format!("{name} Settings") };
        theme::category_as(ui, "Brush Settings", &title, Color32::from_rgb(89, 209, 230), "", true, |ui| {
            if s.mask_mode {
                tip(
                    theme::slider(ui, "Mask Hardness", &mut s.mask_hardness, 0.0..=1.0, |v| format!("{v:.2}")),
                    tips,
                    "How crisp the painted mask's edge is.",
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
                    "On: every pass over the same spot adds more. Off: a stroke builds up to a height and no further.",
                );
            }
            if accumulates || brush == Brush::Layer {
                tip(
                    theme::slider(ui, "Build-Up Strength", &mut bs.build_up, 0.1..=3.0, |v| format!("{v:.2}")),
                    tips,
                    "How fast the brush builds, on top of Strength.",
                );
            }
            let label = if bs.front_facing { "Front Facing Only" } else { "Front Facing Only (Off)" };
            tip(
                theme::checkbox(ui, &mut bs.front_facing, label, ACCENT()),
                tips,
                "Ignore surface that faces away from the camera, so thin shapes are not hit on the far side.",
            );
            match brush {
                Brush::Clay => {
                    theme::slider(ui, "Clay Depth", &mut s.clay_depth, 0.1..=1.5, |v| format!("{v:.2}"));
                    theme::slider(ui, "Tip Softness (Flat - Domed)", &mut s.clay_softness, 0.05..=1.0, |v| format!("{v:.2}"));
                    theme::slider(ui, "Surface Relax", &mut s.clay_relax, 0.0..=1.0, |v| format!("{v:.2}"));
                    modes::steady_plane(ui, s.current_mut(), tips);
                }
                Brush::Inflate => modes::relax_normals(ui, s.current_mut(), tips),
                Brush::Crease => {
                    theme::slider(ui, "Pinch", &mut s.crease_pinch, 0.0..=1.0, |v| format!("{v:.2}"));
                    theme::slider(ui, "Depth", &mut s.crease_depth, 0.05..=1.0, |v| format!("{v:.2}"));
                }
                Brush::Flatten => {
                    theme::slider(ui, "Plane Offset (Lower - Higher)", &mut s.flatten_offset, -0.5..=0.5, |v| format!("{v:.2}"));
                    modes::flatten(ui, s.current_mut(), tips);
                }
                Brush::Smooth => modes::smooth(ui, s.current_mut(), tips),
                Brush::Pinch => theme::about(ui, "Draws the surface in to the line of the stroke. Ctrl or the right button spreads it apart."),
                Brush::Nudge => theme::about(ui, "Strength is how far the surface follows the pen."),
                Brush::Layer => {
                    theme::slider(ui, "Layer Height", &mut s.layer_height, 0.02..=1.0, |v| format!("{v:.2}"));
                }
                Brush::Move => {
                    theme::checkbox(ui, &mut s.move_connected, "Connected Only", ACCENT());
                    theme::about(ui, "Ctrl or the right button pushes the surface in or pulls it out, the way it faces.");
                }
                _ => {}
            }
            modes::tidy(ui, s.current_mut(), brush, tips);
            stroke::brush_section(ui, s, tips);
            falloff::section(ui, s, tips);
            super::alpha::section(ui, s, tips);
        });
    }

    fn mesh_page(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        // Straight down the view, in the model's space: what a custom radial axis is aimed along.
        let down_the_view = self.camera.rotation() * Vec3::NEG_Z;
        let s = &mut self.sculptor.settings;
        let on: Vec<&str> = ["X", "Y", "Z"].into_iter().zip(s.symmetry).filter(|(_, on)| *on).map(|(a, _)| a).collect();
        let mut summary = on.join(" ");
        if let Some(radial) = super::radial::summary(&s.radial) {
            summary = if summary.is_empty() { radial } else { format!("{summary} \u{b7} {radial}") };
        }
        if summary.is_empty() {
            summary = "Off".to_owned();
        }
        theme::category(ui, "Symmetry", Color32::from_rgb(242, 166, 38), &summary, true, |ui| {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 3, 4.0);
                for (i, axis) in ["X", "Y", "Z"].into_iter().enumerate() {
                    let style = ButtonStyle {
                        icon: self.ui.pictures.icon("mirror"),
                        active: s.symmetry[i].then_some(theme::AXIS[i]),
                        badge: if i == 0 { "X" } else { "" },
                        ..Default::default()
                    };
                    let help = format!("Mirror strokes across {axis}: one stroke works both sides.");
                    if tip(theme::button(ui, w, axis, style), tips, &help).clicked() {
                        s.symmetry[i] = !s.symmetry[i];
                    }
                }
            });
            super::radial::section(ui, &mut s.radial, tips, down_the_view);
        });

        // Invert, Clear, Blur, Sharpen, Grow, Shrink and masks from the surface (masking.rs).
        self.masking_card(ui);

        let tris = fmt_count(self.mesh.live_triangle_count());
        theme::category(ui, "Remesh", Color32::from_rgb(110, 200, 120), &tris, true, |ui| {
            let mut resolution = self.remesh_resolution as f32;
            if tip(
                theme::slider(ui, "Resolution", &mut resolution, 64.0..=512.0, |v| format!("{v:.0}")),
                tips,
                "How fine the new mesh is: voxels along the model's longest side.",
            )
            .changed()
            {
                self.remesh_resolution = resolution.round() as u32;
            }
            theme::hint(ui, "Rebuilds the surface as an even mesh, fusing anything that overlaps.");
            let full = ui.available_width();
            if tip(theme::button(ui, full, "Remesh", ButtonStyle { active: Some(ACCENT()), ..Default::default() }), tips, "Remesh now.")
                .clicked()
            {
                self.remesh();
            }
            // Dynamic topology sits here, as on the desktop: the other way to
            // get triangles where they are wanted.
            dyntopo::section(ui, &mut self.sculptor.dyntopo, tips);
            self.dyntopo_levels_note(ui);
        });

        // Subdivision levels and sculpt layers, as the desktop's cards (levels.rs, layers.rs).
        self.levels_card(ui);
        self.layers_card(ui);

        // Decimate, as the desktop's card (without its preview).
        theme::category(ui, "Decimate", Color32::from_rgb(232, 120, 120), "", false, |ui| {
            let range = crate::decimate::TARGET_RANGE;
            let now = self.mesh.live_triangle_count();
            let mut log = self.decimate_target.clamp(*range.start(), *range.end()).log2();
            let percent = |target: f32| if now > 0 { format!(" ({:.0}%)", (target / now as f32 * 100.0).min(999.0)) } else { String::new() };
            let slider = theme::slider(ui, "Target Triangles", &mut log, range.start().log2()..=range.end().log2(), |v| {
                format!("{}{}", fmt_count(v.exp2() as usize), percent(v.exp2()))
            });
            if tip(slider, tips, "How many triangles Decimate brings the model down to. In brackets: what share of the triangles it has now.")
                .changed()
            {
                self.decimate_target = two_figures(log.exp2()).clamp(*range.start(), *range.end());
            }
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 4, 4.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for share in [0.75f32, 0.5, 0.25, 0.1] {
                    let label = format!("{:.0}%", share * 100.0);
                    let help = format!("Sets Target Triangles to {label} of the triangles the model has now.");
                    if tip(theme::button(ui, w, &label, ButtonStyle::default()), tips, &help).clicked() {
                        self.decimate_target = two_figures(now as f32 * share).clamp(*range.start(), *range.end());
                    }
                }
            });
            let target = self.decimate_target.round() as usize;
            let full = ui.available_width();
            ui.add_enabled_ui(now > target, |ui| {
                let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
                let help = "Takes triangles off and keeps the shape: from where the surface is flat, kept where it bends. A closed \
                            model stays closed, a mirror stays exact, fully masked surface is left as it is. Undo brings the \
                            triangles back.";
                if tip(theme::button(ui, full, "Decimate", style), tips, help).clicked() {
                    self.decimate();
                }
            });
            if now > 0 && now <= target {
                theme::hint(ui, "The model is at or under the target already.");
            }
        });
    }

    // ---- the Scene panel ------------------------------------------------------------

    fn scene_body(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        match self.ui.scene_tab {
            tabs::Scene::Objects => {
                self.objects_card(ui);
                theme::category(ui, "New Shape", Color32::from_rgb(110, 200, 120), self.shape.name(), true, |ui| {
                    let shapes = [Primitive::Sphere, Primitive::Cube, Primitive::Cylinder, Primitive::Capsule, Primitive::Cone];
                    for line in shapes.chunks(2) {
                        ui.horizontal(|ui| {
                            let w = theme::row_width(ui, 2, 6.0);
                            for &kind in line {
                                let style = ButtonStyle {
                                    icon: self.ui.pictures.icon(kind.icon()),
                                    active: (self.shape == kind).then_some(ACCENT()),
                                    ..Default::default()
                                };
                                if theme::button(ui, w, kind.name(), style).clicked() {
                                    self.shape = kind;
                                }
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        let w = theme::row_width(ui, 3, 4.0);
                        for (d, name) in [(30_000, "Light"), (120_000, "Medium"), (500_000, "Dense")] {
                            let style = ButtonStyle { active: (self.density == d).then_some(ACCENT()), ..Default::default() };
                            if theme::button(ui, w, name, style).clicked() {
                                self.density = d;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        let w = theme::row_width(ui, 2, 6.0);
                        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
                        if tip(theme::button(ui, w, "Add Object", style), tips, "This shape as a new object, in the middle of the scene.")
                            .clicked()
                        {
                            self.add_shape();
                        }
                        let about = "This shape in the active object's place. Ctrl+Z brings the old one back.";
                        if tip(theme::button(ui, w, "Replace", ButtonStyle::default()), tips, about).clicked() {
                            self.new_shape();
                        }
                    });
                    theme::hint(ui, "The objects are kept in this browser between visits.");
                });
            }
            tabs::Scene::Look => {
                let picked = MATCAPS.get(self.matcap).map_or("", |m| m.0);
                theme::category(ui, "Matcap", Color32::from_rgb(89, 209, 230), picked, true, |ui| {
                    let size = (ui.available_width() - 3.0 * 6.0) / 4.0;
                    let mut choice = None;
                    for (row_start, line) in self.ui.pictures.matcaps.chunks(4).enumerate() {
                        ui.horizontal(|ui| {
                            for (j, (texture, _)) in line.iter().enumerate() {
                                let i = row_start * 4 + j;
                                let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
                                ui.painter().image(texture.id(), rect.shrink(2.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                                if i == self.matcap {
                                    ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, ACCENT()), egui::StrokeKind::Inside);
                                } else if response.hovered() {
                                    ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, theme::HINT()), egui::StrokeKind::Inside);
                                }
                                if tip(response, tips, MATCAPS[i].0).clicked() {
                                    choice = Some(i);
                                }
                            }
                        });
                    }
                    if let Some(i) = choice {
                        self.matcap = i;
                        crate::browser::set_pref("matcap", &i.to_string());
                    }
                    theme::hint(ui, "Teal Matte is GolemSculpt's own; Clay 1 to 7 are Ben Simonds' (CC BY 3.0).");
                });
                // The finish, backdrop, lens, display switches and Save Picture (look.rs).
                self.look_cards(ui);
            }
            tabs::Scene::File => {
                theme::category(ui, "File", theme::lit(ACCENT()), "", true, |ui| {
                    let full = ui.available_width();
                    if tip(theme::button(ui, full, "Open...", ButtonStyle::default()), tips, "Open a scene saved with Save Scene, or an OBJ, STL or PLY file (as a new object).").clicked() {
                        self.open_picker();
                    }
                    let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
                    let help = "Downloads the whole scene as one .golem file: every object with its place, mask, subdivision levels and layers, and the view. Open it here again, in any browser, to carry on.";
                    if tip(theme::button(ui, full, "Save Scene", style), tips, help).clicked() {
                        self.save_scene();
                    }
                    ui.horizontal(|ui| {
                        let w = theme::row_width(ui, 2, 6.0);
                        if tip(theme::button(ui, w, "Save OBJ", ButtonStyle::default()), tips, "Download the model as OBJ.").clicked() {
                            self.save_file(false);
                        }
                        if tip(theme::button(ui, w, "Save STL", ButtonStyle::default()), tips, "Download the model as STL, for printing.")
                            .clicked()
                        {
                            self.save_file(true);
                        }
                    });
                    theme::hint(ui, "Save Scene keeps everything; OBJ and STL keep only the shapes, for other programs.");
                    theme::hint(ui, "Or drop a .golem, OBJ, STL or PLY file anywhere on the page.");
                });
            }
            tabs::Scene::Settings => {
                self.theme_card(ui);
                let summary = format!("{:.0}%", self.ui.scale * 100.0);
                theme::category(ui, "Interface", Color32::from_rgb(242, 158, 89), &summary, true, |ui| {
                    ui.horizontal(|ui| {
                        let sizes = [0.75f32, 1.0, 1.25, 1.5, 2.0];
                        let w = theme::row_width(ui, sizes.len(), 4.0);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for scale in sizes {
                            let style = ButtonStyle { active: ((self.ui.scale - scale).abs() < 0.01).then_some(ACCENT()), ..Default::default() };
                            if theme::button(ui, w, &format!("{:.0}%", scale * 100.0), style).clicked() {
                                self.ui.scale = scale;
                                ui.ctx().set_zoom_factor(scale);
                                crate::browser::set_pref("scale", &scale.to_string());
                            }
                        }
                    });
                    theme::hint(ui, "Ctrl + = and Ctrl + - also work.");
                    let label = if self.ui.tips { "Tooltips: On" } else { "Tooltips: Off" };
                    tip(theme::checkbox(ui, &mut self.ui.tips, label, ACCENT()), tips, "Say what a button does when the pointer rests on it.");
                });
                theme::category(ui, "How to Use", Color32::from_rgb(110, 200, 120), "", true, |ui| {
                    for line in [
                        "Draw on the model to sculpt. A pen's pressure is read.",
                        "Ctrl or the right button: the brush's opposite. Shift: smooth.",
                        "Drag off the model, or Alt + drag: turn the view.",
                        "Middle drag, or Shift + drag off the model: move the view.",
                        "Wheel or pinch: zoom. F twice: frame the model.",
                        "Hold S, F or D and move: brush size, strength, focal shift. [ and ]: size too.",
                        "Hold Space: the brush wheel. 1 to 0: brushes. M: mask. X: mirror across X.",
                        "T, B, E: Tube, Muscle, Strap. Y: Array. N: Mask. K: Split. L: Measure. Esc: back to sculpting.",
                        "Ctrl+Z, Ctrl+Shift+Z: undo, redo.",
                        "Page Up, Page Down: subdivision levels. Ctrl+D: subdivide. F1: the welcome card.",
                    ] {
                        theme::hint(ui, line);
                    }
                    ui.add_space(4.0);
                    let full = ui.available_width();
                    if tip(theme::button(ui, full, "Show the Welcome Card", ButtonStyle::default()), tips, "The short guide shown on a first visit.").clicked() {
                        self.open_welcome();
                    }
                });
                theme::category(ui, "About", Color32::from_rgb(150, 150, 170), "", false, |ui| {
                    theme::hint(ui, "GolemSculpt for the web. It is built with open-source libraries and uses Ben Simonds' matcaps (CC BY 3.0); their licences are in the file below.");
                    let full = ui.available_width();
                    if tip(theme::button(ui, full, "Third-Party Licences", ButtonStyle::default()), tips, "Opens the licences of the libraries, pictures and fonts GolemSculpt uses, in a new tab.").clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab("THIRD-PARTY-LICENSES.txt"));
                    }
                });
            }
        }
    }

    /// The colour themes, a tile each in its own colours (the desktop's Theme card).
    fn theme_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let themes = &theme::THEMES;
        let showing = self.ui.theme.min(themes.len() - 1);
        let mut picked = None;
        theme::category(ui, "Theme", Color32::from_rgb(214, 140, 255), themes[showing].name, true, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let w = theme::row_width(ui, 2, 6.0);
            for (row, pair) in themes.chunks(2).enumerate() {
                ui.horizontal(|ui| {
                    for (column, palette) in pair.iter().enumerate() {
                        let index = row * 2 + column;
                        let help = format!("{}: the interface in these colours.", palette.name);
                        if tip(swatch(ui, w, palette, index == showing), tips, &help).clicked() {
                            picked = Some(index);
                        }
                    }
                });
            }
            theme::hint(ui, "The backdrop behind the model goes with the theme.");
        });
        if let Some(index) = picked {
            self.ui.theme = index;
            theme::set_theme(ui.ctx(), index);
            crate::browser::set_pref("theme", &index.to_string());
        }
    }

    // ---- over the viewport --------------------------------------------------------------

    /// The axis bubbles (a tap turns to that view, a drag orbits), then
    /// Persp / Ortho and Frame, as the desktop's view gizmo.
    fn view_gizmo(&mut self, ctx: &egui::Context, origin: egui::Pos2) {
        let tips = self.ui.tips;
        egui::Area::new(Id::new("view_gizmo")).order(Order::Middle).fade_in(false).fixed_pos(origin).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(108.0), Sense::click_and_drag());
            let p = ui.painter();
            let center = rect.center();
            if response.hovered() || response.dragged() {
                p.circle_filled(center, 52.0, Color32::from_black_alpha(102));
            }
            let inv = self.camera.rotation().inverse();
            let mut bubbles: Vec<(usize, bool, Vec3)> = Vec::new();
            for axis in 0..3 {
                let mut dir = Vec3::ZERO;
                dir[axis] = 1.0;
                bubbles.push((axis, true, inv * dir));
                bubbles.push((axis, false, inv * -dir));
            }
            bubbles.sort_by(|a, b| a.2.z.total_cmp(&b.2.z));
            let under = response.hover_pos().and_then(|q| {
                let distance = |v: Vec3| q.distance(center + vec2(v.x, -v.y) * 37.0);
                bubbles
                    .iter()
                    .filter(|b| distance(b.2) < 16.0)
                    .min_by(|a, b| (distance(a.2) - a.2.z * 4.0).total_cmp(&(distance(b.2) - b.2.z * 4.0)))
                    .map(|b| (b.0, b.1))
            });
            if response.dragged() {
                let d = response.drag_delta();
                self.camera.orbit(glam::Vec2::new(d.x, d.y), None);
            }
            for &(axis, positive, v) in &bubbles {
                let at = center + vec2(v.x, -v.y) * 37.0;
                let depth = v.z * 0.5 + 0.5;
                let mut scale = 0.84 + (1.08 - 0.84) * depth;
                if under == Some((axis, positive)) {
                    scale *= 1.14;
                }
                let color = theme::AXIS[axis].gamma_multiply(0.55 + 0.45 * depth);
                let letter = ["X", "Y", "Z"][axis];
                if positive {
                    p.line_segment([center, at], Stroke::new(3.0, color));
                    p.circle_filled(at, 9.0 * scale, color);
                    p.text(at, Align2::CENTER_CENTER, letter, theme::bold(10.0), Color32::from_black_alpha(220));
                } else {
                    let [r, g, b] = theme::palette().backdrop.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
                    p.circle_filled(at, 8.0 * scale, Color32::from_rgb(r, g, b));
                    p.circle_stroke(at, 8.0 * scale, Stroke::new(1.5, color));
                }
            }
            if response.clicked() {
                if let Some((axis, positive)) = under {
                    let (mut yaw, mut pitch) = match (axis, positive) {
                        (0, true) => (-90.0, 0.0),
                        (0, false) => (90.0, 0.0),
                        (1, true) => (0.0, 89.0),
                        (1, false) => (0.0, -89.0),
                        (_, true) => (0.0, 0.0),
                        (_, false) => (180.0, 0.0),
                    };
                    let turn = ((self.camera.yaw - yaw + 180.0f32).rem_euclid(360.0) - 180.0).abs();
                    if turn < 0.5 && (self.camera.pitch - pitch).abs() < 0.5 {
                        if axis == 1 {
                            pitch = -pitch;
                        } else {
                            yaw += 180.0;
                        }
                    }
                    self.camera.snap_to(yaw, pitch, None);
                }
            }
            ui.horizontal(|ui| {
                ui.add_space(2.0 + view_handles::SIZE + 4.0);
                let label = if self.camera.ortho { "Ortho" } else { "Persp" };
                let r = theme::button(ui, 76.0, label, ButtonStyle { height: 22.0, ..Default::default() });
                if tip(r, tips, "Switch between perspective and orthographic.").clicked() {
                    self.camera.ortho = !self.camera.ortho;
                }
            });
            // Under it, the lens: a few named ones and a slider between, in
            // the millimetres a camera's lenses are named by (camera.rs).
            ui.horizontal(|ui| {
                ui.add_space(2.0 + view_handles::SIZE + 4.0);
                let mm = crate::camera::lens_mm(self.camera.fov_y);
                let mut wanted = None;
                let menu = egui::ComboBox::from_id_salt("lens").selected_text(format!("Lens {mm:.0} mm")).width(76.0).show_ui(ui, |ui| {
                    for lens in crate::camera::LENSES {
                        if ui.selectable_label((lens - mm).abs() < 0.5, format!("{lens:.0} mm")).clicked() {
                            wanted = Some(lens);
                        }
                    }
                    let mut value = mm.clamp(*crate::camera::LENS_RANGE.start(), *crate::camera::LENS_RANGE.end());
                    let slider = egui::Slider::new(&mut value, crate::camera::LENS_RANGE).logarithmic(true).suffix(" mm");
                    if ui.add(slider).changed() {
                        wanted = Some(value);
                    }
                });
                tip(
                    menu.response,
                    tips,
                    "The lens the view is seen through. Wide (18-24 mm) bends the model toward you, as a camera held close \
                     does; 50 mm is about what the eye sees; 85 mm and up flatten it, as portraits are taken.",
                );
                if let Some(mm) = wanted {
                    self.set_lens_mm(mm);
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(2.0 + view_handles::SIZE + 4.0);
                let r = theme::button(ui, 76.0, "Frame", ButtonStyle { height: 22.0, badge: "F", ..Default::default() });
                if tip(r, tips, "Fit the model in the view (F).").clicked() {
                    self.frame_model(true);
                }
            });
        });
    }

    /// The status line and the display switches, along the bottom.
    fn bottom_bar(&mut self, ctx: &egui::Context, origin: egui::Pos2, width: f32) {
        let tips = self.ui.tips;
        egui::Area::new(Id::new("bottom_bar"))
            .order(Order::Middle)
            .fade_in(false)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(origin + vec2(0.0, 22.0))
            .show(ctx, |ui| {
                ui.set_max_width(width);
                let s = &self.sculptor.settings;
                let brush = if self.mode == Mode::Tubes {
                    self.tubes.name().to_owned()
                } else if s.mask_mode {
                    "Mask".to_owned()
                } else {
                    self.sculptor.library.entries()[self.sculptor.library.selected(s)].name.clone()
                };
                let mirror: Vec<&str> = ["X", "Y", "Z"].into_iter().zip(s.symmetry).filter(|(_, on)| *on).map(|(a, _)| a).collect();
                let mut words = vec![brush];
                if !mirror.is_empty() {
                    words.push(format!("Mirror {}", mirror.join(" ")));
                }
                if self.sculptor.dyntopo.enabled {
                    words.push("Dynamic Topology".to_owned());
                }
                ui.label(egui::RichText::new(words.join("  ·  ")).font(theme::font(11.0)).color(theme::HINT()));
                // A brush that has quietly stopped adding detail looks broken: say why.
                if let Some(text) = dyntopo::limit_note(&self.sculptor.dyntopo) {
                    ui.label(egui::RichText::new(text).font(theme::font(11.0)).color(dyntopo::WARNING));
                }
                if !self.status.is_empty() {
                    ui.label(egui::RichText::new(&self.status).font(theme::font(11.0)).color(theme::TEXT()));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let flat = theme::checkbox(ui, &mut self.flat_shading, "Flat Shading", ACCENT());
                    tip(flat, tips, "Shade every triangle flat, so the facets of the mesh show.");
                    let backs = theme::checkbox(ui, &mut self.show_backfaces, "Show Backfaces", ACCENT());
                    tip(backs, tips, "Draw the inside of the surface too. Off, you look straight through holes.");
                    let mask = theme::checkbox(ui, &mut self.mask_tint, "Show Mask", ACCENT());
                    tip(mask, tips, "Darken what is masked.");
                    let mut wire = self.look.wire;
                    let r = theme::checkbox(ui, &mut wire, "Wireframe", ACCENT());
                    tip(r, tips, "Draw the edges of the mesh's triangles over it.");
                    if wire != self.look.wire {
                        self.set_wire(wire);
                    }
                    let help = ui.add(egui::Label::new(egui::RichText::new("Help").font(theme::font(12.0)).color(ACCENT())).sense(Sense::click()));
                    if tip(help.on_hover_cursor(egui::CursorIcon::PointingHand), tips, "How to get started (F1).").clicked() {
                        self.open_welcome();
                    }
                });
            });
    }
}

/// The brush's way, size, strength and focal shift, always at the top of
/// the Brush tab (the desktop's `brush_plate`).
fn brush_plate(ui: &mut Ui, s: &mut Settings, icon: &dyn Fn(&str) -> Option<TextureId>, tips: bool) {
    theme::plate(ui, |ui| {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 4.0);
            let positive = s.current().positive;
            let polar = s.brush.has_polarity() && !s.mask_mode;
            let add = ButtonStyle { icon: icon("polarity_add"), active: (polar && positive).then_some(ACCENT()), ..Default::default() };
            if tip(theme::button(ui, w, "Add", add), tips, "Brush adds material. Hold Ctrl or use the right button to flip.").clicked() {
                s.current_mut().positive = true;
                s.mask_mode = false;
            }
            let sub = ButtonStyle { icon: icon("polarity_sub"), active: (polar && !positive).then_some(theme::SUB), ..Default::default() };
            if tip(theme::button(ui, w, "Sub", sub), tips, "Brush removes material.").clicked() {
                s.current_mut().positive = false;
                s.mask_mode = false;
            }
            let mask = ButtonStyle { icon: icon("brush_mask"), active: s.mask_mode.then_some(theme::MASK), badge: "M", ..Default::default() };
            if tip(theme::button(ui, w, "Mask", mask), tips, "Paint a mask that protects the surface from brushes (M). Ctrl erases.")
                .clicked()
            {
                s.mask_mode = !s.mask_mode;
            }
        });
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        brush_size::slider(ui, s, tips);
        let label = "Strength";
        let bs = s.current_mut();
        tip(
            theme::slider(ui, label, &mut bs.strength, 0.01..=1.0, |v| format!("{:.0}%", v * 100.0)),
            tips,
            "How hard the brush works.",
        );
        let mut focal = bs.focal_shift * 100.0;
        if tip(
            theme::slider(ui, "Focal Shift (Hard - Soft)", &mut focal, -100.0..=100.0, |v| format!("{v:.0}")),
            tips,
            "Moves the brush's weight toward its centre or its edge.",
        )
        .changed()
        {
            bs.focal_shift = focal / 100.0;
        }
        brush_size::toggle(ui, s, tips);
    });
}

/// One brush of the list: its icon and its name, lit when in hand (the
/// desktop's `brushes::row`, without the key and wheel marks).
fn row(ui: &mut Ui, width: f32, icon: Option<TextureId>, name: &str, lit: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    p.rect_filled(rect, 5.0, if lit { ACCENT() } else { theme::CONTROL() });
    if response.hovered() {
        p.rect_filled(rect, 5.0, Color32::from_white_alpha(18));
    }
    let ink = if lit { Color32::WHITE } else { theme::TEXT() };
    if let Some(icon) = icon {
        let at = Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(16.0, 16.0));
        let tint = if lit { Color32::WHITE } else { Color32::from_white_alpha(210) };
        p.image(icon, at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
    }
    let size = if name.len() > 13 { 11.0 } else { 12.0 };
    p.text(pos2(rect.left() + 26.0, rect.center().y), Align2::LEFT_CENTER, name, theme::font(size), ink);
    response
}

/// One theme's tile, painted in that theme's own colours: its name on its
/// card, and a button and its accent beside it (the desktop's `settings::swatch`).
fn swatch(ui: &mut Ui, width: f32, palette: &theme::Palette, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    p.rect_filled(rect, 6.0, palette.card);
    let chip = |from_right: f32, fill: Color32| {
        let centre = pos2(rect.right() - from_right, rect.center().y);
        p.rect_filled(Rect::from_center_size(centre, vec2(12.0, 20.0)), 3.0, fill);
    };
    chip(32.0, palette.control);
    chip(16.0, palette.accent);
    let face = if on { theme::bold(12.0) } else { theme::font(12.0) };
    p.text(rect.left_center() + vec2(10.0, -1.0), Align2::LEFT_CENTER, palette.name, face, palette.text);
    if on {
        p.rect_stroke(rect, 6.0, Stroke::new(2.0, palette.accent), egui::StrokeKind::Inside);
    } else {
        p.rect_stroke(rect, 6.0, Stroke::new(1.0, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
        if response.hovered() {
            p.rect_filled(rect, 6.0, Color32::from_white_alpha(16));
        }
    }
    response
}

/// A count rounded to two significant figures, as the desktop's slider gives it.
fn two_figures(value: f32) -> f32 {
    if value <= 0.0 {
        return value;
    }
    let step = 10f32.powf(value.log10().floor() - 1.0);
    (value / step).round() * step
}
