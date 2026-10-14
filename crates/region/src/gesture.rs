//! The drag every region tool shares: a box, a lasso or a straight line laid
//! over the viewport, and the marquee that shows it while it is dragged out.
//!
//! Positions are viewport pixels, (0, 0) at the top-left and y running down,
//! which is what the pointer reports, what the overlay draws in and what
//! `sculpt_cut` tests against, so a point is never converted on its way from
//! the pen to the cut.

use egui::Color32;
use glam::Vec2;
use sculpt_cut::ScreenRegion;
use sculpt_cut::region::snap_line_end;
use sculpt_toolkit::{Label, Overlay, Run};

/// Outline width, in points. Thin and translucent on purpose: the marquee is
/// drawn straight over the model the user is looking at, and anything heavier
/// competes with the shape being traced around.
const OUTLINE: f32 = 1.5;
/// How much of the tint the shaded inside of a box or half of a line gets.
const FILL_ALPHA: u8 = 26;
/// The part of a cut line beyond the dragged stretch: the cut runs on without
/// end, so the line is shown right across the viewport, but fainter.
const BEYOND_ALPHA: u8 = 140;
/// Spacing and length (points) of the ticks on the covered side of a line.
const TICK_SPACING: f32 = 24.0;
const TICK_LENGTH: f32 = 7.0;
/// Dash and gap (points) of the edge that closes a lasso back to its start.
const CLOSING_DASH: (f32, f32) = (5.0, 4.0);
/// Radius (points) of the mark on a point a snapping line catches.
const ANCHOR_RADIUS: f32 = 5.0;
/// How far (points) the angle reading stands off the end of a snapping line.
const READING_GAP: f32 = 14.0;

/// What is dragged out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Box,
    Lasso,
    /// A straight line, extended without end: everything on its left, looking
    /// from where the drag began to where it ended, is covered.
    Line,
}

impl Shape {
    pub fn name(self) -> &'static str {
        match self {
            Shape::Box => "Box",
            Shape::Lasso => "Lasso",
            Shape::Line => "Line",
        }
    }
}

/// What a line snaps to, for a cut that has to be exact rather than judged by
/// eye: a level or upright line, and one that goes through the very middle
/// of the object.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LineSnap {
    /// Off, the line is the drag as it was made and the rest is not read.
    pub on: bool,
    /// Points on screen worth cutting exactly through: the middle of the
    /// selected object, and its origin when that is somewhere else.
    pub anchors: [Option<Vec2>; 2],
    /// How near, in pixels, the line has to pass to one of them to be moved
    /// onto it.
    pub reach: f32,
}

impl LineSnap {
    pub const OFF: LineSnap = LineSnap { on: false, anchors: [None; 2], reach: 0.0 };

    /// Marks the points a line would catch, the one it has caught filled in.
    pub fn draw_anchors(&self, overlay: &mut Overlay, tint: Color32, caught: Option<Vec2>) {
        if !self.on {
            return;
        }
        for (i, anchor) in self.anchors.iter().enumerate() {
            let Some(at) = *anchor else { continue };
            let fill = if caught == Some(at) { tint } else { faded(tint, FILL_ALPHA) };
            // Round for the middle, square for the origin.
            match i {
                0 => overlay.dot(at, ANCHOR_RADIUS, fill, Color32::WHITE),
                _ => overlay.square(at, ANCHOR_RADIUS, fill, Color32::WHITE),
            }
        }
    }
}

/// How the marquee of a drag in progress is drawn.
#[derive(Clone, Copy, Debug)]
pub struct Marquee {
    pub tint: Color32,
    /// Viewport size in pixels: a line is drawn, and its half shaded, right
    /// across it.
    pub viewport: Vec2,
    pub pixels_per_point: f32,
    /// What a line snaps to.
    pub snap: LineSnap,
    /// Shade the other side of a line: the drag will act on that one.
    pub other_side: bool,
}

/// A region being dragged out.
#[derive(Clone, Debug)]
pub struct Drag {
    pub shape: Shape,
    /// Started with the right button, which reverses the action for the whole
    /// drag whatever Ctrl does later.
    pub inverted: bool,
    start: Vec2,
    current: Vec2,
    /// The lasso's path so far.
    path: Vec<Vec2>,
}

impl Drag {
    pub fn begin(shape: Shape, at: Vec2, inverted: bool) -> Drag {
        Drag { shape, inverted, start: at, current: at, path: vec![at] }
    }

    /// The pointer moved to `at`. A lasso records it only once it is `spacing`
    /// pixels from the last recorded point: every sample of a slow drag would
    /// put hundreds of near-identical points on the path, all of them walked
    /// later for no added accuracy.
    pub fn move_to(&mut self, at: Vec2, spacing: f32) {
        self.current = at;
        if self.shape == Shape::Lasso && self.path.last().is_none_or(|last| last.distance(at) >= spacing) {
            self.path.push(at);
        }
    }

    /// The two ends of a line drag. With `snap` on the line is turned to the
    /// nearest 15 degrees about its start, then moved sideways, keeping that
    /// angle, onto the nearest anchor it passes within reach of.
    pub fn line(&self, snap: LineSnap) -> (Vec2, Vec2) {
        let (a, b, _) = self.snapped_line(snap);
        (a, b)
    }

    /// [`Drag::line`], and the anchor the line was moved onto, if it was.
    pub fn snapped_line(&self, snap: LineSnap) -> (Vec2, Vec2, Option<Vec2>) {
        if !snap.on {
            return (self.start, self.current, None);
        }
        let (a, b) = (self.start, snap_line_end(self.start, self.current));
        let Some(along) = (b - a).try_normalize() else { return (a, b, None) };
        let across = Vec2::new(along.y, -along.x);
        // How far to the side of the line each anchor is; the nearest wins.
        let off = |anchor: Vec2| (anchor - a).dot(across);
        let nearest = snap.anchors.iter().flatten().copied().min_by(|&p, &q| off(p).abs().total_cmp(&off(q).abs()));
        match nearest.filter(|&anchor| off(anchor).abs() <= snap.reach) {
            Some(anchor) => (a + across * off(anchor), b + across * off(anchor), Some(anchor)),
            None => (a, b, None),
        }
    }

    /// Whether the drag went nowhere: a click, which resets rather than
    /// selects. `slop` absorbs the tremor in a click while leaving a
    /// deliberate little box working.
    pub fn is_click(&self, slop: f32) -> bool {
        match self.shape {
            Shape::Line => self.start.distance(self.current) < slop * 2.0,
            Shape::Box => {
                let span = (self.current - self.start).abs();
                span.x < slop && span.y < slop
            }
            Shape::Lasso => self.path.windows(2).map(|pair| pair[0].distance(pair[1])).sum::<f32>() < slop * 2.0,
        }
    }

    /// The dragged shape as the stencil `sculpt_cut` tests points against.
    pub fn region(&self, snap: LineSnap) -> Result<ScreenRegion, String> {
        match self.shape {
            Shape::Box => ScreenRegion::rect(self.start, self.current),
            Shape::Lasso => ScreenRegion::lasso(&self.path),
            Shape::Line => {
                let (a, b) = self.line(snap);
                ScreenRegion::half_plane(a, b)
            }
        }
    }

    /// Draws the shape as it stands.
    pub fn draw(&self, overlay: &mut Overlay, look: Marquee) {
        match self.shape {
            Shape::Box => {
                let (lo, hi) = (self.start.min(self.current), self.start.max(self.current));
                let corners = [lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)];
                overlay.fill(&corners, faded(look.tint, FILL_ALPHA));
                overlay.polyline(&corners, OUTLINE, look.tint, true);
            }
            Shape::Lasso => {
                // Outlined, not filled: the overlay fills convex shapes only.
                overlay.polyline(&self.path, OUTLINE, look.tint, false);
                // The closing edge is part of the region the release will
                // select, so it is shown; dashed, because it was not drawn.
                if let (true, Some(&first), Some(&last)) = (self.path.len() > 2, self.path.first(), self.path.last()) {
                    overlay.dashed(last, first, OUTLINE, look.tint, CLOSING_DASH.0, CLOSING_DASH.1);
                }
            }
            Shape::Line => {
                let (a, b, caught) = self.snapped_line(look.snap);
                let (from, to) = if look.other_side { (b, a) } else { (a, b) };
                draw_line(overlay, from, to, look);
                look.snap.draw_anchors(overlay, look.tint, caught);
                if look.snap.on {
                    draw_angle(overlay, a, b);
                }
            }
        }
    }
}

/// The cut line from `a` to `b`: its covered half shaded, the whole line
/// across the viewport, and ticks on the covered side, so which side a trim
/// removes (or a split turns into the new piece) reads before the pen lifts.
fn draw_line(overlay: &mut Overlay, a: Vec2, b: Vec2, look: Marquee) {
    let along = b - a;
    let length = along.length();
    if length < 0.5 {
        return;
    }
    let along = along / length;
    // Left of the line as seen on a screen whose y runs down.
    let covered = Vec2::new(along.y, -along.x);
    let size = look.viewport;
    let corners = [Vec2::ZERO, Vec2::new(size.x, 0.0), size, Vec2::new(0.0, size.y)];
    overlay.fill(&clip_to_covered(&corners, a, along), faded(look.tint, FILL_ALPHA));

    let reach = (size.x + size.y) * 2.0;
    let beyond = faded(look.tint, BEYOND_ALPHA);
    overlay.line(a - along * reach, a, OUTLINE, beyond);
    overlay.line(b, b + along * reach, OUTLINE, beyond);
    overlay.line(a, b, OUTLINE * 2.0, look.tint);

    let ticks = ((length / (TICK_SPACING * look.pixels_per_point)) as usize).clamp(1, 40);
    for i in 0..=ticks {
        let p = a.lerp(b, i as f32 / ticks as f32);
        overlay.line(p, p + covered * TICK_LENGTH * look.pixels_per_point, OUTLINE, look.tint);
    }
}

/// The slope of a snapping line, read off beside the end being dragged: 0 for
/// level, 90 for upright, so the step it has settled on is a number and not a
/// guess.
fn draw_angle(overlay: &mut Overlay, a: Vec2, b: Vec2) {
    let Some(along) = (b - a).try_normalize() else { return };
    // Up the screen is up: y runs down here.
    let degrees = (-along.y).atan2(along.x).to_degrees().round().rem_euclid(180.0);
    overlay.label(Label {
        at: b,
        push: along,
        gap: READING_GAP,
        runs: vec![Run::new(format!("{degrees:.0}\u{b0}"), Color32::WHITE).bold()],
        background: Color32::from_rgba_unmultiplied(20, 20, 26, 217),
    });
}

/// The part of a convex polygon on the covered side of the line through
/// `origin` along `direction` (Sutherland-Hodgman against one half-plane).
/// The side is decided exactly as `ScreenRegion::half_plane` decides it, so
/// the shading cannot disagree with the cut.
fn clip_to_covered(polygon: &[Vec2], origin: Vec2, direction: Vec2) -> Vec<Vec2> {
    let side = |p: Vec2| direction.x * (p.y - origin.y) - direction.y * (p.x - origin.x);
    let mut out = Vec::with_capacity(polygon.len() + 2);
    for (i, &a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        let (sa, sb) = (side(a), side(b));
        if sa <= 0.0 {
            out.push(a);
        }
        if (sa <= 0.0) != (sb <= 0.0) {
            out.push(a.lerp(b, sa / (sa - sb)));
        }
    }
    out
}

/// An opaque colour at a given alpha.
pub fn faded(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// `a` washed toward `b` by `t`.
pub fn mixed(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(channel(a.r(), b.r()), channel(a.g(), b.g()), channel(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sculpt_toolkit::Shape as Drawn;

    fn look() -> Marquee {
        Marquee {
            tint: Color32::from_rgb(200, 100, 50),
            viewport: Vec2::new(800.0, 600.0),
            pixels_per_point: 1.0,
            snap: LineSnap::OFF,
            other_side: false,
        }
    }

    fn drag(shape: Shape, points: &[Vec2]) -> Drag {
        let mut drag = Drag::begin(shape, points[0], false);
        for &p in &points[1..] {
            drag.move_to(p, 4.0);
        }
        drag
    }

    #[test]
    fn a_drag_that_went_nowhere_is_a_click() {
        let at = Vec2::new(100.0, 100.0);
        assert!(drag(Shape::Box, &[at, at + Vec2::new(5.0, 5.0)]).is_click(6.0));
        assert!(!drag(Shape::Box, &[at, at + Vec2::new(7.0, 1.0)]).is_click(6.0), "a thin box is still a box");
        assert!(drag(Shape::Line, &[at, at + Vec2::new(11.0, 0.0)]).is_click(6.0));
        assert!(!drag(Shape::Line, &[at, at + Vec2::new(13.0, 0.0)]).is_click(6.0));
        // A lasso is judged by the length of its path, not by where it ended:
        // a loop that comes back to its start is a region.
        let there_and_back = [at, at + Vec2::new(40.0, 0.0), at + Vec2::new(40.0, 40.0), at];
        assert!(!drag(Shape::Lasso, &there_and_back).is_click(6.0));
        assert!(drag(Shape::Lasso, &[at, at + Vec2::new(3.0, 0.0)]).is_click(6.0));
    }

    #[test]
    fn a_lasso_keeps_points_only_at_the_spacing() {
        let mut lasso = Drag::begin(Shape::Lasso, Vec2::ZERO, false);
        for i in 1..=100 {
            lasso.move_to(Vec2::new(i as f32, 0.0), 4.0);
        }
        assert_eq!(lasso.path.len(), 26, "one point every four pixels of a hundred");
        let mut boxed = Drag::begin(Shape::Box, Vec2::ZERO, false);
        boxed.move_to(Vec2::new(50.0, 50.0), 4.0);
        assert_eq!(boxed.path.len(), 1, "only a lasso records its path");
    }

    #[test]
    fn regions_cover_what_the_marquee_shows() {
        let boxed = drag(Shape::Box, &[Vec2::new(300.0, 200.0), Vec2::new(100.0, 50.0)]).region(LineSnap::OFF).unwrap();
        assert!(boxed.contains(Vec2::new(200.0, 100.0)) && !boxed.contains(Vec2::new(301.0, 100.0)));

        let loop_ = [Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0), Vec2::new(300.0, 300.0), Vec2::new(100.0, 300.0)];
        let lasso = drag(Shape::Lasso, &loop_).region(LineSnap::OFF).unwrap();
        assert!(lasso.contains(Vec2::new(200.0, 200.0)) && !lasso.contains(Vec2::new(50.0, 200.0)));

        // Drawn left to right, a line covers what is above it on screen.
        let line = drag(Shape::Line, &[Vec2::new(100.0, 300.0), Vec2::new(500.0, 300.0)]);
        let region = line.region(LineSnap::OFF).unwrap();
        assert!(region.contains(Vec2::new(300.0, 100.0)) && !region.contains(Vec2::new(300.0, 500.0)));

        assert!(drag(Shape::Box, &[Vec2::ONE, Vec2::new(1.0, 80.0)]).region(LineSnap::OFF).is_err(), "no width");
    }

    #[test]
    fn shift_turns_a_line_to_fifteen_degrees() {
        let line = drag(Shape::Line, &[Vec2::new(100.0, 100.0), Vec2::new(300.0, 110.0)]);
        let (a, b) = line.line(LineSnap { on: true, ..LineSnap::OFF });
        assert_eq!(a, Vec2::new(100.0, 100.0));
        assert!((b.y - 100.0).abs() < 1e-3, "three degrees off level snaps level: {b}");
        assert_eq!(line.line(LineSnap::OFF).1, Vec2::new(300.0, 110.0));
    }

    #[test]
    fn a_snapping_line_is_moved_onto_the_nearest_anchor_within_reach() {
        let (middle, origin) = (Vec2::new(400.0, 300.0), Vec2::new(400.0, 330.0));
        let snap = LineSnap { on: true, anchors: [Some(middle), Some(origin)], reach: 12.0 };
        // Nearly level and 9 pixels above the middle: level, and through it.
        let line = drag(Shape::Line, &[Vec2::new(100.0, 291.0), Vec2::new(700.0, 296.0)]);
        let (a, b, caught) = line.snapped_line(snap);
        assert_eq!(caught, Some(middle));
        assert!((a.y - 300.0).abs() < 1e-3 && (b.y - 300.0).abs() < 1e-3, "{a} {b}");
        assert!((a.x - 100.0).abs() < 1e-3, "moved straight sideways, not along itself");
        let region = line.region(snap).unwrap();
        assert!(region.contains(middle - Vec2::Y * 0.01) && !region.contains(middle + Vec2::Y * 0.01));

        // Nearer the origin than the middle: the origin has it.
        let line = drag(Shape::Line, &[Vec2::new(100.0, 322.0), Vec2::new(700.0, 322.0)]);
        assert_eq!(line.snapped_line(snap).2, Some(origin));
        // Out of reach of both: only turned.
        let line = drag(Shape::Line, &[Vec2::new(100.0, 200.0), Vec2::new(700.0, 205.0)]);
        let (a, b, caught) = line.snapped_line(snap);
        assert_eq!((caught, a.y), (None, 200.0));
        assert!((b.y - 200.0).abs() < 1e-3);
        // Snapping off: the drag as it was made, whatever it passes.
        let line = drag(Shape::Line, &[Vec2::new(100.0, 291.0), Vec2::new(700.0, 296.0)]);
        let free = line.snapped_line(LineSnap { on: false, ..snap });
        assert_eq!(free, (Vec2::new(100.0, 291.0), Vec2::new(700.0, 296.0), None));
    }

    #[test]
    fn a_snapping_line_shows_its_anchors_and_its_angle() {
        let middle = Vec2::new(400.0, 300.0);
        let snap = LineSnap { on: true, anchors: [Some(middle), None], reach: 12.0 };
        let line = drag(Shape::Line, &[Vec2::new(395.0, 500.0), Vec2::new(398.0, 100.0)]);
        let mut overlay = Overlay::default();
        line.draw(&mut overlay, Marquee { snap, ..look() });
        let dots: Vec<_> = overlay.shapes.iter().filter(|s| matches!(s, Drawn::Dot { .. })).collect();
        let [Drawn::Dot { center, fill, .. }] = dots[..] else { panic!("one anchor: {dots:?}") };
        assert_eq!((*center, *fill), (middle, look().tint), "caught, so filled in");
        let Some(Drawn::Label(label)) = overlay.shapes.last() else { panic!("the angle is drawn last") };
        assert_eq!(label.runs[0].text, "90\u{b0}");

        let mut plain = Overlay::default();
        line.draw(&mut plain, look());
        assert!(!plain.shapes.iter().any(|s| matches!(s, Drawn::Dot { .. } | Drawn::Label(_))));
    }

    #[test]
    fn the_shaded_half_of_a_line_is_the_half_the_region_covers() {
        let line = drag(Shape::Line, &[Vec2::new(200.0, 500.0), Vec2::new(600.0, 100.0)]);
        let region = line.region(LineSnap::OFF).unwrap();
        for other_side in [false, true] {
            let mut overlay = Overlay::default();
            line.draw(&mut overlay, Marquee { other_side, ..look() });
            let Some(Drawn::Fill { points, .. }) = overlay.shapes.first() else { panic!("the half is drawn first") };
            let middle = points.iter().copied().sum::<Vec2>() / points.len() as f32;
            assert_eq!(region.contains(middle), !other_side);
            assert!(points.iter().all(|p| p.x >= -1e-3 && p.y >= -1e-3 && p.x <= 800.001 && p.y <= 600.001));
        }
    }

    #[test]
    fn a_lasso_is_outlined_and_closed_with_a_dashed_edge() {
        let loop_ = [Vec2::new(100.0, 100.0), Vec2::new(300.0, 100.0), Vec2::new(300.0, 300.0)];
        let mut overlay = Overlay::default();
        drag(Shape::Lasso, &loop_).draw(&mut overlay, look());
        assert!(!overlay.shapes.iter().any(|s| matches!(s, Drawn::Fill { .. })), "a lasso is never filled");
        let Drawn::Line { points, closed, dash, .. } = &overlay.shapes[0] else { panic!("the path") };
        assert_eq!((points.len(), *closed, *dash), (3, false, None));
        let Drawn::Line { points, dash, .. } = &overlay.shapes[1] else { panic!("the closing edge") };
        assert_eq!((points[0], points[1]), (loop_[2], loop_[0]));
        assert!(dash.is_some());

        let mut overlay = Overlay::default();
        drag(Shape::Box, &[Vec2::new(10.0, 10.0), Vec2::new(90.0, 60.0)]).draw(&mut overlay, look());
        assert!(matches!(overlay.shapes[0], Drawn::Fill { .. }) && matches!(overlay.shapes[1], Drawn::Line { closed: true, .. }));
    }
}
