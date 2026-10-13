//! The triangle of a mesh nearest a point in space.
//!
//! This is what carries per-face data (face groups) from a mesh onto another
//! mesh of the same shape and different triangles: each new triangle asks
//! which old face lies nearest its centre (`mesh/groups/transfer.rs`).
//!
//! The walk is the usual one: nearer box first, and a box no closer than the
//! best triangle found so far is not opened. A caller with many points that
//! lie close together (the triangles of a mesh, taken in order) hands in the
//! distance to the last answer as `within`; the walk then opens a leaf or two
//! instead of feeling its way down from nothing, which is most of the cost.

use glam::Vec3;

use super::Bvh;

/// The point of triangle `a b c` nearest `p` (Ericson, Real-Time Collision
/// Detection 5.1.5: which of the corner, edge and face regions `p` is over).
pub fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
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
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let sum = va + vb + vc;
    // A triangle with no area has no inside to be over.
    if sum.abs() < 1e-30 {
        return a;
    }
    a + ab * (vb / sum) + ac * (vc / sum)
}

impl Bvh {
    /// The triangle nearest `p` and the square of its distance, among those
    /// `skip` does not name and no further than `within` (a square too;
    /// infinity for no limit). `None` if there is none.
    pub fn nearest_triangle(
        &self,
        positions: &[Vec3],
        indices: &[u32],
        p: Vec3,
        within: f32,
        skip: impl Fn(u32) -> bool,
    ) -> Option<(u32, f32)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = None;
        let mut reach = within;
        // One entry a level at most stays behind: the same room as the ray walk.
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if node.bounds.dist_sq(p) > reach {
                continue;
            }
            if let Some(li) = node.leaf() {
                let leaf = &self.leaves[li as usize];
                for tri in leaf.tri_start..leaf.tri_end {
                    if skip(tri) {
                        continue;
                    }
                    let i = tri as usize * 3;
                    let [a, b, c] = [0, 1, 2].map(|k| positions[indices[i + k] as usize]);
                    let d = closest_on_triangle(p, a, b, c).distance_squared(p);
                    if d <= reach {
                        reach = d;
                        best = Some((tri, d));
                    }
                }
                continue;
            }
            // The nearer child is looked at first, so it goes on last.
            let (near, far) = {
                let (da, db) =
                    (self.nodes[node.a as usize].bounds.dist_sq(p), self.nodes[node.b as usize].bounds.dist_sq(p));
                if da <= db { (node.a, node.b) } else { (node.b, node.a) }
            };
            if sp + 2 <= stack.len() {
                stack[sp] = far;
                stack[sp + 1] = near;
                sp += 2;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    #[test]
    fn the_nearest_triangle_is_the_one_a_search_of_every_triangle_finds() {
        let mesh = primitives::build(Primitive::Sphere, 6_000, 0.5);
        let every = |p: Vec3| {
            (0..mesh.triangle_count() as u32)
                .map(|t| {
                    let i = t as usize * 3;
                    let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[mesh.indices[i + k] as usize]);
                    (t, closest_on_triangle(p, a, b, c).distance_squared(p))
                })
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .unwrap()
        };
        for i in 0..400 {
            let a = i as f32 * 0.731;
            // Inside, on and well outside the surface.
            let p = Vec3::new(a.cos(), (a * 1.7).sin(), (a * 0.3).cos()).normalize() * (0.1 + (i % 9) as f32 * 0.12);
            let (_, want) = every(p);
            let (tri, got) =
                mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, p, f32::INFINITY, |_| false).unwrap();
            assert!((got - want).abs() <= 1e-9, "point {i}: {got} against {want}");
            // A limit just past the answer finds the same triangle; one short of it finds nothing.
            let again = mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, p, got * 1.0001 + 1e-12, |_| false);
            assert_eq!(again.map(|(_, d)| d), Some(got));
            if got > 1e-6 {
                assert!(mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, p, got * 0.99, |_| false).is_none());
            }
            // A skipped triangle is never the answer.
            let other =
                mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, p, f32::INFINITY, |t| t == tri).unwrap();
            assert!(other.0 != tri && other.1 >= got);
        }
    }

    #[test]
    fn the_closest_point_of_a_triangle_is_in_each_of_its_regions() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X, Vec3::Y);
        let near = |p: Vec3, want: Vec3| assert!(closest_on_triangle(p, a, b, c).distance(want) < 1e-6, "{p}");
        near(Vec3::new(0.25, 0.25, 3.0), Vec3::new(0.25, 0.25, 0.0));
        near(Vec3::new(-1.0, -1.0, 0.5), a);
        near(Vec3::new(2.0, -0.5, 0.0), b);
        near(Vec3::new(-0.5, 2.0, 0.0), c);
        near(Vec3::new(0.5, -1.0, 0.0), Vec3::new(0.5, 0.0, 0.0));
        near(Vec3::new(-1.0, 0.5, 0.0), Vec3::new(0.0, 0.5, 0.0));
        near(Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.5, 0.5, 0.0));
        // A triangle squashed to a line still answers with a point of it.
        assert!(closest_on_triangle(Vec3::Y, a, b, b).is_finite());
    }
}
