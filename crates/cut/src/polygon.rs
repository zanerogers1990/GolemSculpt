//! Small 2D predicates the cap triangulators and the key tools share: areas,
//! containment, and distances between polygons and triangles in a flat chart.
//!
//! "Counter-clockwise" here is in the chart's own axes, x right and y up. Fed
//! screen pixels, where y runs down, the same sign means clockwise on screen;
//! nothing in this crate depends on which, because every caller compares
//! orientations with each other rather than with the screen.
//!
//! Orientation tests are evaluated in `f64`. The inputs are `f32`, so their
//! differences and products are held exactly there, and a turn is never
//! decided by rounding noise the way it can be in single precision.

use glam::Vec2;

/// Twice the signed area of triangle `abc`: positive when counter-clockwise.
#[inline]
pub fn cross(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    let (ax, ay) = (a.x as f64, a.y as f64);
    (b.x as f64 - ax) * (c.y as f64 - ay) - (b.y as f64 - ay) * (c.x as f64 - ax)
}

/// Signed area, positive for a counter-clockwise polygon. Summed in double: an
/// outline is a few hundred points far from the chart's origin, and float
/// cancellation there would decide orientation on noise.
pub fn signed_area(polygon: &[Vec2]) -> f64 {
    let n = polygon.len();
    let mut sum = 0.0;
    for i in 0..n {
        let (a, b) = (polygon[i], polygon[(i + 1) % n]);
        sum += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    sum * 0.5
}

/// Signed area of the polygon whose corners are `points[ring[..]]`.
pub fn signed_area_of(points: &[Vec2], ring: &[u32]) -> f64 {
    let n = ring.len();
    let mut sum = 0.0;
    for i in 0..n {
        let (a, b) = (points[ring[i] as usize], points[ring[(i + 1) % n] as usize]);
        sum += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    sum * 0.5
}

/// Even-odd containment, half-open in y so a corner lying exactly on the test
/// ray counts once rather than twice.
pub fn contains(polygon: &[Vec2], p: Vec2) -> bool {
    let n = polygon.len();
    if n == 0 {
        return false;
    }
    let mut inside = false;
    let mut a = polygon[n - 1];
    for &b in polygon {
        if (a.y <= p.y) != (b.y <= p.y) {
            let t = (p.y - a.y) / (b.y - a.y);
            if a.x + t * (b.x - a.x) > p.x {
                inside = !inside;
            }
        }
        a = b;
    }
    inside
}

/// Point in triangle, boundary included, for either winding.
#[inline]
pub fn in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

/// Barycentric weights of `p` in `abc`; `None` for a degenerate triangle.
pub fn barycentric(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> Option<[f32; 3]> {
    let det = cross(a, b, c);
    if det.abs() < 1e-20 {
        return None;
    }
    let wa = (cross(b, c, p) / det) as f32;
    let wb = (cross(c, a, p) / det) as f32;
    Some([wa, wb, 1.0 - wa - wb])
}

pub fn sqr_distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len = ab.length_squared();
    let t = if len > 1e-20 { ((p - a).dot(ab) / len).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t - p).length_squared()
}

/// Distance from `p` to a closed polygon's outline: zero on it, and the same
/// whether `p` is inside or outside (pair with `contains` for the sign).
pub fn distance_to_outline(polygon: &[Vec2], p: Vec2) -> f32 {
    let n = polygon.len();
    let mut best = f32::MAX;
    for i in 0..n {
        best = best.min(sqr_distance_to_segment(p, polygon[i], polygon[(i + 1) % n]));
    }
    best.sqrt()
}

/// Proper crossing of segments `ab` and `cd`: they share an interior point.
/// Touching at an end or running collinear does not count; callers decide
/// those cases themselves.
#[inline]
pub fn segments_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Shortest distance between a triangle and a closed polygon, zero when they
/// overlap. Exact for two shapes that do not, since the closest pair between
/// two sets of segments always includes an end of one of them.
pub fn triangle_to_polygon_distance(a: Vec2, b: Vec2, c: Vec2, polygon: &[Vec2]) -> f32 {
    let n = polygon.len();
    if n == 0 {
        return f32::MAX;
    }
    if contains(polygon, a) || contains(polygon, b) || contains(polygon, c) || in_triangle(polygon[0], a, b, c) {
        return 0.0;
    }
    let mut best = f32::MAX;
    for i in 0..n {
        let (p, q) = (polygon[(i + n - 1) % n], polygon[i]);
        if segments_cross(a, b, p, q) || segments_cross(b, c, p, q) || segments_cross(c, a, p, q) {
            return 0.0;
        }
        for corner in [a, b, c] {
            best = best.min(sqr_distance_to_segment(corner, p, q));
        }
        for (e0, e1) in [(a, b), (b, c), (c, a)] {
            best = best.min(sqr_distance_to_segment(q, e0, e1));
        }
    }
    best.sqrt()
}

/// Whether a closed polygon is simple: no two of its edges cross, touch or
/// run along each other, other than neighbours meeting at their shared corner.
/// A polygon that fails cannot be relied on to triangulate without overlaps.
///
/// Sorted along the polygon's longer side and swept, so an edge is only
/// compared with the edges it could reach: near enough linear for an outline
/// that goes once round its shape.
pub fn is_simple(polygon: &[Vec2]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let lo = polygon.iter().fold(Vec2::MAX, |m, p| m.min(*p));
    let hi = polygon.iter().fold(Vec2::MIN, |m, p| m.max(*p));
    let along = |p: Vec2| if hi.x - lo.x >= hi.y - lo.y { p.x } else { p.y };
    let ends = |i: usize| (polygon[i], polygon[(i + 1) % n]);
    let span = |i: usize| {
        let (a, b) = ends(i);
        (along(a).min(along(b)), along(a).max(along(b)))
    };
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_unstable_by(|&i, &j| span(i as usize).0.total_cmp(&span(j as usize).0));

    for (at, &i) in order.iter().enumerate() {
        let i = i as usize;
        let (a, b) = ends(i);
        if a == b {
            return false;
        }
        let reach = span(i).1;
        for &j in &order[at + 1..] {
            let j = j as usize;
            if span(j).0 > reach {
                break;
            }
            let (c, d) = ends(j);
            let clash = if (i + 1) % n == j {
                folds_back(a, b, d)
            } else if (j + 1) % n == i {
                folds_back(c, d, b)
            } else {
                segments_meet(a, b, c, d)
            };
            if clash {
                return false;
            }
        }
    }
    true
}

/// Whether the corner `a -> b -> c` doubles straight back on itself.
fn folds_back(a: Vec2, b: Vec2, c: Vec2) -> bool {
    cross(a, b, c) == 0.0 && (b - a).dot(c - b) <= 0.0
}

/// Whether segments `ab` and `cd` have any point in common.
fn segments_meet(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    let within = |p: Vec2, s: Vec2, e: Vec2| p.cmpge(s.min(e)).all() && p.cmple(s.max(e)).all();
    (d1 == 0.0 && within(a, c, d))
        || (d2 == 0.0 && within(b, c, d))
        || (d3 == 0.0 && within(c, a, b))
        || (d4 == 0.0 && within(d, a, b))
}

/// One number for the undirected edge between two vertices.
#[inline]
pub fn edge_key(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    ((lo as u64) << 32) | hi as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<Vec2> {
        vec![Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0)]
    }

    #[test]
    fn area_sign_follows_winding() {
        let mut sq = square();
        assert!((signed_area(&sq) - 4.0).abs() < 1e-9);
        assert!((signed_area_of(&sq, &[0, 1, 2]) - 2.0).abs() < 1e-9);
        sq.reverse();
        assert!((signed_area(&sq) + 4.0).abs() < 1e-9);
        assert!(cross(Vec2::ZERO, Vec2::X, Vec2::Y) > 0.0 && cross(Vec2::ZERO, Vec2::Y, Vec2::X) < 0.0);
    }

    #[test]
    fn area_keeps_its_sign_far_from_the_origin() {
        let far = Vec2::new(40_000.0, -25_000.0);
        let tiny: Vec<Vec2> = square().iter().map(|p| *p * 0.01 + far).collect();
        assert!(signed_area(&tiny) > 0.0);
    }

    #[test]
    fn containment_and_outline_distance() {
        let sq = square();
        assert!(contains(&sq, Vec2::new(1.0, 1.0)));
        assert!(!contains(&sq, Vec2::new(3.0, 1.0)));
        assert!(!contains(&sq, Vec2::new(1.0, -0.1)));
        assert!((distance_to_outline(&sq, Vec2::new(1.0, 1.0)) - 1.0).abs() < 1e-6);
        assert!((distance_to_outline(&sq, Vec2::new(3.0, 1.0)) - 1.0).abs() < 1e-6);
        assert!((distance_to_outline(&sq, Vec2::new(3.0, 3.0)) - 2f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn triangle_tests_take_either_winding() {
        let (a, b, c) = (Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(0.0, 4.0));
        for (p, want) in [(Vec2::new(1.0, 1.0), true), (Vec2::new(2.0, 2.0), true), (Vec2::new(3.0, 3.0), false)] {
            assert_eq!(in_triangle(p, a, b, c), want);
            assert_eq!(in_triangle(p, a, c, b), want);
        }
        let w = barycentric(Vec2::new(1.0, 2.0), a, b, c).unwrap();
        assert!((w[0] - 0.25).abs() < 1e-6 && (w[1] - 0.25).abs() < 1e-6 && (w[2] - 0.5).abs() < 1e-6);
        assert!(barycentric(Vec2::ONE, a, b, b).is_none());
    }

    #[test]
    fn only_proper_crossings_count() {
        let (a, b) = (Vec2::new(0.0, 0.0), Vec2::new(2.0, 2.0));
        assert!(segments_cross(a, b, Vec2::new(0.0, 2.0), Vec2::new(2.0, 0.0)));
        assert!(!segments_cross(a, b, Vec2::new(2.0, 2.0), Vec2::new(3.0, 0.0)));
        assert!(!segments_cross(a, b, Vec2::new(1.0, 1.0), Vec2::new(3.0, 3.0)));
        assert!(!segments_cross(a, b, Vec2::new(3.0, 0.0), Vec2::new(4.0, 1.0)));
    }

    #[test]
    fn triangle_to_polygon_distance_is_zero_on_overlap() {
        let sq = square();
        let tri = |dx: f32| [Vec2::new(dx, 0.5), Vec2::new(dx + 1.0, 0.5), Vec2::new(dx, 1.5)];
        let [a, b, c] = tri(3.0);
        assert!((triangle_to_polygon_distance(a, b, c, &sq) - 1.0).abs() < 1e-6);
        let [a, b, c] = tri(1.5);
        assert_eq!(triangle_to_polygon_distance(a, b, c, &sq), 0.0);
        // The polygon wholly inside the triangle.
        let big = [Vec2::new(-10.0, -10.0), Vec2::new(20.0, -10.0), Vec2::new(-10.0, 20.0)];
        assert_eq!(triangle_to_polygon_distance(big[0], big[1], big[2], &sq), 0.0);
        assert_eq!(edge_key(7, 3), edge_key(3, 7));
        assert_ne!(edge_key(7, 3), edge_key(3, 8));
    }

    #[test]
    fn simple_polygons_are_told_from_ones_that_cross_or_touch() {
        assert!(is_simple(&square()));
        let mut reversed = square();
        reversed.reverse();
        assert!(is_simple(&reversed));
        // An L, concave but simple; upright and on its side, so both sweep
        // directions are used.
        let ell =
            [(0.0, 0.0), (9.0, 0.0), (9.0, 1.0), (1.0, 1.0), (1.0, 4.0), (0.0, 4.0)].map(|(x, y)| Vec2::new(x, y));
        assert!(is_simple(&ell));
        assert!(is_simple(&ell.map(|p| Vec2::new(p.y, p.x))));
        // A bow tie crosses itself.
        let bow = [(0.0, 0.0), (4.0, 3.0), (4.0, 0.0), (0.0, 3.0)].map(|(x, y)| Vec2::new(x, y));
        assert!(!is_simple(&bow));
        assert!(!is_simple(&bow.map(|p| Vec2::new(p.y, p.x))));
        // A corner resting on a far edge, a spike folded flat, and a point
        // visited twice all count as touching.
        let resting = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (2.0, 0.0), (0.0, 4.0)].map(|(x, y)| Vec2::new(x, y));
        assert!(!is_simple(&resting));
        let spike = [(0.0, 0.0), (4.0, 0.0), (2.0, 0.0), (2.0, 3.0)].map(|(x, y)| Vec2::new(x, y));
        assert!(!is_simple(&spike));
        let doubled = [(0.0, 0.0), (4.0, 0.0), (4.0, 0.0), (2.0, 3.0)].map(|(x, y)| Vec2::new(x, y));
        assert!(!is_simple(&doubled));
        assert!(!is_simple(&square()[..2]));
        // A big wobbly ring, to be sure the sweep neither misses nor invents.
        let ring: Vec<Vec2> = (0..5000)
            .map(|i| {
                let a = i as f32 / 5000.0 * std::f32::consts::TAU;
                Vec2::new(a.cos() * 300.0, a.sin() * 80.0) * (1.0 + 0.2 * (a * 40.0).sin())
            })
            .collect();
        assert!(is_simple(&ring));
        let mut tangled = ring.clone();
        tangled.swap(1200, 1203);
        assert!(!is_simple(&tangled));
    }
}
