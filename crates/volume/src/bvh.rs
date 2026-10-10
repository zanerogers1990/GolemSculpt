//! Nearest-point and inside/outside queries against a fixed triangle mesh.
//!
//! A bounding-volume tree over the triangles, built once and then only read,
//! so any number of threads can query it at the same time. The triangles'
//! corners are copied into the tree in leaf order: a query touches a handful
//! of leaves, and each is then one contiguous run of memory rather than
//! three indirections per triangle into the mesh.

use glam::{DVec2, Vec3};
use rayon::prelude::*;
use sculpt_geom::{Aabb, TriMesh, tri};

use crate::scan::{Folded, passed, unit_normal};

/// Triangles per leaf. A nearest-point search ends in a few leaves and tests
/// every triangle in them; eight measured a little faster than four or two,
/// the shallower tree saving more than the extra triangles cost.
const LEAF: usize = 8;
/// Below this many triangles a subtree is not worth handing to another thread.
const PARALLEL: usize = 8192;

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    /// Leaf: its first triangle slot. Otherwise: its second child (the first
    /// is always the next node).
    first: u32,
    max: Vec3,
    /// Triangles in a leaf; 0 for an inner node.
    count: u32,
}

impl Node {
    const EMPTY: Node = Node { min: Vec3::ZERO, first: 0, max: Vec3::ZERO, count: 0 };

    #[inline]
    fn distance_squared(&self, p: Vec3) -> f32 {
        (self.min - p).max(p - self.max).max(Vec3::ZERO).length_squared()
    }
}

/// The nearest point of a mesh to a query point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Closest {
    pub point: Vec3,
    pub distance_squared: f32,
    /// The triangle the point lies on, as numbered in the source mesh. Among
    /// triangles exactly as near (those sharing the edge or corner the point
    /// is on) which one is not specified.
    pub triangle: u32,
}

impl Closest {
    pub fn distance(&self) -> f32 {
        self.distance_squared.sqrt()
    }
}

#[derive(Default)]
pub struct TriangleBvh {
    nodes: Vec<Node>,
    /// Corners of the triangle in each slot.
    corners: Vec<[Vec3; 3]>,
    /// Source triangle of each slot, and the slot of each source triangle.
    ids: Vec<u32>,
    slots: Vec<u32>,
}

/// Nodes in the tree over `n` triangles, and over `n + 1`. The tree always
/// splits a run in half, so its shape, and with it where every node will
/// live, is known before any triangle has been looked at.
fn node_counts(n: usize) -> (usize, usize) {
    if n < LEAF {
        return (1, 1);
    }
    if n == LEAF {
        return (1, 3);
    }
    let (lower, upper) = node_counts(n / 2);
    if n % 2 == 0 { (1 + 2 * lower, 1 + lower + upper) } else { (1 + lower + upper, 1 + 2 * upper) }
}

struct Builder<'a> {
    positions: &'a [Vec3],
    indices: &'a [u32],
    centroids: Vec<Vec3>,
}

impl Builder<'_> {
    fn corners(&self, tri: u32) -> [Vec3; 3] {
        let i = tri as usize * 3;
        [0, 1, 2].map(|c| self.positions[self.indices[i + c] as usize])
    }

    /// Builds the subtree over `ids` into `nodes`, which is exactly the right
    /// length for it, and returns its bounds. `node` and `slot` are where the
    /// two slices start in the whole tree.
    fn build(&self, nodes: &mut [Node], ids: &mut [u32], node: usize, slot: usize) -> Aabb {
        if ids.len() <= LEAF {
            let mut bounds = Aabb::EMPTY;
            for &t in ids.iter() {
                self.corners(t).into_iter().for_each(|p| bounds.grow(p));
            }
            nodes[0] = Node { min: bounds.min, first: slot as u32, max: bounds.max, count: ids.len() as u32 };
            return bounds;
        }
        let spread = ids.iter().fold(Aabb::EMPTY, |mut b, &t| {
            b.grow(self.centroids[t as usize]);
            b
        });
        let size = spread.size();
        let axis = if size.x >= size.y && size.x >= size.z { 0 } else if size.y >= size.z { 1 } else { 2 };
        let mid = ids.len() / 2;
        ids.select_nth_unstable_by(mid, |&a, &b| {
            self.centroids[a as usize][axis].total_cmp(&self.centroids[b as usize][axis])
        });

        let big = ids.len() > PARALLEL;
        let (left_ids, right_ids) = ids.split_at_mut(mid);
        let left_nodes = node_counts(mid).0;
        let (head, rest) = nodes.split_first_mut().expect("a subtree has at least one node");
        let (left, right) = rest.split_at_mut(left_nodes);
        let (right_node, right_slot) = (node + 1 + left_nodes, slot + mid);
        let (a, b) = if big {
            rayon::join(
                || self.build(left, left_ids, node + 1, slot),
                || self.build(right, right_ids, right_node, right_slot),
            )
        } else {
            (self.build(left, left_ids, node + 1, slot), self.build(right, right_ids, right_node, right_slot))
        };
        let bounds = Aabb::union(a, b);
        *head = Node { min: bounds.min, first: (node + 1 + left_nodes) as u32, max: bounds.max, count: 0 };
        bounds
    }
}

impl TriangleBvh {
    pub fn new(mesh: &TriMesh) -> TriangleBvh {
        TriangleBvh::build(&mesh.positions, &mesh.indices)
    }

    /// Builds the tree over a triangle list. The mesh is copied; nothing is
    /// borrowed.
    pub fn build(positions: &[Vec3], indices: &[u32]) -> TriangleBvh {
        let count = indices.len() / 3;
        if count == 0 {
            return TriangleBvh::default();
        }
        let centroids = indices
            .par_chunks_exact(3)
            .map(|t| (positions[t[0] as usize] + positions[t[1] as usize] + positions[t[2] as usize]) / 3.0)
            .collect();
        let builder = Builder { positions, indices, centroids };
        let mut ids: Vec<u32> = (0..count as u32).collect();
        let mut nodes = vec![Node::EMPTY; node_counts(count).0];
        builder.build(&mut nodes, &mut ids, 0, 0);

        let corners = ids.par_iter().map(|&t| builder.corners(t)).collect();
        let mut slots = vec![0u32; count];
        for (slot, &t) in ids.iter().enumerate() {
            slots[t as usize] = slot as u32;
        }
        TriangleBvh { nodes, corners, ids, slots }
    }

    pub fn triangle_count(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| Aabb::new(n.min, n.max))
    }

    /// The corners of source triangle `tri`.
    pub fn triangle(&self, tri: u32) -> [Vec3; 3] {
        self.corners[self.slots[tri as usize] as usize]
    }

    /// Unit normal of source triangle `tri`; zero only if it has no area.
    pub fn triangle_normal(&self, tri: u32) -> Vec3 {
        let [a, b, c] = self.triangle(tri);
        unit_normal(a, b, c)
    }

    /// The nearest point of the mesh to `p`. `None` only for an empty mesh.
    pub fn closest(&self, p: Vec3) -> Option<Closest> {
        self.closest_within(p, f32::INFINITY)
    }

    /// The nearest point of the mesh to `p`, if one is closer than
    /// `max_distance`. A tight limit makes a miss cheap.
    pub fn closest_within(&self, p: Vec3, max_distance: f32) -> Option<Closest> {
        let root = self.nodes.first()?;
        let mut best = max_distance * max_distance;
        let mut found: Option<(usize, Vec3)> = None;
        let mut stack = [(0u32, 0.0f32); 64];
        stack[0] = (0, root.distance_squared(p));
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let (index, distance) = stack[top];
            // The best may have improved since this node was pushed.
            if distance >= best {
                continue;
            }
            let node = &self.nodes[index as usize];
            if node.count > 0 {
                let first = node.first as usize;
                for (slot, [a, b, c]) in self.corners[first..first + node.count as usize].iter().enumerate() {
                    let q = tri::closest_point(p, *a, *b, *c);
                    let d = q.distance_squared(p);
                    if d < best {
                        best = d;
                        found = Some((first + slot, q));
                    }
                }
                continue;
            }
            // Nearer child last, so it is looked at first and the farther one
            // is usually ruled out by what it finds.
            let (near, far) = (index + 1, node.first);
            let dn = self.nodes[near as usize].distance_squared(p);
            let df = self.nodes[far as usize].distance_squared(p);
            let ((near, dn), (far, df)) = if dn <= df { ((near, dn), (far, df)) } else { ((far, df), (near, dn)) };
            if df < best {
                stack[top] = (far, df);
                top += 1;
            }
            if dn < best {
                stack[top] = (near, dn);
                top += 1;
            }
        }
        found.map(|(slot, point)| Closest { point, distance_squared: best, triangle: self.ids[slot] })
    }

    /// [`Self::closest`] for many points at once, across all cores.
    pub fn closest_many(&self, points: &[Vec3]) -> Vec<Option<Closest>> {
        points.par_iter().with_min_len(256).map(|&p| self.closest(p)).collect()
    }

    /// Distance from `p` to the mesh; infinite for an empty mesh.
    pub fn distance(&self, p: Vec3) -> f32 {
        self.closest(p).map_or(f32::INFINITY, |c| c.distance())
    }

    /// How many shells of the mesh `p` is inside of: the signed count of
    /// surface crossings on the line through `p` along x, below `p`. 0 is
    /// outside; overlapping closed shells count once each, and a shell wound
    /// backwards counts negative. Only meaningful for closed meshes.
    ///
    /// Counted with the same exact rule scan conversion uses
    /// ([`crate::scan`]), so a line through a shared edge or a vertex is
    /// never counted twice or missed.
    pub fn winding_number(&self, p: Vec3) -> i32 {
        if self.nodes.is_empty() {
            return 0;
        }
        let (su, sv) = (if p.y < 0.0 { -1.0 } else { 1.0 }, if p.z < 0.0 { -1.0 } else { 1.0 });
        let folded_p = DVec2::new(p.y.abs() as f64, p.z.abs() as f64);
        let mut winding = 0;
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let index = stack[top];
            let node = &self.nodes[index as usize];
            let beside = p.y < node.min.y || p.y > node.max.y || p.z < node.min.z || p.z > node.max.z;
            if beside || node.min.x > p.x {
                continue;
            }
            if node.count == 0 {
                stack[top] = index + 1;
                stack[top + 1] = node.first;
                top += 2;
                continue;
            }
            let first = node.first as usize;
            for corners in &self.corners[first..first + node.count as usize] {
                let q = corners.map(|c| DVec2::new(c.y as f64, c.z as f64));
                let depth = corners.map(|c| c.x as f64);
                let Some(folded) = Folded::new(q, depth, su, sv) else { continue };
                if folded.hit(folded_p).is_some_and(|t| passed(t as f32, p.x)) {
                    winding += folded.winding as i32;
                }
            }
        }
        winding
    }

    /// Whether `p` is inside the solid the mesh encloses.
    pub fn contains(&self, p: Vec3) -> bool {
        self.winding_number(p) != 0
    }

    /// Distance from `p` to the mesh, negative inside it.
    pub fn signed_distance(&self, p: Vec3) -> f32 {
        let distance = self.distance(p);
        if self.contains(p) { -distance } else { distance }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Rng, box_mesh, icosphere};

    fn brute(mesh: &TriMesh, p: Vec3) -> f32 {
        (0..mesh.triangle_count())
            .map(|t| {
                let [a, b, c] = mesh.triangle(t);
                tri::closest_point(p, a, b, c).distance_squared(p)
            })
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn the_tree_has_the_node_count_it_was_promised() {
        fn count(n: usize) -> usize {
            if n <= LEAF { 1 } else { 1 + count(n / 2) + count(n - n / 2) }
        }
        for n in 1..600 {
            assert_eq!(node_counts(n), (count(n), count(n + 1)), "{n} triangles");
        }
    }

    /// The Unity distance-query cases: a 20k-triangle sphere, points mostly
    /// near the surface (where a remesh asks), some far away, some exactly on
    /// vertices.
    #[test]
    fn nearest_matches_brute_force() {
        let mesh = icosphere(5, 0.5, Vec3::ZERO);
        let bvh = TriangleBvh::new(&mesh);
        assert_eq!(bvh.triangle_count(), mesh.triangle_count());
        let mut rng = Rng::new(3);
        for i in 0..4000 {
            let dir = (rng.vec3() - 0.5).normalize();
            let r = if i % 10 == 0 { rng.next() * 2.0 } else { 0.5 + (rng.next() - 0.5) * 0.05 };
            let p = if i % 97 == 0 { mesh.positions[rng.below(mesh.vertex_count())] } else { dir * r };

            let found = bvh.closest(p).unwrap();
            let want = brute(&mesh, p);
            assert!((found.distance_squared - want).abs() <= 1e-7 * want.max(1e-6), "at {p}: {found:?} vs {want}");
            // The triangle it names really is that near, and the point is on it.
            let [a, b, c] = mesh.triangle(found.triangle as usize);
            assert_eq!(tri::closest_point(p, a, b, c), found.point);
            assert_eq!(bvh.triangle(found.triangle), [a, b, c]);
        }
    }

    #[test]
    fn a_limit_finds_only_what_is_within_it() {
        let mesh = icosphere(3, 1.0, Vec3::ZERO);
        let bvh = TriangleBvh::new(&mesh);
        let p = Vec3::new(1.5, 0.0, 0.0);
        assert!(bvh.closest_within(p, 0.4).is_none());
        let found = bvh.closest_within(p, 0.6).unwrap();
        assert!((found.distance() - 0.5).abs() < 0.01);
        assert_eq!(bvh.closest(p), Some(found));
    }

    #[test]
    fn many_at_once_is_the_same_as_one_at_a_time() {
        let mesh = icosphere(4, 1.0, Vec3::new(0.3, 0.0, -0.2));
        let bvh = TriangleBvh::new(&mesh);
        let mut rng = Rng::new(11);
        let points: Vec<Vec3> = (0..3000).map(|_| (rng.vec3() - 0.5) * 3.0).collect();
        let all = bvh.closest_many(&points);
        for (p, found) in points.iter().zip(&all) {
            assert_eq!(*found, bvh.closest(*p));
        }
    }

    #[test]
    fn an_empty_mesh_has_nothing_near() {
        let bvh = TriangleBvh::build(&[], &[]);
        assert!(bvh.is_empty() && bvh.bounds().is_empty());
        assert!(bvh.closest(Vec3::ONE).is_none());
        assert_eq!(bvh.distance(Vec3::ONE), f32::INFINITY);
        assert!(!bvh.contains(Vec3::ONE));
        assert_eq!(bvh.closest_many(&[Vec3::ZERO]), [None]);
    }

    #[test]
    fn one_triangle_is_a_tree_too() {
        let bvh = TriangleBvh::build(&[Vec3::ZERO, Vec3::X, Vec3::Y], &[0, 1, 2]);
        let found = bvh.closest(Vec3::new(0.25, 0.25, 2.0)).unwrap();
        assert_eq!((found.point, found.triangle), (Vec3::new(0.25, 0.25, 0.0), 0));
        assert_eq!(bvh.triangle_normal(0), Vec3::Z);
    }

    #[test]
    fn inside_and_outside_of_a_sphere() {
        let centre = Vec3::new(0.2, -0.1, 0.4);
        let mesh = icosphere(5, 1.0, centre);
        let bvh = TriangleBvh::new(&mesh);
        let mut rng = Rng::new(7);
        for _ in 0..4000 {
            let p = centre + (rng.vec3() - 0.5) * 3.0;
            let r = p.distance(centre);
            if (r - 1.0).abs() > 0.01 {
                assert_eq!(bvh.contains(p), r < 1.0, "at {p}");
                assert!((bvh.signed_distance(p) - (r - 1.0)).abs() < 0.01);
            }
        }
    }

    /// Lines aimed exactly at vertices and along edges: every query point
    /// shares its y and z with a vertex of the mesh.
    #[test]
    fn a_line_through_a_vertex_or_along_an_edge_is_counted_once() {
        let mesh = icosphere(4, 1.0, Vec3::ZERO);
        let bvh = TriangleBvh::new(&mesh);
        let mut lines = 0;
        // Not the vertices on the rim as seen along x: a line through one of
        // those only grazes the sphere, and may fairly be judged to miss it.
        for v in mesh.positions.iter().filter(|v| v.x.abs() > 0.3) {
            for x in [-2.0, -0.5 * v.x.abs(), 0.0, 2.0] {
                let p = Vec3::new(x, v.y, v.z);
                assert_eq!(bvh.winding_number(p), (x.abs() < v.x.abs()) as i32, "at {p}");
            }
            lines += 1;
        }
        assert!(lines > 1000);
        let cube = box_mesh(Vec3::splat(-1.0), Vec3::splat(1.0));
        let bvh = TriangleBvh::new(&cube);
        for (p, want) in [
            (Vec3::new(0.0, 0.0, 0.0), 1),
            (Vec3::new(0.5, 0.5, 0.5), 1),
            (Vec3::new(3.0, 0.0, 0.0), 0),
            (Vec3::new(3.0, 0.5, -0.5), 0),
            (Vec3::new(0.0, 1.5, 0.0), 0),
        ] {
            assert_eq!(bvh.winding_number(p), want, "at {p}");
        }
    }
}
