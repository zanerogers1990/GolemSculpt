//! The array as it looks over the viewport, and picking what is drawn: the path
//! the copies follow, the handles on it, the ring and its axis for a circle.
//!
//! Handles are the Unity tool's: a dot for every point along a path, a square
//! for the object's own point (the path's start) and for a circle's centre,
//! larger and brighter when hovered or picked.

use egui::Color32;
use glam::{Vec2, Vec3};
use sculpt_toolkit::{Ctx, Draw, View};

use crate::layout::{axis_vector, radial_arc};
use crate::path::{self, STEPS_PER_SEGMENT};
use crate::state::ArrayState;
use crate::types::ArrayKind;

/// Points of the ring drawn for a circle.
const ARC_STEPS: usize = 96;
/// How close (interface points) a press must be to a handle, or to the path to
/// add a point to it.
pub const HANDLE_PICK: f32 = 11.0;
pub const CURVE_PICK: f32 = 8.0;

const PATH: Color32 = Color32::from_rgba_unmultiplied_const(92, 204, 255, 242);
const GUIDE: Color32 = Color32::from_rgba_unmultiplied_const(92, 204, 255, 115);
const HANDLE: Color32 = Color32::from_rgb(247, 247, 247);
const HANDLE_OUTLINE: Color32 = Color32::from_rgba_unmultiplied_const(26, 26, 31, 230);
const PICKED: Color32 = Color32::from_rgb(92, 204, 255);
const OBJECT_POINT: Color32 = Color32::from_rgb(255, 209, 77);
const INSERT: Color32 = Color32::from_rgb(128, 242, 140);
const STROKE: Color32 = Color32::from_rgba_unmultiplied_const(255, 230, 89, 230);

/// The path in world space as a polyline, with the control segment each of its
/// points lies on (a circle has none: it has no points to add).
#[derive(Default)]
pub struct Picture {
    pub path: Vec<Vec3>,
    segment: Vec<usize>,
}

/// Where on the path a press landed: the segment and how far along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveHit {
    pub segment: usize,
    pub t: f32,
    pub world: Vec3,
}

/// What is lit on top of the plain handles.
#[derive(Default)]
pub struct Marks {
    pub picked: Option<usize>,
    pub hovered: Option<usize>,
    /// Where a click on the path would add a point.
    pub insert: Option<Vec3>,
}

impl Picture {
    /// The path of `state` for the object at `origin`.
    pub fn new(state: &ArrayState, origin: Vec3) -> Picture {
        let settings = &state.settings;
        if settings.kind == ArrayKind::Radial {
            if state.points.len() < 2 {
                return Picture::default();
            }
            let path = radial_arc(settings, &state.points, ARC_STEPS).into_iter().map(|p| origin + p).collect();
            return Picture { path, segment: Vec::new() };
        }
        let sampled = path::sample(&state.points, settings.closed);
        Picture { path: sampled.points.into_iter().map(|p| origin + p).collect(), segment: sampled.segment_of_point }
    }
}

/// How far `p` is from the segment `a` to `b` on screen, and how far along the
/// segment (0 to 1) the nearest point is.
fn screen_distance(p: Vec2, a: Vec2, b: Vec2) -> (f32, f32) {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-6 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((a + ab * t).distance(p), t)
}

/// The handle under a pixel, if any. The picked one is favoured a hair, and a
/// path point over the object's own where they overlap (a closed loop's ends).
pub fn pick_handle(view: &View, state: &ArrayState, origin: Vec3, px: Vec2, picked: Option<usize>) -> Option<usize> {
    let mut best = None;
    let mut best_distance = HANDLE_PICK * view.pixels_per_point;
    for i in 0..state.handle_count() {
        let Some(screen) = view.project(origin + state.points[i]) else { continue };
        let mut d = screen.distance(px);
        if picked == Some(i) {
            d -= 0.5;
        }
        if i == 0 {
            d += 0.5;
        }
        if d < best_distance {
            best_distance = d;
            best = Some(i);
        }
    }
    best
}

/// The nearest point of the path within reach of a pixel: where a click adds a
/// point. A circle has no points to add, so it never hits.
pub fn pick_curve(view: &View, state: &ArrayState, picture: &Picture, px: Vec2) -> Option<CurveHit> {
    if !state.settings.is_path() || picture.segment.len() != picture.path.len() {
        return None;
    }
    let mut best = None;
    let mut best_distance = CURVE_PICK * view.pixels_per_point;
    for i in 1..picture.path.len() {
        let (Some(a), Some(b)) = (view.project(picture.path[i - 1]), view.project(picture.path[i])) else { continue };
        let (d, f) = screen_distance(px, a, b);
        if d >= best_distance {
            continue;
        }
        best_distance = d;
        // Polyline point i is step (i - segment * steps) of its control segment.
        let segment = picture.segment[i];
        let step = i - segment * STEPS_PER_SEGMENT;
        best = Some(CurveHit {
            segment,
            t: ((step as f32 - 1.0 + f) / STEPS_PER_SEGMENT as f32).clamp(0.0, 1.0),
            world: picture.path[i - 1].lerp(picture.path[i], f),
        });
    }
    best
}

/// Draws the path, the circle's guides and every handle of an array, then the
/// spot a click would add a point at. `object_size` is the object's rough
/// size, which the axis through a circle's centre is drawn long enough to
/// clear.
pub fn draw(
    cx: &Ctx,
    draw: &mut Draw,
    state: &ArrayState,
    origin: Vec3,
    picture: &Picture,
    marks: &Marks,
    object_size: f32,
) {
    let view = &cx.view;
    draw.overlay.polyline_3d(view, &picture.path, 2.0, PATH);
    if state.settings.kind == ArrayKind::Radial && state.points.len() > 1 {
        draw_circle_guides(cx, draw, state, origin, object_size);
    }

    let radial = state.settings.kind == ArrayKind::Radial;
    for i in 0..state.handle_count() {
        let Some(screen) = view.project(origin + state.points[i]) else { continue };
        let lit = marks.hovered == Some(i) || marks.picked == Some(i);
        let radius = if lit { 6.5 } else { 5.0 };
        let fill = if marks.picked == Some(i) {
            PICKED
        } else if i == 0 {
            OBJECT_POINT
        } else {
            HANDLE
        };
        // Square: the object itself and a circle's centre, the two points that
        // are not somewhere along the path.
        if i == 0 || radial {
            draw.overlay.square(screen, radius, fill, HANDLE_OUTLINE);
        } else {
            draw.overlay.dot(screen, radius, fill, HANDLE_OUTLINE);
        }
    }

    if let Some(screen) = marks.insert.and_then(|p| view.project(p)) {
        draw.overlay.dot(screen, 4.0, INSERT, HANDLE_OUTLINE);
    }
}

/// A stroke being drawn, in world space.
pub fn draw_stroke(cx: &Ctx, draw: &mut Draw, stroke: &[Vec3]) {
    if stroke.len() > 1 {
        draw.overlay.polyline_3d(&cx.view, stroke, 1.5, STROKE);
    }
}

/// The spoke from the centre to the object, and the axis through the centre.
fn draw_circle_guides(cx: &Ctx, draw: &mut Draw, state: &ArrayState, origin: Vec3, object_size: f32) {
    let view = &cx.view;
    let object = origin + state.points[0];
    let centre = origin + state.points[1];
    draw.overlay.line_3d(view, object, centre, 1.2, GUIDE);

    let axis = axis_vector(state.settings.axis);
    let reach = ((centre - object).length() * 0.6).max(object_size * 0.5);
    if let (Some(lo), Some(hi)) = (view.project(centre - axis * reach), view.project(centre + axis * reach)) {
        draw.overlay.dashed(lo, hi, 1.5, GUIDE, 6.0, 4.0);
    }
}

#[cfg(test)]
mod tests {
    use sculpt_toolkit::testing::Bench;

    use super::*;
    use crate::types::ArraySettings;

    fn state(kind: ArrayKind, points: Vec<Vec3>) -> ArrayState {
        ArrayState { settings: ArraySettings::new(kind), points }
    }

    #[test]
    fn a_line_is_a_dense_path_with_its_segments_and_a_circle_is_a_ring_without() {
        let line = state(ArrayKind::Line, vec![Vec3::ZERO, Vec3::X * 3.0]);
        let picture = Picture::new(&line, Vec3::Y);
        assert_eq!(picture.path.len(), STEPS_PER_SEGMENT + 1);
        assert!(picture.path[0].distance(Vec3::Y) < 1e-6, "relative to the object");
        assert!(picture.path.last().unwrap().distance(Vec3::new(3.0, 1.0, 0.0)) < 1e-5);

        let ring = state(ArrayKind::Radial, vec![Vec3::ZERO, Vec3::X * -2.0]);
        let picture = Picture::new(&ring, Vec3::ZERO);
        assert_eq!(picture.path.len(), ARC_STEPS + 1);
        assert!(
            picture.path[0].distance(Vec3::ZERO) < 1e-5 && picture.path[ARC_STEPS].distance(Vec3::ZERO) < 1e-4,
            "a full ring closes"
        );
        assert!(
            pick_curve(&Bench::new().env.view, &ring, &picture, Vec2::ZERO).is_none(),
            "nothing to add to a circle"
        );
    }

    #[test]
    fn a_handle_is_picked_within_reach_and_the_object_point_gives_way_to_a_path_point() {
        let bench = Bench::new();
        let view = bench.env.view;
        let line = state(ArrayKind::Line, vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0]);
        let end = view.project(Vec3::X * 2.0).unwrap();
        assert_eq!(pick_handle(&view, &line, Vec3::ZERO, end + Vec2::new(6.0, 4.0), None), Some(2));
        assert_eq!(pick_handle(&view, &line, Vec3::ZERO, end + Vec2::new(40.0, 0.0), None), None);

        // A closed loop's last point sits on the object's: the path point wins a tie.
        let loop_ = state(ArrayKind::Curve, vec![Vec3::ZERO, Vec3::X, Vec3::ZERO]);
        let at = view.project(Vec3::ZERO).unwrap();
        assert_eq!(pick_handle(&view, &loop_, Vec3::ZERO, at, None), Some(2));
        assert_eq!(pick_handle(&view, &loop_, Vec3::ZERO, at, Some(0)), Some(0), "unless the object's own is picked");
    }

    #[test]
    fn a_click_on_the_path_finds_the_segment_and_how_far_along_it() {
        let bench = Bench::new();
        let view = bench.env.view;
        let curve = state(ArrayKind::Curve, vec![Vec3::ZERO, Vec3::new(1.0, 0.5, 0.0), Vec3::new(2.0, 0.0, 0.0)]);
        let picture = Picture::new(&curve, Vec3::ZERO);
        // Ask for the point a known way along the second segment, from its own pixel.
        let target = crate::path::evaluate(&curve.points, false, 1, 0.5);
        let hit = pick_curve(&view, &curve, &picture, view.project(target).unwrap()).expect("on the path");
        assert_eq!(hit.segment, 1);
        assert!((hit.t - 0.5).abs() < 0.05, "{}", hit.t);
        assert!(hit.world.distance(target) < 0.02);
        assert!(pick_curve(&view, &curve, &picture, view.project(Vec3::new(1.0, 2.0, 0.0)).unwrap()).is_none());
    }
}
