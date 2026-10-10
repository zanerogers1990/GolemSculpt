//! Things drawn over the viewport: the brush cursor, the view gizmo, the
//! radial brush menu and the action toast.

use egui::{
    Align2, Color32, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2,
};
use glam::Vec3;

use super::{UiState, theme};
use crate::doc::{Action, Doc, Tool};
use crate::sculpt::Brush;

/// What the brush cursor should look like this frame, in egui points.
#[derive(Clone, Copy)]
pub struct Cursor {
    pub visible: bool,
    pub center: Pos2,
    pub radius: f32,
    pub tint: Color32,
    /// Smooth has no add/subtract polarity, so its ring is dashed.
    pub dashed: bool,
    /// Radius (as a fraction) where the brush is at half weight.
    pub inner: f32,
    pub inner_bold: bool,
    /// Strength 0..1 shown as a filled disc while F is held.
    pub strength: Option<f32>,
    /// Raw pointer position when lazy mouse is trailing behind it.
    pub tether: Option<Pos2>,
    /// Orbit pivot marker and its opacity.
    pub pivot: Option<(Pos2, f32)>,
}

pub const ADD: Color32 = Color32::from_rgb(51, 255, 102);
pub const SUBTRACT: Color32 = Color32::from_rgb(255, 76, 76);
pub const SMOOTH: Color32 = Color32::from_rgb(76, 166, 255);

pub fn cursor(ctx: &egui::Context, c: &Cursor) {
    let p = ctx.layer_painter(LayerId::background());
    if let Some((pos, alpha)) = c.pivot {
        let a = |v: f32| (v * alpha * 255.0) as u8;
        p.circle_stroke(pos, 5.5, Stroke::new(2.0, Color32::from_black_alpha(a(0.75))));
        p.circle_filled(pos, 2.5, Color32::from_white_alpha(a(1.0)));
    }
    if !c.visible {
        return;
    }
    let r = c.radius.max(7.0);
    if let Some(strength) = c.strength {
        let t = ((strength - 0.01) / 0.99).clamp(0.0, 1.0);
        p.circle_filled(c.center, (r * t).max(2.0), Color32::from_rgba_unmultiplied(255, 77, 77, 115));
    }
    let halo = Stroke::new(4.0, Color32::from_black_alpha(110));
    let ring = Stroke::new(1.75, c.tint);
    if c.dashed {
        let dashes = 14;
        for i in 0..dashes {
            let a0 = i as f32 / dashes as f32 * std::f32::consts::TAU;
            let a1 = a0 + 0.6 / dashes as f32 * std::f32::consts::TAU;
            let arc: Vec<Pos2> = (0..=6)
                .map(|k| {
                    let a = a0 + (a1 - a0) * k as f32 / 6.0;
                    c.center + vec2(a.cos(), a.sin()) * r
                })
                .collect();
            p.line(arc.clone(), halo);
            p.line(arc, ring);
        }
    } else {
        p.circle_stroke(c.center, r, halo);
        p.circle_stroke(c.center, r, ring);
    }
    if c.inner > 0.0 && c.inner < 0.99 {
        let (width, alpha) = if c.inner_bold { (2.0, 1.0) } else { (1.5, 0.6) };
        p.circle_stroke(c.center, (r * c.inner).max(4.0), Stroke::new(width, c.tint.gamma_multiply(alpha)));
    }
    p.circle_filled(c.center, 2.0, c.tint);
    if let Some(raw) = c.tether {
        p.line_segment([c.center, raw], Stroke::new(1.5, c.tint.gamma_multiply(0.5)));
        p.circle_filled(raw, 2.5, c.tint.gamma_multiply(0.5));
    }
}

/// The transform gizmo, under the panels like the model it sits on.
pub fn transform_gizmo(ctx: &egui::Context, shapes: &[crate::gizmo::Shape]) {
    use crate::gizmo::Shape;
    if shapes.is_empty() {
        return;
    }
    let ppp = ctx.pixels_per_point();
    let at = |p: &glam::Vec2| pos2(p.x / ppp, p.y / ppp);
    let tint = |c: &[u8; 4]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
    let p = ctx.layer_painter(LayerId::background());
    for shape in shapes {
        match shape {
            Shape::Line { points, width, color } => {
                p.line(points.iter().map(at).collect(), Stroke::new(width / ppp, tint(color)));
            }
            Shape::Fill { points, color } => {
                p.add(egui::Shape::convex_polygon(points.iter().map(at).collect(), tint(color), Stroke::NONE));
            }
        }
    }
}

pub fn view_gizmo(ctx: &egui::Context, doc: &mut Doc, origin: Pos2, actions: &mut Vec<Action>) {
    egui::Area::new(Id::new("view_gizmo")).order(Order::Middle).fade_in(false).fixed_pos(origin).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(108.0), Sense::click_and_drag());
        let p = ui.painter();
        let center = rect.center();
        if response.hovered() || response.dragged() {
            p.circle_filled(center, 52.0, Color32::from_black_alpha(102));
        }
        if response.dragged() {
            // Same feel as orbiting in the viewport.
            let d = response.drag_delta() * ui.ctx().pixels_per_point();
            doc.camera.orbit(glam::Vec2::new(d.x, d.y), None);
        }

        let inv = doc.camera.rotation().inverse();
        // (axis, positive?, view-space direction)
        let mut bubbles: Vec<(usize, bool, Vec3)> = Vec::new();
        for axis in 0..3 {
            let mut dir = Vec3::ZERO;
            dir[axis] = 1.0;
            bubbles.push((axis, true, inv * dir));
            bubbles.push((axis, false, inv * -dir));
        }
        // Far ones first so near ones draw over them.
        bubbles.sort_by(|a, b| a.2.z.total_cmp(&b.2.z));
        let pointer = response.hover_pos();
        let mut clicked = None;
        let mut named = None;
        for &(axis, positive, v) in &bubbles {
            let at = center + vec2(v.x, -v.y) * 37.0;
            let depth = v.z * 0.5 + 0.5;
            let mut scale = 0.84 + (1.08 - 0.84) * depth;
            let hovered = pointer.is_some_and(|q| q.distance(at) < 10.0 * scale);
            if hovered {
                scale *= 1.14;
                // The nearest bubble under the pointer is the last one drawn.
                named = Some(match (axis, positive) {
                    (0, true) => "Right view",
                    (0, false) => "Left view",
                    (1, true) => "Top view",
                    (1, false) => "Bottom view",
                    (_, true) => "Front view",
                    (_, false) => "Back view",
                });
                if response.clicked() {
                    clicked = Some((axis, positive));
                }
            }
            let color = theme::AXIS[axis].gamma_multiply(0.55 + 0.45 * depth);
            let letter = ["X", "Y", "Z"][axis];
            if positive {
                p.line_segment([center, at], Stroke::new(3.0, color));
                p.circle_filled(at, 9.0 * scale, color);
                p.text(at, Align2::CENTER_CENTER, letter, theme::bold(10.0), Color32::from_black_alpha(220));
            } else {
                p.circle_filled(at, 8.0 * scale, Color32::from_rgb(24, 24, 30));
                p.circle_stroke(at, 8.0 * scale, Stroke::new(1.5, color));
                if hovered {
                    p.text(at, Align2::CENTER_CENTER, letter, theme::bold(9.0), color);
                }
            }
        }
        if let Some((axis, positive)) = clicked {
            // Top and bottom always come up straight: the front of the model
            // toward the bottom of the screen, never a quarter turn round.
            let (mut yaw, mut pitch) = match (axis, positive) {
                (0, true) => (-90.0, 0.0),
                (0, false) => (90.0, 0.0),
                (1, true) => (0.0, 90.0),
                (1, false) => (0.0, -90.0),
                (_, true) => (0.0, 0.0),
                (_, false) => (180.0, 0.0),
            };
            // Clicking the view you are already in flips to the other side.
            let turn = ((doc.camera.yaw - yaw + 180.0).rem_euclid(360.0) - 180.0).abs();
            if turn < 0.5 && (doc.camera.pitch - pitch).abs() < 0.5 {
                if axis == 1 {
                    pitch = -pitch;
                } else {
                    yaw += 180.0;
                }
            }
            actions.push(Action::Snap(yaw, pitch));
        }
        if let (Some(name), true) = (named, doc.opts.tooltips) {
            response.on_hover_text(name);
        }

        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let label = if doc.camera.ortho { "Ortho" } else { "Persp" };
            let r = theme::button(ui, 76.0, label, theme::ButtonStyle { height: 22.0, ..Default::default() });
            if r.on_hover_text("Switch between perspective and orthographic.").clicked() {
                doc.camera.ortho = !doc.camera.ortho;
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let style = theme::ButtonStyle {
                height: 22.0,
                active: doc.opts.stroke_pivot.then_some(theme::ACCENT),
                ..Default::default()
            };
            let r = theme::button(ui, 76.0, "Stroke Pivot", style);
            if r.on_hover_text("Orbit and zoom around the last place you sculpted.").clicked() {
                doc.opts.stroke_pivot = !doc.opts.stroke_pivot;
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let style = theme::ButtonStyle { height: 22.0, badge: "F F", ..Default::default() };
            let r = theme::button(ui, 76.0, "Frame", style);
            if r.on_hover_text("Fit the model in view (press F twice, or double-click the background).").clicked() {
                actions.push(Action::Frame);
            }
        });
    });
}

/// Hold-Space wheel: the brushes when sculpting, the edit modes while the
/// SSphere tool is up. Tiles are clicked; letting go of Space closes it.
pub fn radial_menu(ctx: &egui::Context, doc: &mut Doc, state: &mut UiState) {
    let Some(anchor) = state.radial else { return };
    let screen = ctx.content_rect();
    let open = ctx.animate_bool_with_time(Id::new("radial_open"), true, 0.11);
    let spheres = doc.tool == Tool::SSpheres;
    // The wheel's half size, the radius of its disc and of the ring the
    // tiles sit on, a tile's size and how near the pointer picks it. The
    // brush wheel has nine tiles where the Unity app's has eight (Pose is
    // not on that one): on the old ring two of nine would overlap at the
    // corners, so its ring is a little wider and its tiles a little smaller.
    let (half, disc, ring, tile_size, pick) =
        if spheres { (140.0, 134.0, 92.0, 58.0, 34.0) } else { (152.0, 146.0, 104.0, 54.0, 31.0) };
    let center = pos2(
        anchor.x.clamp(screen.left() + half, screen.right() - half),
        anchor.y.clamp(screen.top() + half, screen.bottom() - half),
    );
    // Clockwise from the top; `None` is the mask tile.
    let brushes: [(Option<Brush>, &str, &str); 9] = [
        (Some(Brush::Move), "Move", "brush_move"),
        (Some(Brush::Clay), "Clay", "brush_clay"),
        (Some(Brush::Standard), "Standard", "brush_standard"),
        (Some(Brush::Smooth), "Smooth", "brush_smooth"),
        (Some(Brush::Crease), "Crease", "brush_crease"),
        (None, "Mask", "brush_mask"),
        (Some(Brush::Inflate), "Inflate", "brush_inflate"),
        (Some(Brush::Flatten), "Flatten", "brush_flatten"),
        (Some(Brush::Pose), "Pose", "brush_pose"),
    ];
    // Name, icon and whether it is the current choice.
    let s = &doc.sculptor.settings;
    let tiles: Vec<(&str, &str, bool)> = if spheres {
        super::spheres::radial_tiles(doc)
    } else {
        let lit = |brush: Option<Brush>| match brush {
            Some(b) => b == s.brush && !s.mask_mode,
            None => s.mask_mode,
        };
        brushes.iter().map(|&(brush, name, icon)| (name, icon, lit(brush))).collect()
    };
    let current = if spheres {
        super::spheres::radial_current(doc)
    } else if s.mask_mode {
        "Mask"
    } else {
        s.brush.name()
    };
    let mut clicked = None;
    egui::Area::new(Id::new("radial_menu"))
        .order(Order::Foreground)
        .fixed_pos(center - Vec2::splat(half))
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(2.0 * half), Sense::click());
            let p = ui.painter();
            let scale = 0.85 + 0.15 * open;
            let fade = |c: Color32| c.gamma_multiply(open);
            let c = rect.center();
            p.circle_filled(c, disc * scale, fade(Color32::from_rgba_unmultiplied(14, 14, 18, 200)));
            p.circle_stroke(c, disc * scale, Stroke::new(1.0, fade(Color32::from_white_alpha(18))));
            let pointer = response.hover_pos();
            let mut hovered_name = None;
            for (i, &(name, icon, active)) in tiles.iter().enumerate() {
                let a = i as f32 / tiles.len() as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let at = c + vec2(a.cos(), a.sin()) * ring * scale;
                let hovered = pointer.is_some_and(|q| q.distance(at) < pick);
                let size = tile_size * scale * if hovered { 1.1 } else { 1.0 };
                let tile = Rect::from_center_size(at, Vec2::splat(size));
                let fill = if hovered {
                    Color32::from_rgb(92, 163, 255)
                } else if active {
                    theme::ACCENT
                } else {
                    Color32::from_rgb(43, 43, 54)
                };
                p.rect_filled(tile, 11.0, fade(fill));
                if let Some(id) = state.assets.icon(icon) {
                    p.image(
                        id,
                        Rect::from_center_size(at - vec2(0.0, 7.0), Vec2::splat(28.0)),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        fade(Color32::WHITE),
                    );
                }
                p.text(at + vec2(0.0, 18.0), Align2::CENTER_CENTER, name, theme::bold(10.0), fade(theme::TEXT));
                if hovered {
                    hovered_name = Some(name);
                    if response.clicked() {
                        clicked = Some(i);
                    }
                }
            }
            p.circle_filled(c, 40.0 * scale, fade(Color32::from_rgb(26, 26, 32)));
            p.text(c, Align2::CENTER_CENTER, hovered_name.unwrap_or(current), theme::bold(14.0), fade(theme::TEXT));
        });
    if spheres {
        // The size of the next sphere, set from under the wheel.
        super::spheres::radial_slider(ctx, doc, center, 140.0, open);
    }
    if let Some(i) = clicked {
        if spheres {
            super::spheres::radial_pick(doc, i);
        } else {
            let s = &mut doc.sculptor.settings;
            match brushes[i].0 {
                Some(b) => {
                    s.brush = b;
                    s.mask_mode = false;
                }
                None => s.mask_mode = true,
            }
        }
        state.radial = None;
    }
}

pub fn toast(ctx: &egui::Context, state: &mut UiState, time: f64) {
    let Some((text, color, start)) = &state.toast else { return };
    let age = (time - start) as f32;
    if age > 1.4 {
        state.toast = None;
        return;
    }
    let screen = ctx.content_rect();
    let rise = 24.0 * (age / 0.8).min(1.0);
    let alpha = (1.0 - (age - 0.8).max(0.0) / 0.6).clamp(0.0, 1.0);
    let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("toast")));
    let pos = pos2(screen.center().x, screen.bottom() - 60.0 - rise);
    p.text(pos + vec2(0.0, 1.5), Align2::CENTER_CENTER, text, theme::bold(22.0), Color32::from_black_alpha((160.0 * alpha) as u8));
    p.text(pos, Align2::CENTER_CENTER, text, theme::bold(22.0), color.gamma_multiply(alpha));
    ctx.request_repaint();
}

/// One line of help at the bottom of the screen for a moment after the clean
/// view comes up: with every panel gone there is otherwise nothing to say
/// how to get them back. `alpha` fades it out.
pub fn clean_hint(ctx: &egui::Context, text: &str, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let screen = ctx.content_rect();
    let rect = Rect::from_center_size(pos2(screen.center().x, screen.bottom() - 39.0), vec2(420.0, 30.0));
    let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("clean_hint")));
    p.rect_filled(rect, 6.0, Color32::from_black_alpha((140.0 * alpha) as u8));
    p.text(rect.center(), Align2::CENTER_CENTER, text, theme::font(14.0), Color32::from_white_alpha((230.0 * alpha) as u8));
    ctx.request_repaint();
}
