//! Things drawn over the viewport: the brush cursor, the view gizmo, the
//! radial brush menu with the bar under it, and the action toast.

use egui::{
    Align2, Color32, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2,
};
use glam::Vec3;

use super::brushes::{initials, shortened, wheel_tiles};
use super::{UiState, theme};
use crate::doc::{Action, Doc, Tool};
use crate::sculpt::Brush;
use crate::sculpt::library::{Library, Origin};

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
    /// The picture the brush will print, to show inside the ring
    /// (`alpha_preview.rs`); `None` for a brush with no alpha.
    pub alpha: Option<super::alpha_preview::Preview>,
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
    // Under the ring's own lines, and only where the ring is.
    if let Some(preview) = &c.alpha {
        super::alpha_preview::draw(&p, c.center, r, c.tint, preview);
    }
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
            // Same feel as orbiting in the viewport, and about the model as there.
            let d = response.drag_delta() * ui.ctx().pixels_per_point();
            actions.push(Action::Turn { by: [d.x, d.y], begins: response.drag_started() });
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
                // The far end of an axis: a ring, filled in the backdrop's colour.
                let [r, g, b] = doc.opts.background.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
                p.circle_filled(at, 8.0 * scale, Color32::from_rgb(r, g, b));
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
                active: doc.opts.stroke_pivot.then_some(theme::ACCENT()),
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

/// Where a hold-Space wheel that reaches `half` each way from its centre
/// opens for a pointer at `anchor`: round the pointer, moved in as far as it
/// takes for all of the wheel to be on `screen`. On a window smaller than
/// the wheel there is no such place; it then sits in the middle, with as
/// much of it showing as there can be. (`clamp` would panic there: its low
/// end is past its high end.)
pub fn wheel_center(anchor: Pos2, half: f32, screen: Rect) -> Pos2 {
    let middle = |low: f32, high: f32| (low + high) * 0.5;
    let fit = |v: f32, low: f32, high: f32| if low <= high && v.is_finite() { v.max(low).min(high) } else { middle(low, high) };
    pos2(
        fit(anchor.x, screen.left() + half, screen.right() - half),
        fit(anchor.y, screen.top() + half, screen.bottom() - half),
    )
}

/// How a wheel of `tiles` tiles is laid out, in points.
struct WheelLayout {
    /// How far the wheel reaches each way from its centre, and the radius
    /// of its disc.
    half: f32,
    disc: f32,
    /// The radius of the ring the tiles sit on.
    ring: f32,
    /// A tile's side, and how near its middle the pointer picks it.
    tile: f32,
    pick: f32,
}

impl WheelLayout {
    /// The SSphere tool's wheel of edit modes: the Unity app's.
    const SPHERES: WheelLayout = WheelLayout { half: 140.0, disc: 134.0, ring: 92.0, tile: 58.0, pick: 34.0 };

    /// The brush wheel. Up to nine tiles it is as it has been (a little
    /// wider than the Unity app's eight, so that no two of nine overlap).
    /// With more, the tiles are smaller and the ring as much wider as keeps
    /// a gap between each tile and the next, however many there are.
    fn brushes(tiles: usize) -> WheelLayout {
        if tiles <= 9 {
            return WheelLayout { half: 152.0, disc: 146.0, ring: 104.0, tile: 54.0, pick: 31.0 };
        }
        const TILE: f32 = 48.0;
        const GAP: f32 = 6.0;
        // Neighbours on a ring are a chord apart: the tile and the gap.
        let ring = ((TILE + GAP) / (2.0 * (std::f32::consts::PI / tiles as f32).sin())).max(104.0).ceil();
        let half = ring + TILE * 0.5 + 24.0;
        WheelLayout { half, disc: half - 6.0, ring, tile: TILE, pick: (TILE + GAP) * 0.5 - 1.0 }
    }
}

/// The ring the brush wheel's tiles sit on, in points from its centre: the
/// top tile is that far straight up. The wheel has a tile for each brush
/// pinned to it (`brushes::wheel_tiles`), so the ring is as wide as they need.
pub fn brush_wheel_ring(library: &Library) -> f32 {
    WheelLayout::brushes(wheel_tiles(library).len()).ring
}

/// Where on the brush wheel the tile of brush `brush` of the library is,
/// from the wheel's centre, in points; `None` if the brush is not pinned.
pub fn brush_wheel_tile(library: &Library, brush: usize) -> Option<Vec2> {
    let tiles = wheel_tiles(library);
    let i = tiles.iter().position(|&tile| tile == Some(brush))?;
    let a = i as f32 / tiles.len() as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    Some(vec2(a.cos(), a.sin()) * WheelLayout::brushes(tiles.len()).ring)
}

/// What a tile of the brush wheel shows for a brush of the library, or for
/// the mask tile: its name, its icon and, for a brush that is not one of
/// the base fourteen, its initials for the corner (the icon is its kernel's,
/// which it shares).
fn brush_tile(library: &Library, tile: Option<usize>) -> (&str, &'static str, String) {
    let Some(entry) = tile.and_then(|i| library.entries().get(i)) else { return ("Mask", "brush_mask", String::new()) };
    let badge = if entry.origin == Origin::Base { String::new() } else { initials(&entry.name) };
    (&entry.name, entry.kernel.icon(), badge)
}

/// Hold-Space wheel: the brushes when sculpting, the edit modes while the
/// SSphere tool is up. Tiles are clicked; letting go of Space closes it.
/// A right-click takes a brush's tile off the wheel and leaves the wheel
/// open (a right-click on a brush in the panel puts it back).
/// Under it is a bar for the one value worth setting from here: the brush's
/// strength, or the next sphere's size.
pub fn radial_menu(ctx: &egui::Context, doc: &mut Doc, state: &mut UiState) {
    let Some(anchor) = state.radial else { return };
    let screen = ctx.content_rect();
    let open = ctx.animate_bool_with_time(Id::new("radial_open"), true, 0.11);
    let spheres = doc.tool == Tool::SSpheres;
    // Clockwise from the top: the pinned brushes, by their place in the
    // library; `None` is the mask tile.
    let (library, s) = (&doc.sculptor.library, &doc.sculptor.settings);
    let brushes = wheel_tiles(library);
    let in_hand = library.selected(s);
    let WheelLayout { half, disc, ring, tile: tile_size, pick } =
        if spheres { WheelLayout::SPHERES } else { WheelLayout::brushes(brushes.len()) };
    let center = wheel_center(anchor, half, screen);
    // Name, icon and whether it is the current choice; and, for the brush
    // wheel, what goes in each tile's corner.
    let mut badges = Vec::new();
    let tiles: Vec<(&str, &str, bool)> = if spheres {
        super::spheres::radial_tiles(doc)
    } else {
        let lit = |tile: Option<usize>| match tile {
            Some(i) => i == in_hand && !s.mask_mode,
            None => s.mask_mode,
        };
        let shown: Vec<_> = brushes.iter().map(|&tile| (brush_tile(library, tile), lit(tile))).collect();
        badges = shown.iter().map(|((_, _, badge), _)| badge.clone()).collect();
        shown.into_iter().map(|((name, icon, _), lit)| (name, icon, lit)).collect()
    };
    let current = if spheres {
        super::spheres::radial_current(doc)
    } else if s.mask_mode {
        "Mask"
    } else {
        library.entries()[in_hand].name.as_str()
    };
    let mut clicked = None;
    let mut unpinned = None;
    let tips = doc.opts.tooltips;
    egui::Area::new(Id::new("radial_menu"))
        .order(Order::Foreground)
        .fixed_pos(center - Vec2::splat(half))
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(2.0 * half), Sense::click());
            let p = ui.painter();
            let scale = 0.85 + 0.15 * open;
            let fade = |c: Color32| c.gamma_multiply(open);
            let c = rect.center();
            p.circle_filled(c, disc * scale, fade(theme::wheel_disc()));
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
                    theme::lit(theme::ACCENT())
                } else if active {
                    theme::ACCENT()
                } else {
                    theme::HEADER()
                };
                p.rect_filled(tile, 11.0, fade(fill));
                // The picture and the name under it, in the tile's own
                // proportions (28 and 10 points on a tile of 54).
                let k = tile_size / 54.0;
                if let Some(id) = state.assets.icon(icon) {
                    p.image(
                        id,
                        Rect::from_center_size(at - vec2(0.0, 7.0 * k), Vec2::splat(28.0 * k)),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        fade(Color32::WHITE),
                    );
                }
                // A long name a size down, and the longest cut short, so it
                // stays on its own tile (the middle of the wheel has it whole).
                let letters = if name.len() > 8 { 8.0 } else { 10.0 };
                let font = theme::bold(letters * k.max(0.95));
                let fits = |text: &str| p.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x <= tile_size * scale - 4.0;
                let count = name.chars().count();
                let shown = (0..=count).rev().map(|n| shortened(name, n)).find(|text| fits(text)).unwrap_or_default();
                p.text(at + vec2(0.0, 18.0 * k), Align2::CENTER_CENTER, shown, font, fade(theme::TEXT()));
                if let Some(badge) = badges.get(i).filter(|badge| !badge.is_empty()) {
                    let corner = tile.right_top() + vec2(-4.0, 3.0);
                    p.text(corner, Align2::RIGHT_TOP, badge, theme::bold(9.0 * k.max(0.95)), fade(theme::KEY));
                }
                if hovered {
                    hovered_name = Some(name);
                    if response.clicked() {
                        clicked = Some(i);
                    }
                    if response.secondary_clicked() {
                        unpinned = Some(i);
                    }
                }
            }
            p.circle_filled(c, 40.0 * scale, fade(theme::CARD()));
            p.text(c, Align2::CENTER_CENTER, hovered_name.unwrap_or(current), theme::bold(14.0), fade(theme::TEXT()));
            // Under the middle, while a brush's tile is pointed at: how to
            // take it off the wheel. Only with the tooltips on.
            let brush_hovered = !spheres && hovered_name.is_some_and(|name| name != "Mask");
            if tips && brush_hovered {
                let at = c + vec2(0.0, 52.0 * scale);
                p.text(at, Align2::CENTER_CENTER, "Right-click: off the wheel", theme::font(9.0), fade(theme::KEY));
            }
        });
    if spheres {
        // The size of the next sphere, set from under the wheel.
        super::spheres::radial_slider(ctx, doc, center, half, open);
    } else {
        let name = doc.sculptor.library.entries()[in_hand].name.clone();
        strength_bar(ctx, &mut doc.sculptor.settings, &name, center, half, open);
    }
    if let Some(Some(brush)) = unpinned.filter(|_| !spheres).map(|i| brushes[i]) {
        doc.sculptor.library.set_pinned(brush, false);
    }
    if let Some(i) = clicked {
        if spheres {
            super::spheres::radial_pick(doc, i);
        } else {
            let (library, s) = (&mut doc.sculptor.library, &mut doc.sculptor.settings);
            match brushes[i] {
                // Picking a brush leaves mask painting.
                Some(brush) => library.select(brush, s),
                None => s.mask_mode = true,
            }
        }
        state.radial = None;
    }
}

/// The size of the plate a wheel's bar sits on, and the gap between it and
/// the wheel.
const BAR_SIZE: Vec2 = vec2(236.0, theme::SLIDER_HEIGHT + 14.0);
const BAR_GAP: f32 = 6.0;

/// Where the bar that goes with a wheel sits: under the wheel, or above it
/// when the wheel is too near the bottom of the window for the bar to fit.
/// On a window too short for either it goes beside the wheel, to the right
/// if it fits there, else to the left. It is never put where part of it
/// would be off the window or on the wheel's tiles: where none of the four
/// places will do there is no bar (the value has its slider in the panel).
/// `center` is the wheel's centre and `radius` its outer radius.
fn wheel_bar_rect(center: Pos2, radius: f32, screen: Rect) -> Option<Rect> {
    let wheel = Rect::from_center_size(center, Vec2::splat(2.0 * radius));
    let across = center.x - BAR_SIZE.x * 0.5;
    let down = center.y - BAR_SIZE.y * 0.5;
    let places = [
        pos2(across, wheel.bottom() + BAR_GAP),
        pos2(across, wheel.top() - BAR_GAP - BAR_SIZE.y),
        pos2(wheel.right() + BAR_GAP, down),
        pos2(wheel.left() - BAR_GAP - BAR_SIZE.x, down),
    ];
    places.into_iter().map(|at| Rect::from_min_size(at, BAR_SIZE)).find(|bar| screen.contains_rect(*bar))
}

/// The part of a wheel bar's plate the slider is painted in. The whole plate
/// takes the press; the track runs a little inside this.
fn wheel_bar_slider(plate: Rect) -> Rect {
    Rect::from_min_max(plate.min + vec2(12.0, 6.0), plate.max - vec2(12.0, 8.0))
}

/// A slider that goes with a hold-Space wheel, so one value can be set
/// without a trip to the panel: the next sphere's size under the SSphere
/// wheel, the brush's strength under the brush wheel. A press anywhere on
/// its plate sets the value and a drag follows the pointer, so a pen need
/// not land on the thin track. `open` is how far the wheel has faded in
/// (0 to 1). Returns whether the value changed. On a window with no room
/// for it (`wheel_bar_rect`) there is no bar, and nothing changes.
pub fn wheel_bar(
    ctx: &egui::Context,
    id: &str,
    (center, radius, open): (Pos2, f32, f32),
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    readout: impl Fn(f32) -> String,
) -> bool {
    let Some(plate) = wheel_bar_rect(center, radius, ctx.content_rect()) else { return false };
    let mut changed = false;
    egui::Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(plate.min).show(ctx, |ui| {
        ui.multiply_opacity(open);
        let (rect, response) = ui.allocate_exact_size(plate.size(), Sense::click_and_drag());
        // The card colour of the panels, so the unfilled part of the track
        // shows against it as it does there.
        ui.painter().rect_filled(rect, 10.0, theme::CARD());
        changed = theme::slider_in(ui, wheel_bar_slider(rect), response, label, value, range, readout).changed();
    });
    changed
}

/// The lowest and highest Strength a brush can have: the ends of the Sculpt
/// panel's slider, of F + drag and of the bar under the wheel.
const STRENGTH_RANGE: std::ops::RangeInclusive<f32> = 0.01..=1.0;

/// What the strength bar under the brush wheel is labelled, or `None` when
/// there is nothing for it to set. The bar always belongs to the brush in
/// hand, never to the tile the pointer happens to be over, so its label
/// names that brush. Mask painting works at the strength of the brush under
/// it, so the bar stays up for it. Pose alone makes no use of Strength, and
/// a control that does nothing is not shown.
fn strength_bar_label(s: &crate::sculpt::Settings, name: &str) -> Option<String> {
    if s.mask_mode {
        Some("Mask Strength".to_owned())
    } else if s.brush == Brush::Pose {
        None
    } else {
        Some(format!("{name} Strength"))
    }
}

/// The Strength bar under the brush wheel: the Strength of the brush in
/// hand (`name`), the same value the Sculpt panel's slider and F + drag change.
fn strength_bar(ctx: &egui::Context, s: &mut crate::sculpt::Settings, name: &str, center: Pos2, radius: f32, open: f32) {
    let Some(label) = strength_bar_label(s, name) else { return };
    let strength = &mut s.current_mut().strength;
    let percent = |v: f32| format!("{:.0}%", v * 100.0);
    wheel_bar(ctx, "radial_strength", (center, radius, open), &label, strength, STRENGTH_RANGE, percent);
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

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton};

    use super::*;
    use crate::sculpt::Settings;

    const SCREEN: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1600.0, 900.0));

    /// One frame of an interface with nothing in it but `ui`, fed `events`.
    fn frame(ctx: &egui::Context, events: Vec<Event>, mut ui: impl FnMut(&egui::Context)) {
        let input = egui::RawInput { screen_rect: Some(SCREEN), events, ..Default::default() };
        ctx.run_ui(input, |root| ui(root.ctx())).drop_without_applying_deltas();
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() }
    }

    #[test]
    fn the_strength_bar_belongs_to_the_brush_in_hand() {
        let mut s = Settings::default();
        s.brush = Brush::Clay;
        assert_eq!(strength_bar_label(&s, "Clay").as_deref(), Some("Clay Strength"));
        // A brush of the library over the same kernel goes by its own name.
        assert_eq!(strength_bar_label(&s, "Clay Strips").as_deref(), Some("Clay Strips Strength"));
        s.brush = Brush::SnakeHook;
        assert_eq!(strength_bar_label(&s, "Snake Hook").as_deref(), Some("Snake Hook Strength"));
        // Pose makes no use of Strength: no bar that does nothing.
        s.brush = Brush::Pose;
        assert_eq!(strength_bar_label(&s, "Pose"), None);
        // Mask painting works at the strength of the brush under it, Pose included.
        s.mask_mode = true;
        assert_eq!(strength_bar_label(&s, "Pose").as_deref(), Some("Mask Strength"));
        // The bar's ends are the ends a settings file is held to.
        let mut kept = crate::prefs::BrushPrefs::of(&s);
        for (written, read) in [(-3.0, *STRENGTH_RANGE.start()), (40.0, *STRENGTH_RANGE.end())] {
            kept.brushes.get_mut("Clay").unwrap().strength = written;
            kept.apply(&mut s);
            assert_eq!(s.per_brush[Brush::Clay.index()].strength, read);
        }
    }

    #[test]
    fn a_wheel_bar_sits_under_its_wheel_or_above_it_at_the_bottom_of_the_window() {
        let under = wheel_bar_rect(pos2(800.0, 400.0), 152.0, SCREEN).unwrap();
        assert_eq!(under.top(), 400.0 + 152.0 + BAR_GAP);
        assert_eq!(under.center().x, 800.0);
        assert_eq!(under.size(), BAR_SIZE);
        // The wheel as low as it can open: the bar would hang off the window.
        let above = wheel_bar_rect(pos2(800.0, 900.0 - 152.0), 152.0, SCREEN).unwrap();
        assert_eq!(above.bottom(), 900.0 - 304.0 - BAR_GAP);
        // And the last place it still fits under.
        let fits = wheel_bar_rect(pos2(800.0, 900.0 - 152.0 - BAR_GAP - BAR_SIZE.y), 152.0, SCREEN).unwrap();
        assert_eq!(fits.bottom(), 900.0);
    }

    /// The two wheels' half sizes: the brush wheel's, and the SSphere and
    /// hosted tools' (see `radial_menu` and `tools::radial`).
    const WHEELS: [f32; 2] = [152.0, 140.0];

    /// Holding Space opens the wheel on a window of any size, down to one
    /// point each way and none at all (house rule 9: the lowest input). It
    /// is whole wherever the window is big enough for it, and in the middle
    /// of a window that is not.
    #[test]
    fn a_wheel_has_a_place_on_a_window_of_any_size() {
        let sizes = [(0.0, 0.0), (1.0, 1.0), (303.0, 900.0), (900.0, 303.0), (200.0, 150.0), (304.0, 304.0), (1600.0, 900.0)];
        for half in WHEELS {
            for (width, height) in sizes {
                let screen = Rect::from_min_size(pos2(10.0, 20.0), vec2(width, height));
                for anchor in [pos2(0.0, 0.0), screen.center(), screen.max, pos2(-5000.0, 9000.0), pos2(f32::NAN, f32::INFINITY)] {
                    let what = format!("half {half}, {width} x {height}, pointer at {anchor:?}");
                    let center = wheel_center(anchor, half, screen);
                    assert!(center.x.is_finite() && center.y.is_finite(), "{what}: {center:?}");
                    let wheel = Rect::from_center_size(center, Vec2::splat(2.0 * half));
                    if width >= 2.0 * half {
                        assert!(wheel.left() >= screen.left() && wheel.right() <= screen.right(), "{what}: {wheel:?}");
                    } else {
                        assert_eq!(center.x, screen.center().x, "{what}");
                    }
                    if height >= 2.0 * half {
                        assert!(wheel.top() >= screen.top() && wheel.bottom() <= screen.bottom(), "{what}: {wheel:?}");
                    } else {
                        assert_eq!(center.y, screen.center().y, "{what}");
                    }
                }
            }
        }
        // Where there is room it opens round the pointer, as it always has.
        assert_eq!(wheel_center(pos2(700.0, 420.0), 152.0, SCREEN), pos2(700.0, 420.0));
        assert_eq!(wheel_center(pos2(3.0, 899.0), 152.0, SCREEN), pos2(152.0, 900.0 - 152.0));
    }

    /// On a window of any size, wherever the wheel opens, its bar is all on
    /// the window and off the wheel's tiles, or there is no bar.
    #[test]
    fn a_wheel_bar_is_never_off_the_window_or_on_the_tiles() {
        let mut beside = 0;
        for half in WHEELS {
            for height in [0.0, 1.0, 40.0, 200.0, 303.0, 304.0, 330.0, 361.0, 362.0, 420.0, 900.0] {
                for width in [0.0, 1.0, 235.0, 304.0, 545.0, 546.0, 700.0, 1600.0] {
                    let screen = Rect::from_min_size(Pos2::ZERO, vec2(width, height));
                    for anchor in [screen.min, screen.center(), screen.max, screen.left_bottom(), screen.right_top()] {
                        let center = wheel_center(anchor, half, screen);
                        let Some(bar) = wheel_bar_rect(center, half, screen) else { continue };
                        let wheel = Rect::from_center_size(center, Vec2::splat(2.0 * half));
                        let what = format!("half {half}, {width} x {height}, pointer at {anchor:?}: {bar:?}");
                        assert!(screen.contains_rect(bar), "off the window: {what}");
                        assert!(!bar.intersects(wheel.shrink(0.01)), "on the wheel: {what}");
                        assert_eq!(bar.size(), BAR_SIZE);
                        beside += usize::from(bar.center().y == center.y);
                    }
                }
            }
        }
        assert!(beside > 0, "some short, wide window has the bar beside the wheel");
        // A window 330 points high has no room over or under the brush wheel:
        // the bar goes beside it, on whichever side has the room.
        let short = Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 330.0));
        let center = wheel_center(pos2(400.0, 100.0), 152.0, short);
        assert_eq!(center, pos2(400.0, 152.0));
        let right = wheel_bar_rect(center, 152.0, short).unwrap();
        assert_eq!(right.min, pos2(400.0 + 152.0 + BAR_GAP, 152.0 - BAR_SIZE.y * 0.5));
        let left = wheel_bar_rect(wheel_center(pos2(1590.0, 100.0), 152.0, short), 152.0, short).unwrap();
        assert_eq!(left.right(), 1600.0 - 304.0 - BAR_GAP);
        // And one with room on neither side has none.
        let small = Rect::from_min_size(Pos2::ZERO, vec2(500.0, 330.0));
        assert_eq!(wheel_bar_rect(wheel_center(small.center(), 152.0, small), 152.0, small), None);
    }

    /// A pen pressed anywhere on the bar's plate sets the value from where
    /// it is along the track, a drag follows it past either end and no
    /// further, and letting go changes nothing.
    #[test]
    fn a_press_and_drag_along_a_wheel_bar_sets_its_value() {
        let ctx = egui::Context::default();
        let wheel = (pos2(800.0, 400.0), 152.0, 1.0);
        let plate = wheel_bar_rect(wheel.0, wheel.1, SCREEN).unwrap();
        let track = wheel_bar_slider(plate).shrink2(vec2(7.0, 0.0));
        let mut value = 0.5;
        // A frame of the bar alone; true when the bar says its value changed.
        let run = |events: Vec<Event>, value: &mut f32| {
            let mut changed = false;
            frame(&ctx, events, |ctx| {
                changed |= wheel_bar(ctx, "bar", wheel, "Strength", value, STRENGTH_RANGE, |v| format!("{v}"));
            });
            changed
        };
        // The bar takes a frame to size itself and one to be laid out.
        assert!(!run(vec![], &mut value) && !run(vec![], &mut value));
        assert_eq!(value, 0.5, "showing the bar changes nothing");

        // Pressed on the plate's top edge, well off the thin track.
        let press = pos2(track.left() + track.width() * 0.75, plate.top() + 1.0);
        let pressed = run(vec![Event::PointerMoved(press), button(press, true)], &mut value);
        assert!(pressed | run(vec![], &mut value));
        assert!((value - 0.7525).abs() < 1e-4, "{value}");
        let quarter = pos2(track.left() + track.width() * 0.25, plate.center().y);
        assert!(run(vec![Event::PointerMoved(quarter)], &mut value));
        assert!((value - 0.2575).abs() < 1e-4, "{value}");
        // Off the plate altogether, still held: the ends hold.
        run(vec![Event::PointerMoved(pos2(-50.0, 10.0))], &mut value);
        assert_eq!(value, 0.01);
        let far = pos2(1590.0, 890.0);
        run(vec![Event::PointerMoved(far)], &mut value);
        assert_eq!(value, 1.0);
        let let_go = run(vec![button(far, false)], &mut value);
        let moved = run(vec![Event::PointerMoved(quarter)], &mut value);
        assert!(!let_go && !moved && value == 1.0, "nothing moves it once the pen is up");

        // A press beside the plate is not the bar's.
        let beside = pos2(plate.right() + 4.0, plate.center().y);
        let pressed = run(vec![Event::PointerMoved(beside), button(beside, true)], &mut value);
        let released = run(vec![button(beside, false)], &mut value);
        assert!(!pressed && !released && value == 1.0);
    }

    /// Every base brush but Erase Layer and Paint starts on the wheel once, with the
    /// mask tile, Move at the top; and however many tiles a wheel has, no two of them
    /// overlap, each is picked from its own middle only, and all of it is
    /// inside the wheel's own square.
    #[test]
    fn the_brush_wheel_has_room_for_every_brush_however_many_there_are() {
        let library = Library::default();
        let tiles = wheel_tiles(&library);
        // The base brushes but two, and the mask tile.
        assert_eq!(tiles.len(), Brush::ALL.len() - 1);
        assert_eq!(tiles[0], Some(Library::base(Brush::Move)));
        assert_eq!(tiles.iter().filter(|tile| tile.is_none()).count(), 1);
        for brush in Brush::ALL {
            let on_it = tiles.iter().filter(|&&tile| tile == Some(Library::base(brush))).count();
            assert_eq!(on_it, usize::from(brush.starts_on_wheel()), "{brush:?}");
        }
        assert_eq!(brush_wheel_ring(&library), WheelLayout::brushes(13).ring);
        // A base brush's tile has no initials in its corner; another brush's has.
        assert_eq!(brush_tile(&library, Some(Library::base(Brush::SnakeHook))), ("Snake Hook", "brush_snakehook", String::new()));
        let strips = library.named("Clay Strips");
        assert_eq!(brush_tile(&library, strips), ("Clay Strips", "brush_clay", "CS".to_owned()));
        assert_eq!(brush_tile(&library, None), ("Mask", "brush_mask", String::new()));
        // The wheel as it was while there were nine tiles or fewer.
        let nine = WheelLayout::brushes(9);
        assert_eq!((nine.half, nine.disc, nine.ring, nine.tile, nine.pick), (152.0, 146.0, 104.0, 54.0, 31.0));
        for count in [1usize, 2, 9, 10, 13, 20, 40] {
            let wheel = WheelLayout::brushes(count);
            // Neighbours on the ring are a chord apart.
            let apart = if count > 1 { 2.0 * wheel.ring * (std::f32::consts::PI / count as f32).sin() } else { f32::MAX };
            assert!(count <= 2 || apart >= wheel.tile + 1.0, "{count} tiles of {} are {apart} apart", wheel.tile);
            assert!(count <= 2 || wheel.pick * 2.0 <= apart, "{count} tiles: two are picked at once");
            // The tiles grow a tenth under the pointer.
            assert!(wheel.ring + wheel.tile * 0.55 <= wheel.disc && wheel.disc < wheel.half, "{count} tiles spill off the wheel");
        }
    }
}
