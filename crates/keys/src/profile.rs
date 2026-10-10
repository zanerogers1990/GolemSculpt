//! A key's cross-section: a rounded polygon that can be offset exactly.
//!
//! Every preset is a closed outline built from CORNER CIRCLES joined by their
//! common tangents, including the circle (a single corner) and the shapes
//! with inside corners (a concave corner is a circle on the outside of the
//! outline, stored with a negative radius).
//!
//! WHY THIS REPRESENTATION. The same shape is needed at several OFFSETS: the
//! peg at its nominal size, the hole grown by the print clearance, the peg's
//! tip shrunk by its chamfer, every ring in between shrunk by the taper. The
//! fit depends on those being true parallel offsets of one another.
//! Offsetting a rounded polygon is exact and trivial: every corner keeps its
//! centre and its signed radius grows by the offset. The tangent DIRECTIONS
//! between corners do not change with the offset at all (they depend only on
//! the difference of two radii, which the offset cancels), so every offset
//! outline has the same arcs sweeping the same angles and the same straight
//! runs of the same length. Sampled with fixed segment counts, every offset
//! therefore has the same number of points and point i of one is directly
//! opposite point i of any other. That is what lets a peg's wall be built as
//! plain quad strips between rings, and what makes the hole wall exactly
//! `clearance` away from the peg wall everywhere, corners included. A general
//! polygon offset gives neither.
//!
//! The one constraint is that no radius may change sign across the offsets
//! used: a convex corner shrunk past zero, or a concave one grown past zero,
//! would turn inside out. `KeyProfile::create` takes the offset range up
//! front and sizes the corners so that cannot happen, which also happens to
//! be what a printer wants, since no printed corner is sharper than its
//! nozzle.

use std::f32::consts::{PI, TAU};

use glam::Vec2;
use sculpt_cut::polygon;

/// The cross-section presets a key can have. Everything but `Circle` is
/// keyed (it only goes in one way round), so a single one of those is enough
/// to stop the parts twisting; a lone circle is not, which is why automatic
/// placement puts two circles where it can.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyShape {
    #[default]
    Circle,
    Square,
    Rectangle,
    Hexagon,
    Triangle,
    DShape,
    Cross,
    Star,
    Dovetail,
    PuzzleTab,
}

impl KeyShape {
    pub const ALL: [KeyShape; 10] = [
        KeyShape::Circle,
        KeyShape::Square,
        KeyShape::Rectangle,
        KeyShape::Hexagon,
        KeyShape::Triangle,
        KeyShape::DShape,
        KeyShape::Cross,
        KeyShape::Star,
        KeyShape::Dovetail,
        KeyShape::PuzzleTab,
    ];

    /// As the panel names it.
    pub fn name(self) -> &'static str {
        match self {
            KeyShape::Circle => "Circle",
            KeyShape::Square => "Square",
            KeyShape::Rectangle => "Rectangle",
            KeyShape::Hexagon => "Hexagon",
            KeyShape::Triangle => "Triangle",
            KeyShape::DShape => "D-Shape",
            KeyShape::Cross => "Cross",
            KeyShape::Star => "Star",
            KeyShape::Dovetail => "Dovetail",
            KeyShape::PuzzleTab => "Puzzle Tab",
        }
    }
}

/// A key's cross-section; see the module remarks.
#[derive(Clone, Debug)]
pub struct KeyProfile {
    centre: Vec<Vec2>,
    /// Signed: positive convex, negative concave.
    radius: Vec<f32>,
    /// Outward normal of the edge arriving at corner i.
    normal_in: Vec<Vec2>,
    /// Signed angle the outline turns through at corner i.
    sweep: Vec<f32>,
    arc_segments: Vec<usize>,
    /// Per edge i -> i+1; 0 means the two corners touch.
    edge_segments: Vec<usize>,
    single: bool,
    shape: KeyShape,
    size: f32,
    point_count: usize,
    min_offset: f32,
    max_offset: f32,
    problem: Option<&'static str>,
}

impl KeyProfile {
    fn new(
        shape: KeyShape,
        size: f32,
        centre: Vec<Vec2>,
        radius: Vec<f32>,
        min_offset: f32,
        max_offset: f32,
        chord_tolerance: f32,
        max_edge: f32,
    ) -> KeyProfile {
        let n = centre.len();
        let mut profile = KeyProfile {
            normal_in: vec![Vec2::ZERO; n],
            sweep: vec![0.0; n],
            arc_segments: vec![0; n],
            edge_segments: vec![0; n],
            single: n == 1,
            shape,
            size,
            point_count: 0,
            min_offset,
            max_offset,
            problem: None,
            centre,
            radius,
        };
        let (centre, radius) = (&profile.centre, &profile.radius);

        let flips = |r: f32| if r > 0.0 { r + min_offset <= 0.0 } else { r + max_offset >= 0.0 };
        if radius.iter().any(|&r| flips(r)) {
            profile.problem = Some("a corner is too sharp for the offsets asked of it");
        }

        if profile.single {
            profile.normal_in[0] = Vec2::new(0.0, -1.0);
            profile.sweep[0] = TAU;
            profile.arc_segments[0] = arc_segments(TAU, radius[0].abs() + max_offset.max(0.0), chord_tolerance, 12);
            profile.point_count = profile.arc_segments[0];
            return profile;
        }

        // The tangent from each corner to the next: its outward normal and its
        // length. Both are offset-invariant (see the module remarks), so they
        // are worked out once here.
        let tangents: Vec<(Vec2, f32)> =
            (0..n).map(|i| tangent(centre[i], radius[i], centre[(i + 1) % n], radius[(i + 1) % n])).collect();

        for i in 0..n {
            let prev = (i + n - 1) % n;
            profile.normal_in[i] = tangents[prev].0;

            // The outline turns LEFT round a convex corner and right round a
            // concave one, and a corner can turn through more than half a turn
            // (the big arc of a D, the head of a puzzle tab), so the sign is
            // decided by the corner and only the angle by atan2.
            let mut turn = signed_angle(tangents[prev].0, tangents[i].0);
            if radius[i] > 0.0 && turn < 0.0 {
                turn += TAU;
            }
            if radius[i] < 0.0 && turn > 0.0 {
                turn -= TAU;
            }
            profile.sweep[i] = turn;

            // Sampled for the LARGEST radius this corner reaches over the
            // offset range: the count has to be the same at every offset, and
            // a count fine enough for the largest arc is fine enough for the
            // rest.
            let largest =
                if radius[i] > 0.0 { radius[i] + max_offset.max(0.0) } else { -radius[i] - min_offset.min(0.0) };
            profile.arc_segments[i] =
                if turn.abs() < 1e-4 { 0 } else { arc_segments(turn.abs(), largest, chord_tolerance, 1) };

            // A tangent shorter than a hair is two corners that touch (the
            // neck of a puzzle tab meeting its head): no straight run between
            // them, and no duplicate point.
            let length = tangents[i].1;
            profile.edge_segments[i] = if length <= 1e-5 * size.max(1e-3) {
                0
            } else {
                ((length / max_edge.max(1e-6)).ceil() as usize).max(1)
            };
        }

        let (mut points, mut total_turn) = (0, 0.0);
        for i in 0..n {
            let prev = (i + n - 1) % n;
            // The arc's own points, less its first when the edge arriving
            // here has zero length (that point is the previous corner's last
            // one).
            points += profile.arc_segments[i] + usize::from(profile.edge_segments[prev] != 0);
            // Interior points of the straight run to the next corner.
            points += profile.edge_segments[i].saturating_sub(1);
            total_turn += profile.sweep[i];
        }
        profile.point_count = points;

        // A simple counter-clockwise outline turns through exactly one full
        // turn. Corners rounded bigger than the edges between them take room
        // they do not have, and the tangents then wrap round an extra time:
        // this is where that shows.
        if (total_turn - TAU).abs() > 0.01 && profile.problem.is_none() {
            profile.problem = Some("corners are too round for this shape");
        }
        profile
    }

    /// Builds a preset. `size` is the key's nominal width: the diameter of a
    /// circle or D, the side of a square, the SHORT side of a rectangle (the
    /// long one is `size * aspect`), the flat-to-flat width of a hexagon, the
    /// tip-to-tip span of a cross or star, and the overall length of a
    /// dovetail or puzzle tab. `corner_radius` rounds every corner that has
    /// one.
    ///
    /// `min_offset..max_offset` is the range of offsets this profile will be
    /// asked for (the most a peg tip shrinks, the most a hole mouth grows);
    /// corners are kept rounder than that so none of them turns inside out.
    /// `chord_tolerance` is the most an arc may deviate from its true circle,
    /// and `max_edge` the longest a straight run may go unsplit: the density
    /// of the surface the key is sewn into, so the wall is not a fan of
    /// needles.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        shape: KeyShape,
        size: f32,
        aspect: f32,
        corner_radius: f32,
        min_offset: f32,
        max_offset: f32,
        chord_tolerance: f32,
        max_edge: f32,
    ) -> KeyProfile {
        let s = size.max(1e-4);
        let min_offset = min_offset.min(0.0);
        let max_offset = max_offset.max(0.0);

        // What every corner must clear: a convex one shrinks by up to
        // -min_offset, a concave one by up to max_offset. The margin keeps the
        // smallest arc a real arc rather than a point.
        let convex_floor = -min_offset + s * 0.02;
        let concave_floor = max_offset + s * 0.02;
        // The shape's own ceiling on rounding comes first (past it a square is
        // a circle), the offset floor last: a corner that turned inside out at
        // some offset would be a broken key, where one rounder than the shape
        // likes is only a softer one. If the floor is so high the shape cannot
        // take it at all, the outline comes out invalid and says so.
        let convex = |cap: f32| corner_radius.min(cap).max(convex_floor);
        let concave = |cap: f32| corner_radius.min(cap).max(concave_floor);

        let (centres, radii) = match shape {
            KeyShape::Circle => (vec![Vec2::ZERO], vec![s * 0.5]),
            KeyShape::Square => from_polygon(&rect(s, s), convex(s * 0.45), concave(s)),
            KeyShape::Rectangle => from_polygon(&rect(s * aspect.max(1.0), s), convex(s * 0.45), concave(s)),
            KeyShape::Hexagon => {
                let circumradius = s * 0.5 / (PI / 6.0).cos();
                from_polygon(&regular(6, circumradius, 0.0), convex(s * 0.4), concave(s))
            }
            KeyShape::Triangle => from_polygon(&regular(3, s * 0.6, PI * 0.5), convex(s * 0.2), concave(s)),
            KeyShape::DShape => d_shape(s, convex(s * 0.2)),
            KeyShape::Cross => {
                let arm = s * 0.38;
                from_polygon(&cross(s, arm), convex(arm * 0.45), concave(arm * 0.8))
            }
            KeyShape::Star => from_polygon(&star(5, s * 0.5, s * 0.26), convex(s * 0.08), concave(s * 0.12)),
            // Narrow at the root (-y), wide at the far end: the classic tail
            // seen end-on.
            KeyShape::Dovetail => from_polygon(
                &[
                    Vec2::new(-0.3 * s, -0.4 * s),
                    Vec2::new(0.3 * s, -0.4 * s),
                    Vec2::new(0.5 * s, 0.4 * s),
                    Vec2::new(-0.5 * s, 0.4 * s),
                ],
                convex(s * 0.15),
                concave(s),
            ),
            KeyShape::PuzzleTab => puzzle_tab(s, convex(s * 0.12), concave(s * 0.15)),
        };

        let mut profile =
            KeyProfile::new(shape, s, centres, radii, min_offset, max_offset, chord_tolerance.max(1e-6), max_edge);
        if profile.is_valid() && (!is_simple(&profile.outline(min_offset)) || !is_simple(&profile.outline(max_offset))) {
            profile.problem = Some("the outline folds over itself at this size");
        }
        profile
    }

    pub fn shape(&self) -> KeyShape {
        self.shape
    }

    /// Nominal size the profile was built at; see `create` for what it
    /// measures per shape.
    pub fn size(&self) -> f32 {
        self.size
    }

    /// Points in every outline this profile produces, at any offset.
    pub fn point_count(&self) -> usize {
        self.point_count
    }

    /// The offset range the corners were sized for. `outline` clamps to it.
    pub fn min_offset(&self) -> f32 {
        self.min_offset
    }

    pub fn max_offset(&self) -> f32 {
        self.max_offset
    }

    /// Only a circle can turn inside its own hole.
    pub fn is_rotationally_symmetric(&self) -> bool {
        self.shape == KeyShape::Circle
    }

    /// False when the outline cannot be drawn at every offset in its range: a
    /// corner rounded past the shape's own geometry (a star's tips asked to
    /// take a chamfer wider than the tips), or a corner radius that changes
    /// sign inside the range. `problem` says which.
    pub fn is_valid(&self) -> bool {
        self.problem.is_none()
    }

    pub fn problem(&self) -> Option<&'static str> {
        self.problem
    }

    /// The outline grown by `offset` (negative shrinks it), counter-clockwise,
    /// `point_count` points. Offsets outside the range the profile was built
    /// for are clamped.
    pub fn outline(&self, offset: f32) -> Vec<Vec2> {
        let mut points = vec![Vec2::ZERO; self.point_count];
        self.outline_into(offset, &mut points);
        points
    }

    /// As `outline`, into a buffer of `point_count` points.
    pub fn outline_into(&self, offset: f32, into: &mut [Vec2]) {
        let offset = offset.clamp(self.min_offset, self.max_offset);
        let at = |centre: Vec2, angle: f32, radius: f32| centre + Vec2::from_angle(angle) * radius;
        if self.single {
            let segments = self.arc_segments[0];
            for (k, point) in into.iter_mut().take(segments).enumerate() {
                *point = at(self.centre[0], -0.5 * PI + TAU * k as f32 / segments as f32, self.radius[0] + offset);
            }
            return;
        }

        let n = self.centre.len();
        let mut w = 0;
        for i in 0..n {
            let prev = (i + n - 1) % n;
            let s = self.radius[i] + offset;
            let start = self.normal_in[i].y.atan2(self.normal_in[i].x);
            let segments = self.arc_segments[i];
            let first = usize::from(self.edge_segments[prev] == 0);
            for k in first..=segments {
                let a = start + if segments == 0 { 0.0 } else { self.sweep[i] * k as f32 / segments as f32 };
                into[w] = at(self.centre[i], a, s);
                w += 1;
            }

            // Straight run to the next corner: from this arc's last point to
            // the next arc's first, split into equal pieces.
            let edge = self.edge_segments[i];
            if edge > 1 {
                let j = (i + 1) % n;
                let from = at(self.centre[i], start + self.sweep[i], s);
                let next_start = self.normal_in[j].y.atan2(self.normal_in[j].x);
                let to = at(self.centre[j], next_start, self.radius[j] + offset);
                for k in 1..edge {
                    into[w] = from.lerp(to, k as f32 / edge as f32);
                    w += 1;
                }
            }
        }
    }

    /// Largest distance from the profile's origin to its outline at `offset`.
    pub fn circumradius(&self, offset: f32) -> f32 {
        self.outline(offset).iter().map(|p| p.length()).fold(0.0, f32::max)
    }

    /// Enclosed area at `offset` (shoelace over the sampled outline).
    pub fn area(&self, offset: f32) -> f32 {
        polygon::signed_area(&self.outline(offset)).abs() as f32
    }
}

/// No two edges of the closed outline cross, and no edge has zero length.
/// Corners that merely touch are let through: two arcs of one outline meet
/// tangentially by design (see `d_shape` and `puzzle_tab`).
pub fn is_simple(p: &[Vec2]) -> bool {
    let n = p.len();
    if n < 3 {
        return false;
    }
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        if a.distance_squared(b) <= 1e-16 {
            return false;
        }
        let (lo, hi) = (a.min(b), a.max(b));
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let (c, d) = (p[j], p[(j + 1) % n]);
            if c.max(d).cmplt(lo).any() || c.min(d).cmpgt(hi).any() {
                continue;
            }
            if polygon::segments_cross(a, b, c, d) {
                return false;
            }
        }
    }
    true
}

fn rect(w: f32, h: f32) -> [Vec2; 4] {
    let (x, y) = (w * 0.5, h * 0.5);
    [Vec2::new(-x, -y), Vec2::new(x, -y), Vec2::new(x, y), Vec2::new(-x, y)]
}

fn regular(sides: usize, circumradius: f32, phase: f32) -> Vec<Vec2> {
    (0..sides).map(|i| Vec2::from_angle(phase + TAU * i as f32 / sides as f32) * circumradius).collect()
}

fn star(points: usize, outer: f32, inner: f32) -> Vec<Vec2> {
    (0..points * 2)
        .map(|i| Vec2::from_angle(PI * 0.5 + PI * i as f32 / points as f32) * if i % 2 == 0 { outer } else { inner })
        .collect()
}

fn cross(span: f32, arm: f32) -> [Vec2; 12] {
    let (h, a) = (span * 0.5, arm * 0.5);
    [(a, -h), (a, -a), (h, -a), (h, a), (a, a), (a, h), (-a, h), (-a, a), (-h, a), (-h, -a), (-a, -a), (-a, -h)]
        .map(|(x, y)| Vec2::new(x, y))
}

/// A circle with a flat: the big arc is one convex corner centred on the
/// circle, and the two ends of the flat are small corners sitting INSIDE it,
/// tangent to both the flat and the circle, so the arc runs straight into
/// them with no straight run between.
fn d_shape(s: f32, rc: f32) -> (Vec<Vec2>, Vec<f32>) {
    let r = s * 0.5;
    let flat = -r + s * 0.2;
    let cy = flat + rc;
    let cx = ((r - rc) * (r - rc) - cy * cy).max(0.0).sqrt();
    (vec![Vec2::ZERO, Vec2::new(-cx, cy), Vec2::new(cx, cy)], vec![r, rc, rc])
}

/// A jigsaw tab: a neck rising from the root (-y) into a round head, with
/// concave fillets where they meet. Each fillet is tangent to the neck's side
/// and to the head at once, so it touches the head directly.
fn puzzle_tab(s: f32, rc: f32, rf: f32) -> (Vec<Vec2>, Vec<f32>) {
    let half_neck = s * 0.18;
    let head = s * 0.32;
    let bottom = -s * 0.5;
    let head_y = s * 0.5 - head;
    let reach = head + rf;
    let fx = half_neck + rf;
    let fy = head_y - (reach * reach - fx * fx).max(0.0).sqrt();
    let centres = vec![
        Vec2::new(-half_neck + rc, bottom + rc),
        Vec2::new(half_neck - rc, bottom + rc),
        Vec2::new(fx, fy),
        Vec2::new(0.0, head_y),
        Vec2::new(-fx, fy),
    ];
    (centres, vec![rc, rc, -rf, head, -rf])
}

/// Rounds every corner of a counter-clockwise polygon: each corner's circle
/// is placed tangent to both edges meeting there, inside the polygon for a
/// convex corner, outside it (negative radius) for a concave one. The rounded
/// outline's straight runs therefore lie on the original edges.
fn from_polygon(polygon: &[Vec2], convex_radius: f32, concave_radius: f32) -> (Vec<Vec2>, Vec<f32>) {
    let n = polygon.len();
    let (mut centres, mut radii) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        let (prev, p, next) = (polygon[(i + n - 1) % n], polygon[i], polygon[(i + 1) % n]);
        let (e_in, e_out) = ((p - prev).normalize(), (next - p).normalize());
        // Inward normals of the two edges.
        let (n_in, n_out) = (Vec2::new(-e_in.y, e_in.x), Vec2::new(-e_out.y, e_out.x));
        let convex = e_in.perp_dot(e_out) > 0.0;
        let r = if convex { convex_radius } else { -concave_radius };

        // The point whose distance from both edge lines is r, on the side r's
        // sign says.
        let denominator = 1.0 + n_in.dot(n_out);
        let offset = if denominator > 1e-6 { (n_in + n_out) * (r / denominator) } else { Vec2::ZERO };
        centres.push(p + offset);
        radii.push(r);
    }
    (centres, radii)
}

/// The common tangent leaving corner a and arriving at corner b with the
/// outline's interior on its left: its outward (right-hand) unit normal, and
/// its length. With signed radii one formula covers convex-convex (an outer
/// tangent) and convex-concave (a crossing one): the normal m must satisfy
/// `m . (cb - ca) = ra - rb`.
fn tangent(ca: Vec2, ra: f32, cb: Vec2, rb: f32) -> (Vec2, f32) {
    let d = cb - ca;
    let l = d.length();
    if l < 1e-9 {
        return (Vec2::new(0.0, -1.0), 0.0);
    }
    let dir = d / l;
    let right = Vec2::new(dir.y, -dir.x);
    let alpha = ((ra - rb) / l).clamp(-1.0, 1.0);
    let beta = (1.0 - alpha * alpha).max(0.0).sqrt();
    (dir * alpha + right * beta, l * beta)
}

fn signed_angle(from: Vec2, to: Vec2) -> f32 {
    from.perp_dot(to).atan2(from.dot(to))
}

/// Segments for an arc of `sweep` radians at `radius` so no chord strays more
/// than `tolerance` from the circle, between 4 and 12 degrees a segment.
fn arc_segments(sweep: f32, radius: f32, tolerance: f32, minimum: usize) -> usize {
    let step = if radius > tolerance { 2.0 * (1.0 - tolerance / radius).clamp(-1.0, 1.0).acos() } else { PI };
    let step = step.clamp(4f32.to_radians(), 12f32.to_radians());
    ((sweep / step).ceil() as usize).max(minimum)
}

#[cfg(test)]
mod tests {
    use sculpt_geom::units::from_millimetres as mm;

    use super::*;
    use crate::builder;
    use crate::types::KeyDimensions;

    /// The Unity project's `SplitKeyTests.EveryPresetOffsetsAsATrueParallelOffset`.
    #[test]
    fn every_preset_offsets_as_a_true_parallel_offset() {
        let size = mm(6.0);
        for shape in KeyShape::ALL {
            let mut d = KeyDimensions::from_millimetres(5.0, 0.2, 0.4, 0.5, 0.3, 1.0);
            let (p, _) = builder::fit_profile(shape, size, 1.6, mm(0.6), &mut d, size / 20.0);
            assert!(p.is_valid(), "{shape:?}: {:?}", p.problem());
            let (nominal, grown) = (p.outline(0.0), p.outline(p.max_offset()));
            assert_eq!(nominal.len(), grown.len(), "{shape:?}: point counts differ between offsets");
            assert_eq!(nominal.len(), p.point_count());
            assert!(polygon::signed_area(&nominal) > 0.0, "{shape:?}: not counter-clockwise");
            for q in &grown {
                let off = polygon::distance_to_outline(&nominal, *q);
                let strayed = (off - p.max_offset()).abs();
                assert!(strayed < mm(0.02), "{shape:?}: the grown outline is {off} away, not a parallel offset");
            }
        }
    }

    #[test]
    fn point_i_of_one_offset_is_opposite_point_i_of_another() {
        let size = mm(8.0);
        for shape in KeyShape::ALL {
            let p = KeyProfile::create(shape, size, 1.6, mm(0.6), -mm(0.4), mm(0.5), mm(0.01), size / 10.0);
            assert!(p.is_valid(), "{shape:?}: {:?}", p.problem());
            let (inner, outer) = (p.outline(-mm(0.3)), p.outline(mm(0.45)));
            for (a, b) in inner.iter().zip(&outer) {
                assert!((a.distance(*b) - mm(0.75)).abs() < mm(0.002), "{shape:?}: {} apart", a.distance(*b));
            }
            // Offsets past the range the corners were sized for are clamped.
            assert_eq!(p.outline(mm(5.0)), p.outline(p.max_offset()));
        }
    }

    #[test]
    fn sizes_measure_what_the_panel_says_they_do() {
        let p = |shape| KeyProfile::create(shape, 1.0, 2.0, 0.05, -0.01, 0.01, 1e-4, 0.1);
        let extent = |profile: &KeyProfile| {
            let o = profile.outline(0.0);
            let lo = o.iter().fold(Vec2::MAX, |m, q| m.min(*q));
            let hi = o.iter().fold(Vec2::MIN, |m, q| m.max(*q));
            hi - lo
        };
        let near = |a: f32, b: f32| (a - b).abs() < 2e-3;
        let circle = p(KeyShape::Circle);
        assert!(near(extent(&circle).x, 1.0) && near(circle.circumradius(0.0), 0.5));
        assert!((circle.area(0.0) - PI * 0.25).abs() < 0.01);
        assert!(circle.is_rotationally_symmetric() && !p(KeyShape::Square).is_rotationally_symmetric());
        let square = extent(&p(KeyShape::Square));
        assert!(near(square.x, 1.0) && near(square.y, 1.0));
        let rectangle = extent(&p(KeyShape::Rectangle));
        assert!(near(rectangle.x, 2.0) && near(rectangle.y, 1.0), "the short side is the size");
        assert!(near(extent(&p(KeyShape::Hexagon)).y, 1.0), "a hexagon is measured flat to flat");
        assert!(near(extent(&p(KeyShape::Cross)).x, 1.0) && near(extent(&p(KeyShape::DShape)).x, 1.0));
        assert!(near(extent(&p(KeyShape::PuzzleTab)).y, 1.0), "a puzzle tab is measured along its length");
    }

    #[test]
    fn a_corner_asked_to_shrink_past_nothing_is_reported() {
        // A star's tips cannot take a chamfer wider than the tips.
        let p = KeyProfile::create(KeyShape::Star, mm(3.0), 1.0, mm(0.6), -mm(1.0), mm(0.5), mm(0.01), mm(0.3));
        assert!(!p.is_valid());
        assert!(p.problem().is_some());
        assert!(!is_simple(&[Vec2::ZERO, Vec2::new(2.0, 2.0), Vec2::new(2.0, 0.0), Vec2::new(0.0, 2.0)]));
        assert!(!is_simple(&[Vec2::ZERO, Vec2::X]));
    }
}
