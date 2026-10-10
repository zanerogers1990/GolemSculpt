//! Spatial index for the sculpt mesh.
//!
//! A binary BVH over triangles with fairly large leaves. Building it also
//! reorders the mesh so that every leaf owns one contiguous run of triangles
//! and one contiguous run of vertices. That ordering is what makes the rest of
//! the engine cheap:
//!   * a brush gathers vertices by walking a handful of contiguous ranges,
//!   * normals/bounds are recomputed leaf-by-leaf in parallel without locks,
//!   * the GPU upload after a dab is a few `write_buffer` calls over ranges.
//!
//! During a stroke the tree is refitted (bounds updated). Under dynamic topology
//! it also grows: new triangles and vertices become leaves of their own beside
//! the old ones, and an old leaf keeps the triangles and vertices it had (see
//! `bvh/grow.rs`, which also lays the tree out again over the same leaves when
//! growth makes it too deep). It is built from scratch only when a mesh is built.

use glam::Vec3;
use rayon::prelude::*;

use super::lists::Lists;

mod grow;
pub use grow::LeafRuns;

pub const LEAF_MAX_TRIS: usize = 64;
const LEAF_BIT: u32 = 1 << 31;
const NO_PARENT: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb =
        Aabb { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) };

    #[inline]
    pub fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    #[inline]
    pub fn union(a: Aabb, b: Aabb) -> Aabb {
        Aabb { min: a.min.min(b.min), max: a.max.max(b.max) }
    }

    #[inline]
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    #[inline]
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    #[inline]
    pub fn dist_sq(&self, p: Vec3) -> f32 {
        (self.min - p).max(p - self.max).max(Vec3::ZERO).length_squared()
    }

    /// Slab test. Returns the entry distance if the ray hits before `t_max`.
    ///
    /// Deliberately a little generous: a box wrongly kept only costs a few
    /// triangle tests, but a box wrongly skipped drops the triangles in it,
    /// and the ray would then sail through the surface.
    #[inline]
    pub fn ray_entry(&self, origin: Vec3, inv_dir: Vec3, t_max: f32) -> Option<f32> {
        let mut near = 0.0f32;
        let mut far = t_max;
        for axis in 0..3 {
            let t0 = (self.min[axis] - origin[axis]) * inv_dir[axis];
            let t1 = (self.max[axis] - origin[axis]) * inv_dir[axis];
            // A ray running exactly along a face of the box gives 0 * inf =
            // NaN. Both comparisons are then false and the axis is left
            // unconstrained, which is right: the ray is within that slab.
            if t0 <= t1 {
                near = near.max(t0);
                far = far.min(t1);
            } else if t1 < t0 {
                near = near.max(t1);
                far = far.min(t0);
            }
        }
        (near * (1.0 - 1e-5) <= far * (1.0 + 1e-5)).then_some(near)
    }
}

#[derive(Clone, Copy)]
pub struct Node {
    pub bounds: Aabb,
    /// Internal: left child node. Leaf: leaf index with `LEAF_BIT` set.
    a: u32,
    /// Internal: right child node. Children always have a higher index than
    /// their parent, so a descending sweep visits children first.
    b: u32,
    parent: u32,
}

impl Node {
    #[inline]
    fn leaf(&self) -> Option<u32> {
        (self.a & LEAF_BIT != 0).then_some(self.a & !LEAF_BIT)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Leaf {
    pub tri_start: u32,
    pub tri_end: u32,
    pub vert_start: u32,
    pub vert_end: u32,
    node: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub tri: u32,
    pub pos: Vec3,
    /// Geometric normal of the hit triangle, facing the ray origin.
    pub normal: Vec3,
    pub bary: [f32; 3],
}

#[derive(Default)]
pub struct Bvh {
    pub nodes: Vec<Node>,
    pub leaves: Vec<Leaf>,
    /// Leaves that share at least one vertex with a leaf (symmetric).
    nbr: Lists,
    /// Owning leaf of each vertex.
    pub vert_leaf: Vec<u32>,
    /// Set once dynamic topology has changed the mesh: a leaf may then own a
    /// vertex that none of its own triangles uses, so its bounds have to take
    /// in its vertices as well as its triangles (see bvh/grow.rs).
    loose: bool,
    /// Scratch for refits: last refit generation that touched each node.
    stamp: Vec<u32>,
    generation: u32,
    scratch: Vec<u32>,
}

/// Result of building: the reordered mesh arrays.
pub struct Built {
    pub bvh: Bvh,
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
    /// old vertex index -> new vertex index (`u32::MAX` if unreferenced).
    pub vert_remap: Vec<u32>,
    /// new triangle index -> old triangle index.
    pub tri_order: Vec<u32>,
}

enum Tmp {
    Leaf { start: usize, end: usize },
    Inner(Box<Tmp>, Box<Tmp>),
}

fn build_rec(ids: &mut [u32], base: usize, centroids: &[Vec3]) -> Tmp {
    let n = ids.len();
    if n <= LEAF_MAX_TRIS {
        return Tmp::Leaf { start: base, end: base + n };
    }
    let mut b = Aabb::EMPTY;
    for &t in ids.iter() {
        b.grow(centroids[t as usize]);
    }
    let ext = b.size();
    let axis = if ext.x >= ext.y && ext.x >= ext.z {
        0
    } else if ext.y >= ext.z {
        1
    } else {
        2
    };
    let mid = n / 2;
    ids.select_nth_unstable_by(mid, |&x, &y| {
        centroids[x as usize][axis].total_cmp(&centroids[y as usize][axis])
    });
    let (l, r) = ids.split_at_mut(mid);
    let (lt, rt) = if n > 16_384 {
        rayon::join(|| build_rec(l, base, centroids), || build_rec(r, base + mid, centroids))
    } else {
        (build_rec(l, base, centroids), build_rec(r, base + mid, centroids))
    };
    Tmp::Inner(Box::new(lt), Box::new(rt))
}

fn flatten(tmp: &Tmp, parent: u32, nodes: &mut Vec<Node>, leaves: &mut Vec<Leaf>) -> u32 {
    let idx = nodes.len() as u32;
    nodes.push(Node { bounds: Aabb::EMPTY, a: 0, b: 0, parent });
    match tmp {
        Tmp::Leaf { start, end } => {
            nodes[idx as usize].a = leaves.len() as u32 | LEAF_BIT;
            leaves.push(Leaf {
                tri_start: *start as u32,
                tri_end: *end as u32,
                vert_start: 0,
                vert_end: 0,
                node: idx,
            });
        }
        Tmp::Inner(l, r) => {
            let a = flatten(l, idx, nodes, leaves);
            let b = flatten(r, idx, nodes, leaves);
            nodes[idx as usize].a = a;
            nodes[idx as usize].b = b;
        }
    }
    idx
}

impl Bvh {
    /// Builds the tree and returns the mesh reordered to match it.
    pub fn build(positions: &[Vec3], indices: &[u32]) -> Built {
        let tri_count = indices.len() / 3;
        let centroids: Vec<Vec3> = indices
            .par_chunks_exact(3)
            .map(|t| {
                (positions[t[0] as usize] + positions[t[1] as usize] + positions[t[2] as usize])
                    * (1.0 / 3.0)
            })
            .collect();
        let mut ids: Vec<u32> = (0..tri_count as u32).collect();

        let mut nodes = Vec::new();
        let mut leaves = Vec::new();
        if tri_count > 0 {
            let tmp = build_rec(&mut ids, 0, &centroids);
            flatten(&tmp, NO_PARENT, &mut nodes, &mut leaves);
        }

        // Triangles in leaf order; vertices numbered by first use, which makes
        // each leaf's owned vertices one contiguous run.
        let mut vert_remap = vec![u32::MAX; positions.len()];
        let mut new_indices = Vec::with_capacity(indices.len());
        let mut next = 0u32;
        for leaf in &mut leaves {
            leaf.vert_start = next;
            for &t in &ids[leaf.tri_start as usize..leaf.tri_end as usize] {
                for c in 0..3 {
                    let old = indices[t as usize * 3 + c] as usize;
                    if vert_remap[old] == u32::MAX {
                        vert_remap[old] = next;
                        next += 1;
                    }
                    new_indices.push(vert_remap[old]);
                }
            }
            leaf.vert_end = next;
        }
        let mut new_positions = vec![Vec3::ZERO; next as usize];
        for (old, &new) in vert_remap.iter().enumerate() {
            if new != u32::MAX {
                new_positions[new as usize] = positions[old];
            }
        }

        let mut vert_leaf = vec![0u32; next as usize];
        for (li, leaf) in leaves.iter().enumerate() {
            vert_leaf[leaf.vert_start as usize..leaf.vert_end as usize].fill(li as u32);
        }

        // Symmetric leaf adjacency through shared vertices.
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        for (li, leaf) in leaves.iter().enumerate() {
            let li = li as u32;
            for &v in &new_indices[leaf.tri_start as usize * 3..leaf.tri_end as usize * 3] {
                let owner = vert_leaf[v as usize];
                if owner != li {
                    pairs.push((li, owner));
                    pairs.push((owner, li));
                }
            }
        }
        pairs.par_sort_unstable();
        pairs.dedup();
        let mut nbr_off = vec![0u32; leaves.len() + 1];
        for &(l, _) in &pairs {
            nbr_off[l as usize + 1] += 1;
        }
        for i in 0..leaves.len() {
            nbr_off[i + 1] += nbr_off[i];
        }
        let nbr = Lists::from_offsets(&nbr_off, pairs.iter().map(|&(_, o)| o).collect());

        let stamp = vec![0u32; nodes.len()];
        let mut bvh =
            Bvh { nodes, leaves, nbr, vert_leaf, loose: false, stamp, generation: 0, scratch: Vec::new() };
        let all: Vec<u32> = (0..bvh.leaves.len() as u32).collect();
        bvh.refit_leaves(&all, &new_positions, &new_indices, false);
        Built { bvh, positions: new_positions, indices: new_indices, vert_remap, tri_order: ids }
    }

    #[inline]
    pub fn leaf_neighbours(&self, leaf: u32) -> &[u32] {
        self.nbr.row(leaf as usize)
    }

    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| n.bounds)
    }

    /// What the tree holds in memory, for `Mesh::memory_bytes`.
    pub fn bytes(&self) -> usize {
        std::mem::size_of_val(self.nodes.as_slice())
            + std::mem::size_of_val(self.leaves.as_slice())
            + self.nbr.bytes()
            + (self.vert_leaf.len() + self.stamp.len() + self.scratch.len()) * 4
    }

    fn leaf_bounds(leaf: &Leaf, positions: &[Vec3], indices: &[u32], loose: bool) -> Aabb {
        let mut b = Aabb::EMPTY;
        for &v in &indices[leaf.tri_start as usize * 3..leaf.tri_end as usize * 3] {
            b.grow(positions[v as usize]);
        }
        if loose {
            for &p in &positions[leaf.vert_start as usize..leaf.vert_end as usize] {
                b.grow(p);
            }
        }
        b
    }

    /// Recomputes the bounds of `leaves` and of every ancestor above them.
    ///
    /// With `grow`, bounds only ever expand. Strokes use that so the tree keeps
    /// enclosing the stroke-start surface as well as the live one, which lets
    /// brushes raycast and gather against either without a second index.
    pub fn refit_leaves(
        &mut self,
        leaves: &[u32],
        positions: &[Vec3],
        indices: &[u32],
        grow: bool,
    ) {
        if leaves.is_empty() {
            return;
        }
        let bounds: Vec<Aabb> = leaves
            .par_iter()
            .with_min_len(64)
            .map(|&l| Self::leaf_bounds(&self.leaves[l as usize], positions, indices, self.loose))
            .collect();

        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.fill(0);
            self.generation = 1;
        }
        let generation = self.generation;
        let mut ancestors = std::mem::take(&mut self.scratch);
        ancestors.clear();
        for (&l, b) in leaves.iter().zip(bounds) {
            let node = self.leaves[l as usize].node as usize;
            self.nodes[node].bounds =
                if grow { Aabb::union(self.nodes[node].bounds, b) } else { b };
            let mut p = self.nodes[node].parent;
            while p != NO_PARENT && self.stamp[p as usize] != generation {
                self.stamp[p as usize] = generation;
                ancestors.push(p);
                p = self.nodes[p as usize].parent;
            }
        }
        // Children have higher indices than parents, so descending order
        // guarantees a node's children are final before it is recomputed.
        ancestors.sort_unstable_by(|a, b| b.cmp(a));
        for &i in &ancestors {
            let n = self.nodes[i as usize];
            let b = Aabb::union(self.nodes[n.a as usize].bounds, self.nodes[n.b as usize].bounds);
            self.nodes[i as usize].bounds = if grow { Aabb::union(n.bounds, b) } else { b };
        }
        self.scratch = ancestors;
    }

    /// Nearest intersection of a ray with the mesh. `pos` supplies vertex
    /// positions, so the same tree can be queried against the live surface or
    /// the stroke-start one.
    pub fn raycast(
        &self,
        indices: &[u32],
        origin: Vec3,
        dir: Vec3,
        cull_backfaces: bool,
        pos: impl Fn(u32) -> Vec3,
    ) -> Option<Hit> {
        // A `skip` that is never true compiles away, so this is the plain
        // ray test with nothing extra per triangle.
        self.raycast_skipping(indices, origin, dir, cull_backfaces, pos, |_| false)
    }

    /// [`Self::raycast`], passing straight through every triangle `skip`
    /// names: hidden faces, which must not stop the pointer.
    pub fn raycast_skipping(
        &self,
        indices: &[u32],
        origin: Vec3,
        dir: Vec3,
        cull_backfaces: bool,
        pos: impl Fn(u32) -> Vec3,
        skip: impl Fn(u32) -> bool,
    ) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv_dir = dir.recip();
        // The triangle test runs in double precision. In single precision the
        // rounding error in a hit's position within its triangle is about one
        // part in a thousand for a dense mesh seen from a normal distance,
        // which leaves hairline gaps along every edge: roughly 1 ray in 140
        // slipped through a million-triangle sphere and "hit" its far side.
        // In double the error is far below `EDGE_SLACK`, which also lets both
        // neighbours accept a ray that runs exactly along their shared edge.
        const EDGE_SLACK: f64 = 1e-9;
        let (origin_d, dir_d) = (origin.as_dvec3(), dir.as_dvec3());
        let mut best: Option<Hit> = None;
        let mut t_max = f32::INFINITY;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if node.bounds.ray_entry(origin, inv_dir, t_max).is_none() {
                continue;
            }
            if let Some(li) = node.leaf() {
                let leaf = &self.leaves[li as usize];
                for tri in leaf.tri_start..leaf.tri_end {
                    if skip(tri) {
                        continue;
                    }
                    let i = tri as usize * 3;
                    let p0 = pos(indices[i]).as_dvec3();
                    let e1 = pos(indices[i + 1]).as_dvec3() - p0;
                    let e2 = pos(indices[i + 2]).as_dvec3() - p0;
                    let pvec = dir_d.cross(e2);
                    let det = e1.dot(pvec);
                    if cull_backfaces {
                        if det <= 1e-40 {
                            continue;
                        }
                    } else if det.abs() <= 1e-40 {
                        continue;
                    }
                    let inv_det = 1.0 / det;
                    let tvec = origin_d - p0;
                    let u = tvec.dot(pvec) * inv_det;
                    if !(-EDGE_SLACK..=1.0 + EDGE_SLACK).contains(&u) {
                        continue;
                    }
                    let qvec = tvec.cross(e1);
                    let v = dir_d.dot(qvec) * inv_det;
                    if v < -EDGE_SLACK || u + v > 1.0 + EDGE_SLACK {
                        continue;
                    }
                    let t = e2.dot(qvec) * inv_det;
                    if t > 1e-6 && t < t_max as f64 {
                        t_max = t as f32;
                        let mut n = e1.cross(e2).normalize_or_zero().as_vec3();
                        if n.dot(dir) > 0.0 {
                            n = -n;
                        }
                        let (u, v) = (u.clamp(0.0, 1.0) as f32, v.clamp(0.0, 1.0) as f32);
                        best = Some(Hit {
                            tri,
                            pos: (origin_d + dir_d * t).as_vec3(),
                            normal: n,
                            bary: [(1.0 - u - v).max(0.0), u, v],
                        });
                    }
                }
            } else {
                // Push the farther child first so the nearer one is popped next.
                let ta = self.nodes[node.a as usize].bounds.ray_entry(origin, inv_dir, t_max);
                let tb = self.nodes[node.b as usize].bounds.ray_entry(origin, inv_dir, t_max);
                match (ta, tb) {
                    (Some(x), Some(y)) => {
                        let (near, far) = if x <= y { (node.a, node.b) } else { (node.b, node.a) };
                        stack[sp] = far;
                        stack[sp + 1] = near;
                        sp += 2;
                    }
                    (Some(_), None) => {
                        stack[sp] = node.a;
                        sp += 1;
                    }
                    (None, Some(_)) => {
                        stack[sp] = node.b;
                        sp += 1;
                    }
                    (None, None) => {}
                }
            }
        }
        best
    }

    /// Appends every leaf whose bounds intersect the sphere.
    pub fn leaves_in_sphere(&self, center: Vec3, radius: f32, out: &mut Vec<u32>) {
        if self.nodes.is_empty() {
            return;
        }
        let r2 = radius * radius;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if node.bounds.dist_sq(center) > r2 {
                continue;
            }
            if let Some(li) = node.leaf() {
                out.push(li);
            } else {
                stack[sp] = node.a;
                stack[sp + 1] = node.b;
                sp += 2;
            }
        }
    }
}
