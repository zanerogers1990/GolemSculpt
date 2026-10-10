//! What the tool draws over the viewport: the axis, the profile curve on both silhouettes, and the
//! control-point handles.
//!
//! Drawn flat over the viewport rather than as 3D lines, so it stays crisp and constant-width at any
//! zoom and always sits on top of the preview it annotates (the silhouette handles would otherwise
//! z-fight with the very surface they shape).

use glam::Vec2;
use sculpt_toolkit::{Color32, Ctx, Overlay, View};

use super::{Drag, Lathe};
use crate::curve;

const AXIS: Color32 = Color32::from_rgba_premultiplied(112, 115, 127, 140);
const AXIS_LIT: Color32 = Color32::from_rgba_premultiplied(180, 186, 205, 230);
const CURVE: Color32 = Color32::from_rgba_premultiplied(242, 155, 53, 242);
const MIRROR_CURVE: Color32 = Color32::from_rgba_premultiplied(89, 57, 20, 89);
const WARN_CURVE: Color32 = Color32::from_rgba_premultiplied(230, 73, 73, 242);
const MIRROR_WARN_CURVE: Color32 = Color32::from_rgba_premultiplied(97, 31, 31, 102);
const HANDLE: Color32 = Color32::from_rgb(247, 247, 247);
const HANDLE_OUTLINE: Color32 = Color32::from_rgba_premultiplied(23, 23, 28, 230);
const SELECTED: Color32 = Color32::from_rgb(255, 163, 56);
const POLE: Color32 = Color32::from_rgb(115, 204, 255);
const MIRROR_HANDLE: Color32 = Color32::from_rgba_premultiplied(99, 99, 99, 102);
const INSERT: Color32 = Color32::from_rgb(128, 242, 140);
pub(super) const SKETCH: Color32 = Color32::from_rgba_premultiplied(242, 218, 84, 242);

/// Handle radii in points.
const HANDLE_RADIUS: f32 = 5.0;
const LIT_RADIUS: f32 = 6.5;
const MIRROR_RADIUS: f32 = 3.5;

impl Lathe {
    pub(super) fn draw_overlay(&self, cx: &Ctx, overlay: &mut Overlay) {
        let view = &cx.view;
        self.draw_axis(view, overlay);
        self.draw_curve(view, overlay);
        self.draw_handles(view, overlay);
        if let Some(insert) = self.hover.insert.filter(|_| !self.dragging())
            && let Some(at) = self.plane.screen(view, insert.position, insert.side)
        {
            overlay.dot(at, 4.0, INSERT, HANDLE_OUTLINE);
        }
        if let Drag::Sketch { points, .. } = &self.drag {
            let on_screen: Vec<Vec2> = points.iter().filter_map(|&p| self.plane.screen(view, p, 1.0)).collect();
            overlay.polyline(&on_screen, 2.0, SKETCH, false);
        }
    }

    /// The axis, a little past the shape at both ends. Brighter while it can be grabbed.
    fn draw_axis(&self, view: &View, overlay: &mut Overlay) {
        let (low, high) = self.axis_span();
        let (Some(a), Some(b)) =
            (self.plane.screen(view, Vec2::new(0.0, low), 1.0), self.plane.screen(view, Vec2::new(0.0, high), 1.0))
        else {
            return;
        };
        let lit = self.hover.axis || matches!(self.drag, Drag::Axis { .. });
        if lit {
            overlay.dashed(a, b, 2.0, AXIS_LIT, 8.0, 5.0);
        } else {
            overlay.dashed(a, b, 1.5, AXIS, 8.0, 5.0);
        }
    }

    fn draw_curve(&self, view: &View, overlay: &mut Overlay) {
        if self.built.has_mesh {
            // As tessellated, so it traces the mesh's actual edge.
            let warn = self.build.self_intersecting || self.build.touches_axis;
            self.stroke_both_sides(view, overlay, &self.build.profile, self.build.profile_is_loop, warn);
        } else if self.profile.len() >= 2 {
            // The smooth curve straight from the control points, drawn when there is no mesh to trace (every
            // point on the axis, say), so the artist still sees what they are drawing.
            let points = self.profile.points();
            let closed = self.profile.closed_loop();
            let mut curve_points = Vec::new();
            for segment in 0..curve::segment_count(points, closed) {
                let first = if segment == 0 { 0 } else { 1 };
                for step in first..=16 {
                    curve_points.push(curve::evaluate(points, closed, segment, step as f32 / 16.0));
                }
            }
            self.stroke_both_sides(view, overlay, &curve_points, false, false);
        }
    }

    /// The mirror side first, so the canonical one sits on top where they overlap.
    fn stroke_both_sides(&self, view: &View, overlay: &mut Overlay, curve_points: &[Vec2], closed: bool, warn: bool) {
        for side in [-1.0f32, 1.0] {
            let (color, width) = match (side > 0.0, warn) {
                (true, false) => (CURVE, 2.0),
                (true, true) => (WARN_CURVE, 2.0),
                (false, false) => (MIRROR_CURVE, 1.5),
                (false, true) => (MIRROR_WARN_CURVE, 1.5),
            };
            let on_screen: Vec<Option<Vec2>> =
                curve_points.iter().map(|&p| self.plane.screen(view, p, side)).collect();
            if on_screen.iter().all(Option::is_some) {
                let all: Vec<Vec2> = on_screen.into_iter().flatten().collect();
                overlay.polyline(&all, width, color, closed);
                continue;
            }
            // Part of the curve is behind the camera: draw what is in front of it, run by run.
            for run in on_screen.split(Option::is_none) {
                let run: Vec<Vec2> = run.iter().flatten().copied().collect();
                overlay.polyline(&run, width, color, false);
            }
        }
    }

    /// Handles over the curve: the mirror side first, so the canonical ones sit on top where they
    /// overlap. A corner is a square.
    fn draw_handles(&self, view: &View, overlay: &mut Overlay) {
        for side in [-1.0f32, 1.0] {
            for (i, point) in self.profile.points().iter().enumerate() {
                let on_axis = point.position.x <= 0.0;
                // The same place as the canonical one.
                if on_axis && side < 0.0 {
                    continue;
                }
                let Some(at) = self.plane.screen(view, point.position, side) else { continue };
                let selected = self.selected == Some(i);
                let hovered = self.hover.point.is_some_and(|(h, _)| h == i);
                let radius = match (side > 0.0, hovered || selected) {
                    (true, true) => LIT_RADIUS,
                    (true, false) => HANDLE_RADIUS,
                    (false, _) => MIRROR_RADIUS,
                };
                let fill = if side < 0.0 {
                    MIRROR_HANDLE
                } else if selected {
                    SELECTED
                } else if on_axis {
                    POLE
                } else {
                    HANDLE
                };
                if point.sharp {
                    overlay.square(at, radius, fill, HANDLE_OUTLINE);
                } else {
                    overlay.dot(at, radius, fill, HANDLE_OUTLINE);
                }
            }
        }
    }
}
