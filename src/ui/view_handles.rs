//! Three round handles down the left of the view gizmo's buttons: a hand
//! beside Persp to drag the view sideways (pan), a magnifier beside Stroke
//! Pivot to drag it nearer or further (zoom, up is nearer, as with Ctrl +
//! Alt + drag), and a house beside Frame for Home, which glides back to the
//! view a scene starts with. With these and the axis ball the camera goes
//! everywhere it can go with a pen alone, no keyboard or middle button in
//! reach.
//!
//! They sit beside the buttons and not in a row of their own because the
//! gizmo cannot grow: the panels' tabs hang under it, and on a small window
//! there is no height to spare above the bottom bar, nor width beside the
//! status plate (`panels::tests`, `status::tests`).
//!
//! The drags are the same moves the viewport's own make (`Action::PanView`,
//! `Action::ZoomView`, carried out in app/view_moves.rs), so they feel the
//! same.

use egui::{Color32, Pos2, Response, Sense, Shape, Stroke, Ui, Vec2, vec2};

use super::theme;
use crate::doc::Action;

/// Each handle is a round button this wide, which is also the height of the
/// gizmo's button rows.
pub(super) const SIZE: f32 = 26.0;

#[derive(Clone, Copy)]
pub(super) enum Handle {
    Pan,
    Zoom,
    Home,
}

/// One handle, pushing what it was used for.
pub(super) fn handle(ui: &mut Ui, kind: Handle, tips: bool, actions: &mut Vec<Action>) {
    let response = drawn(ui, kind);
    let ppp = ui.ctx().pixels_per_point();
    let help = match kind {
        Handle::Pan => {
            if response.dragged() {
                let d = response.drag_delta() * ppp;
                actions.push(Action::PanView([d.x, d.y]));
            }
            "Drag to move the view sideways: the model follows the pen."
        }
        Handle::Zoom => {
            if response.dragged() {
                actions.push(Action::ZoomView(response.drag_delta().y * ppp));
            }
            "Drag up to come nearer, down to go further away."
        }
        Handle::Home => {
            if response.clicked() {
                actions.push(Action::HomeView);
            }
            "Home: back to the view a scene starts with, the whole scene from a little to one side and above, in \
             perspective."
        }
    };
    if tips {
        sculpt_toolkit::translate::hover(response, help);
    }
}

/// The handle with its picture drawn on it.
fn drawn(ui: &mut Ui, kind: Handle) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::click_and_drag());
    let p = ui.painter();
    let c = rect.center();
    let lit = response.hovered() || response.dragged();
    let face = if lit { theme::ACCENT().gamma_multiply(0.85) } else { theme::CONTROL() };
    p.circle_filled(c, SIZE * 0.5, face);
    let ink = if lit { Color32::WHITE } else { theme::TEXT() };
    let line = Stroke::new(1.5, ink);
    match kind {
        Handle::Pan => {
            // Four arrows out from the middle.
            for d in [vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0)] {
                let tip = c + d * 7.5;
                p.line_segment([c + d * 1.5, tip], line);
                let side = vec2(-d.y, d.x) * 2.6;
                p.add(Shape::convex_polygon(vec![tip + d * 1.8, tip - d * 1.8 + side, tip - d * 1.8 - side], ink, Stroke::NONE));
            }
        }
        Handle::Zoom => {
            let lens = c + vec2(-2.0, -2.0);
            p.circle_stroke(lens, 5.0, Stroke::new(1.7, ink));
            p.line_segment([lens + vec2(3.6, 3.6), c + vec2(6.5, 6.5)], Stroke::new(2.2, ink));
        }
        Handle::Home => {
            let at = |x: f32, y: f32| -> Pos2 { c + vec2(x, y) };
            p.add(Shape::convex_polygon(vec![at(0.0, -7.5), at(7.5, -0.5), at(-7.5, -0.5)], ink, Stroke::NONE));
            p.rect_filled(egui::Rect::from_min_max(at(-5.0, -0.5), at(5.0, 6.5)), 1.0, ink);
            // The door, in the handle's own colour.
            p.rect_filled(egui::Rect::from_min_max(at(-1.5, 2.0), at(1.5, 6.5)), 0.5, face);
        }
    }
    response
}
