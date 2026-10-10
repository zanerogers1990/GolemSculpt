//! Single-triangle queries.

use glam::Vec3;

/// The point of triangle `abc` nearest to `p`.
pub fn closest_point(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Where a ray meets triangle `abc`, as a distance along `dir`; both faces
/// count. `dir` need not be unit length.
///
/// The arithmetic is done in `f64`, with a hair of slack at the edges. In `f32`
/// the rounding in a hit's position within its triangle is about one part in a
/// thousand for a dense mesh seen from a normal distance, which leaves gaps
/// along every edge: roughly 1 ray in 140 slipped between two triangles of a
/// million-triangle sphere and went on to "hit" its far side. In `f64` the error
/// is far below `EDGE_SLACK`, which also lets both neighbours accept a ray that
/// runs exactly along their shared edge, so one of them always reports it.
pub fn ray_hit(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    const EDGE_SLACK: f64 = 1e-9;
    let (origin, dir) = (origin.as_dvec3(), dir.as_dvec3());
    let a = a.as_dvec3();
    let (e1, e2) = (b.as_dvec3() - a, c.as_dvec3() - a);
    let h = dir.cross(e2);
    let det = e1.dot(h);
    if det.abs() <= 1e-40 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(h) * inv;
    if !(-EDGE_SLACK..=1.0 + EDGE_SLACK).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < -EDGE_SLACK || u + v > 1.0 + EDGE_SLACK {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t as f32)
}

/// Normal scaled by twice the area; zero for a degenerate triangle.
#[inline]
pub fn area_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    (b - a).cross(c - a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::TriMesh;
    use crate::testing::Rng;
    use glam::DVec3;
    use rayon::prelude::*;

    #[test]
    fn closest_point_covers_face_edge_and_corner() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X, Vec3::Y);
        let near = |p: Vec3, want: Vec3| closest_point(p, a, b, c).distance(want) < 1e-6;
        assert!(near(Vec3::new(0.2, 0.2, 3.0), Vec3::new(0.2, 0.2, 0.0)));
        assert!(near(Vec3::new(0.5, -2.0, 0.0), Vec3::new(0.5, 0.0, 0.0)));
        assert!(near(Vec3::new(-1.0, -1.0, 1.0), a));
        assert!(near(Vec3::new(2.0, 2.0, 0.0), Vec3::new(0.5, 0.5, 0.0)));
    }

    #[test]
    fn ray_hits_front_and_back_but_not_behind() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X, Vec3::Y);
        let p = Vec3::new(0.25, 0.25, 0.0);
        assert_eq!(ray_hit(p + Vec3::Z, -Vec3::Z, a, b, c), Some(1.0));
        assert_eq!(ray_hit(p - Vec3::Z, Vec3::Z, a, b, c), Some(1.0));
        assert_eq!(ray_hit(p + Vec3::Z, Vec3::Z, a, b, c), None);
        assert_eq!(ray_hit(Vec3::new(2.0, 2.0, 1.0), -Vec3::Z, a, b, c), None);
    }

    /// A latitude/longitude sphere of radius 1: `rings * segs * 2` triangles, closed, with both
    /// poles' vertices exactly on the axis. Cell (i, j) owns triangles `2 * (i * segs + j)` and
    /// the one after. Winding does not matter here: `ray_hit` counts both faces.
    fn lat_long_sphere(rings: usize, segs: usize) -> TriMesh {
        let mut positions = Vec::with_capacity((rings + 1) * segs);
        for i in 0..=rings {
            let polar = std::f64::consts::PI * i as f64 / rings as f64;
            for j in 0..segs {
                let azimuth = std::f64::consts::TAU * j as f64 / segs as f64;
                let ring = if i == 0 || i == rings { 0.0 } else { polar.sin() };
                positions.push(Vec3::new(
                    (ring * azimuth.cos()) as f32,
                    polar.cos() as f32,
                    (ring * azimuth.sin()) as f32,
                ));
            }
        }
        let vertex = |i: usize, j: usize| (i * segs + j % segs) as u32;
        let mut indices = Vec::with_capacity(rings * segs * 6);
        for i in 0..rings {
            for j in 0..segs {
                let (a, b, c, d) = (vertex(i, j), vertex(i, j + 1), vertex(i + 1, j + 1), vertex(i + 1, j));
                indices.extend_from_slice(&[a, d, b, b, d, c]);
            }
        }
        TriMesh::new(positions, indices)
    }

    /// The rays, as indices into `directions`, that found nothing near where they enter. Each
    /// starts `distance` unit-sphere radii out and aims at the centre; `dir` is deliberately not
    /// unit length. Only the cells around where the ray enters are tested, which keeps a
    /// million-triangle sphere cheap: a ray can only hit there.
    fn rays_that_miss(sphere: &TriMesh, rings: usize, segs: usize, directions: &[DVec3], distance: f64) -> Vec<usize> {
        directions
            .par_iter()
            .enumerate()
            .filter(|(_, d)| {
                let origin = (**d * distance).as_vec3();
                let dir = -origin;
                let polar = d.y.acos() / std::f64::consts::PI;
                let azimuth = d.z.atan2(d.x).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
                let ring = ((polar * rings as f64) as usize).min(rings - 1);
                let seg = ((azimuth * segs as f64) as usize).min(segs - 1);
                let mut nearest = f32::INFINITY;
                for i in ring.saturating_sub(2)..=(ring + 2).min(rings - 1) {
                    for j in seg + segs - 3..=seg + segs + 3 {
                        let cell = i * segs + j % segs;
                        for t in 2 * cell..2 * cell + 2 {
                            let [a, b, c] = sphere.triangle(t);
                            if let Some(hit) = ray_hit(origin, dir, a, b, c) {
                                nearest = nearest.min(hit);
                            }
                        }
                    }
                }
                // The surface is a hair inside the unit sphere, so the hit is at (distance - 1) / distance.
                (nearest - ((distance - 1.0) / distance) as f32).abs() >= 1e-3
            })
            .map(|(k, _)| k)
            .collect()
    }

    /// An f32 test let rays slip between two triangles of a million-triangle sphere and report
    /// the far side, or nothing (the app's BVH saw about 1 in 140). Fired at the centre from
    /// outside, every ray must hit a triangle where it enters. The error grows with the ray's
    /// length: the f32 version held on this sphere from three radii away but lost 53 of 100,000
    /// rays from a hundred (nearly parallel rays, as from a far camera), which is what the
    /// second distance is for.
    #[test]
    fn no_ray_slips_between_the_triangles_of_a_dense_sphere() {
        let (rings, segs) = (500, 1000);
        let sphere = lat_long_sphere(rings, segs);
        assert!(sphere.triangle_count() >= 1_000_000);

        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let directions: Vec<DVec3> = (0..100_000)
            .map(|_| {
                let (y, azimuth) = (rng.next() * 2.0 - 1.0, rng.next() * std::f64::consts::TAU);
                let r = (1.0 - y * y).sqrt();
                DVec3::new(r * azimuth.cos(), y, r * azimuth.sin())
            })
            .collect();

        for distance in [3.0, 100.0] {
            let misses = rays_that_miss(&sphere, rings, segs, &directions, distance);
            assert!(
                misses.is_empty(),
                "from {distance} radii, {} of {} rays missed, first {:?}",
                misses.len(),
                directions.len(),
                &misses[..misses.len().min(5)]
            );
        }
    }

    /// A ray along the edge two triangles share lies on both: at least one must report it.
    #[test]
    fn a_ray_along_a_shared_edge_hits_one_of_the_two_triangles() {
        let quad = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
        let hits = |origin: Vec3| {
            let down = -Vec3::Z;
            [ray_hit(origin, down, quad[0], quad[1], quad[2]), ray_hit(origin, down, quad[0], quad[2], quad[3])]
        };
        for k in 1..100 {
            let s = k as f32 / 100.0;
            let [first, second] = hits(Vec3::new(s, s, 1.0));
            assert!(first.is_some() || second.is_some(), "missed the diagonal at {s}");
        }

        // Skewed triangles, and points that are not exactly on the edge once rounded to f32: the
        // ray passes a hair to one side or the other, and one of the two triangles must have it.
        // The second triangle's far corner is the first's reflected through the edge midpoint (so
        // it lies on the other side as seen along the ray), lifted a little out of the plane.
        let mut rng = Rng(12345);
        let mut point = move || Vec3::new(rng.next() as f32, rng.next() as f32, (rng.next() * 0.3) as f32);
        for _ in 0..4000 {
            let (a, b, c) = (point(), point(), point());
            let normal = (b - a).cross(c - a).normalize_or_zero();
            if normal == Vec3::ZERO {
                continue;
            }
            let d = a + b - c + normal * 0.05;
            let s = point().x;
            let origin = a + (b - a) * s + normal * 0.7;
            let (first, second) = (ray_hit(origin, -normal, a, b, c), ray_hit(origin, -normal, b, a, d));
            assert!(first.is_some() || second.is_some(), "ray slipped through the edge {a} - {b} at {s}");
        }
    }

    /// Every triangle of a fan has the centre vertex as a corner, so a ray through it hits.
    #[test]
    fn a_ray_through_a_shared_vertex_hits() {
        let rim: Vec<Vec3> = (0..6)
            .map(|k| {
                let angle = std::f32::consts::TAU * k as f32 / 6.0;
                Vec3::new(angle.cos(), angle.sin(), 0.0)
            })
            .collect();
        let hit = (0..6).filter_map(|k| ray_hit(Vec3::Z, -Vec3::Z, Vec3::ZERO, rim[k], rim[(k + 1) % 6])).count();
        assert!(hit >= 1);
    }
}
