//! The brush falloff curve editor: a small graph of weight (up) against
//! distance from the brush centre (left = centre, right = edge).
//!
//! Drag a point to move it; click empty space to add one; right-click a
//! point, or drag it well off the graph, to remove it. The two end points
//! only move up and down: the curve always spans the whole radius. A point in
//! between cannot be dragged past its neighbours, so the points never change
//! places under the pointer in the middle of a drag.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use glam::Vec2;

use super::theme::{self, ACCENT, ButtonStyle};
use crate::sculpt::{FalloffCurve, FalloffPreset, Settings, focal_k, shifted};

const GRAPH_HEIGHT: f32 = 110.0;
/// Room round the plot, so the end points can be seen and grabbed whole.
const INSET: f32 = 8.0;
const POINT_SIZE: f32 = 7.0;
const PICK_RADIUS: f32 = 10.0;
/// How far off the graph a point is dragged before it is taken out.
const REMOVE_DISTANCE: f32 = 28.0;
const SEGMENTS: usize = 64;

const GRID: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 20);
const CURVE: Color32 = Color32::from_rgb(89, 166, 255);
/// The curve the brush really uses while Focal Shift is not 0.
const SHIFTED: Color32 = Color32::from_rgba_premultiplied(178, 143, 62, 178);

/// The Falloff Curve part of the brush settings: the tick box that swaps the
/// current brush's own soft edge for a hand-drawn curve and, while it is
/// ticked, the graph and the shapes to start from. Each brush keeps its own
/// curve. Not shown while mask painting, which has its Hardness instead.
pub fn section(ui: &mut Ui, s: &mut Settings, tips: bool) {
    theme::heading(ui, "Falloff Curve");
    let slot = s.brush.index();
    let mut custom = s.falloff[slot].is_some();
    let tick = theme::checkbox(ui, &mut custom, "Custom Falloff (this brush)", ACCENT);
    let tick = theme::tip(
        tick,
        tips,
        "Replace this brush's own soft edge with a curve you draw. Each brush keeps its own curve.",
    );
    if tick.changed() {
        s.falloff[slot] = custom.then(|| FalloffCurve::preset(FalloffPreset::Smooth));
    }
    let focal = s.current().focal_shift;
    let Some(curve) = s.falloff[slot].as_mut() else { return };

    graph(ui, curve, focal);
    theme::hint(
        ui,
        "Left: brush centre. Right: edge. Drag a point; click to add one; right-click one, or drag it off the graph, to remove it.",
    );
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, FalloffPreset::ALL.len(), 4.0);
        for preset in FalloffPreset::ALL {
            let shape = FalloffCurve::preset(preset);
            let style = ButtonStyle { active: (*curve == shape).then_some(ACCENT), ..Default::default() };
            let text = format!("Start this brush's curve from the {} shape.", preset.name());
            if theme::tip(theme::button(ui, w, preset.name(), style), tips, &text).clicked() {
                *curve = shape;
            }
        }
    });
}

fn graph(ui: &mut Ui, curve: &mut FalloffCurve, focal: f32) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), GRAPH_HEIGHT), Sense::click_and_drag());
    let plot = rect.shrink(INSET);
    let to_screen = |p: Vec2| pos2(plot.left() + p.x * plot.width(), plot.bottom() - p.y * plot.height());
    let to_curve = |pos: Pos2| {
        Vec2::new((pos.x - plot.left()) / plot.width(), (plot.bottom() - pos.y) / plot.height())
            .clamp(Vec2::ZERO, Vec2::ONE)
    };
    let pick = |curve: &FalloffCurve, pos: Pos2| {
        let mut best = None;
        let mut nearest = PICK_RADIUS;
        for (i, &p) in curve.points.iter().enumerate() {
            let d = to_screen(p).distance(pos);
            if d <= nearest {
                (nearest, best) = (d, Some(i));
            }
        }
        best
    };

    // Which point a drag holds, kept between frames.
    let drag_id = response.id.with("point");
    let mut held: Option<usize> = ui.data(|d| d.get_temp(drag_id));
    let (pressed, down) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down()));
    let pointer = response.interact_pointer_pos();
    let last = curve.points.len().saturating_sub(1);

    if response.secondary_clicked() {
        // The ends stay: the curve always spans the whole radius.
        if let Some(i) = pointer.and_then(|pos| pick(curve, pos)).filter(|&i| i > 0 && i < last) {
            curve.points.remove(i);
            curve.normalize();
        }
        held = None;
    } else if let (true, true, Some(pos)) = (pressed, response.is_pointer_button_down_on(), pointer) {
        held = pick(curve, pos);
        if held.is_none() {
            // Put in order of distance, so the new point can be dragged
            // straight away.
            let at = to_curve(pos);
            let mut insert = 1;
            while insert < last && curve.points[insert].x < at.x {
                insert += 1;
            }
            curve.points.insert(insert.clamp(0, curve.points.len()), at);
            curve.normalize();
            held = Some(insert);
        }
    } else if let (true, Some(i), Some(pos)) = (down, held.filter(|&i| i <= last), pointer) {
        let at = to_curve(pos);
        let interior = i > 0 && i < last;
        if interior && rect.distance_to_pos(pos) > REMOVE_DISTANCE {
            curve.points.remove(i);
            held = None;
        } else {
            let x = if i == 0 {
                0.0
            } else if i == last {
                1.0
            } else {
                // Not past its neighbours. The lower bound wins if two
                // neighbours have been brought closer than this together.
                at.x.min(curve.points[i + 1].x - 0.01).max(curve.points[i - 1].x + 0.01)
            };
            curve.points[i] = Vec2::new(x, at.y);
        }
        curve.normalize();
    } else if !down {
        held = None;
    }
    ui.data_mut(|d| match held {
        Some(i) => {
            d.insert_temp(drag_id, i);
        }
        None => d.remove::<usize>(drag_id),
    });

    if !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 5.0, theme::FIELD);
    for k in 0..=4 {
        let t = k as f32 / 4.0;
        let (x, y) = (plot.left() + plot.width() * t, plot.top() + plot.height() * t);
        p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, GRID));
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, GRID));
    }
    let trace = |weight: &dyn Fn(f32) -> f32| -> Vec<Pos2> {
        (0..=SEGMENTS)
            .map(|i| {
                let u = i as f32 / SEGMENTS as f32;
                to_screen(Vec2::new(u, weight(u)))
            })
            .collect()
    };
    p.line(trace(&|u| curve.evaluate(u)), Stroke::new(2.0, CURVE));
    if focal != 0.0 {
        // The points stay where they were drawn; this is where the shift
        // puts the weight.
        let k = focal_k(focal);
        p.line(trace(&|u| curve.evaluate(shifted(u, k))), Stroke::new(2.0, SHIFTED));
    }
    let hovered = response.hover_pos().and_then(|pos| pick(curve, pos));
    for (i, &point) in curve.points.iter().enumerate() {
        let lit = held == Some(i) || (held.is_none() && hovered == Some(i));
        let size = if lit { POINT_SIZE + 3.0 } else { POINT_SIZE };
        let square = Rect::from_center_size(to_screen(point), vec2(size, size));
        p.rect_filled(square, 1.5, if lit { ACCENT } else { Color32::WHITE });
    }
}
