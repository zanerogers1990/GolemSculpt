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

use std::sync::atomic::{AtomicU32, Ordering};

use glam::Vec3;
use rayon::prelude::*;

use super::lists::Lists;
use crate::util::SharedMut;

#[cfg(test)]
mod build_tests;
mod grow;
pub mod nearest;
#[cfg(test)]
mod refit_tests;
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

#[derive(Clone, Default)]
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
    /// During a stroke, what each leaf's box has to go on holding besides
    /// the live surface: its box as the stroke found it, and whatever
    /// dynamic topology has had it take in since (see [`Self::refit_leaves`]).
    /// Taken the first time the stroke refits the leaf; `kept_in[leaf]` is
    /// the stroke it was taken in.
    kept: Vec<Aabb>,
    kept_in: Vec<u32>,
    /// The stroke under way, counted from 1; 0 is none yet.
    stroke: u32,
    /// The live box of each leaf a refit is working on.
    live: Vec<Aabb>,
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

/// How many leaves the tree over `n` triangles has, and the tree over
/// `n + 1`. A tree's shape follows from its size alone, since every split is
/// at the middle, so the place of every node and leaf is known before any of
/// them is made.
fn leaf_counts(n: usize) -> (usize, usize) {
    if n < LEAF_MAX_TRIS {
        (1, 1)
    } else if n == LEAF_MAX_TRIS {
        (1, 2)
    } else {
        let (a, b) = leaf_counts(n / 2);
        if n % 2 == 0 { (2 * a, a + b) } else { (a + b, 2 * b) }
    }
}

/// Where a subtree goes: its node and its first leaf, numbered in the order
/// a walk that takes the left side first meets them, and its parent.
#[derive(Clone, Copy)]
struct Place {
    node: usize,
    leaf: usize,
    parent: u32,
}

/// The tree over `ids`, the triangles from `base` on, put in its places in
/// `nodes` and `leaves`; `ids` is left in leaf order. Halves are built on
/// threads of their own, each writing its own places.
fn build_rec(ids: &mut [u32], base: usize, centroids: &[Vec3], at: Place, nodes: &SharedMut<Node>, leaves: &SharedMut<Leaf>) {
    let n = ids.len();
    if n <= LEAF_MAX_TRIS {
        let node = Node { bounds: Aabb::EMPTY, a: at.leaf as u32 | LEAF_BIT, b: 0, parent: at.parent };
        let leaf = Leaf { tri_start: base as u32, tri_end: (base + n) as u32, vert_start: 0, vert_end: 0, node: at.node as u32 };
        // SAFETY: every node and every leaf has a place of its own.
        unsafe {
            nodes.write(at.node, node);
            leaves.write(at.leaf, leaf);
        }
        return;
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
    let left_leaves = leaf_counts(mid).0;
    let left = Place { node: at.node + 1, leaf: at.leaf, parent: at.node as u32 };
    let right = Place { node: at.node + 2 * left_leaves, leaf: at.leaf + left_leaves, parent: at.node as u32 };
    // SAFETY: as for a leaf.
    unsafe { nodes.write(at.node, Node { bounds: Aabb::EMPTY, a: left.node as u32, b: right.node as u32, parent: at.parent }) };
    let (l, r) = ids.split_at_mut(mid);
    if n > 16_384 {
        rayon::join(
            || build_rec(l, base, centroids, left, nodes, leaves),
            || build_rec(r, base + mid, centroids, right, nodes, leaves),
        );
    } else {
        build_rec(l, base, centroids, left, nodes, leaves);
        build_rec(r, base + mid, centroids, right, nodes, leaves);
    }
}

/// Numbers the vertices by first use, triangles taken in the order of `ids`
/// (leaf order), and gives each leaf the run of numbers its triangles handed
/// out. Returns where each vertex went (`u32::MAX` for one no triangle
/// uses), the triangles in leaf order with the new numbers, and the
/// positions in the new order.
///
/// A walk in one order, shared between threads without changing a number:
/// each vertex's first use is the lowest corner that names it, so that is
/// found first; each leaf then counts the first uses among its corners, and
/// the counts, added up in leaf order, say where each leaf's run begins.
fn number_by_first_use(
    positions: &[Vec3],
    indices: &[u32],
    ids: &[u32],
    leaves: &mut [Leaf],
) -> (Vec<u32>, Vec<u32>, Vec<Vec3>) {
    let corner = |k: usize| indices[ids[k / 3] as usize * 3 + k % 3] as usize;
    let corners = ids.len() * 3;
    let first: Vec<AtomicU32> = (0..positions.len()).into_par_iter().map(|_| AtomicU32::new(u32::MAX)).collect();
    (0..corners).into_par_iter().with_min_len(4096).for_each(|k| {
        first[corner(k)].fetch_min(k as u32, Ordering::Relaxed);
    });
    let is_first = |k: usize| first[corner(k)].load(Ordering::Relaxed) == k as u32;
    let corners_of = |leaf: &Leaf| leaf.tri_start as usize * 3..leaf.tri_end as usize * 3;

    let counts: Vec<u32> = leaves.par_iter().map(|leaf| corners_of(leaf).filter(|&k| is_first(k)).count() as u32).collect();
    let mut next = 0u32;
    for (leaf, count) in leaves.iter_mut().zip(counts) {
        leaf.vert_start = next;
        next += count;
        leaf.vert_end = next;
    }

    let mut vert_remap = vec![u32::MAX; positions.len()];
    let mut old_of_new = vec![0u32; next as usize];
    {
        let (remap, olds) = (SharedMut::new(&mut vert_remap), SharedMut::new(&mut old_of_new));
        leaves.par_iter().for_each(|leaf| {
            let mut new = leaf.vert_start;
            for k in corners_of(leaf).filter(|&k| is_first(k)) {
                let old = corner(k);
                // SAFETY: only a vertex's first use writes its number, and
                // each leaf hands out numbers from its own run.
                unsafe {
                    remap.write(old, new);
                    olds.write(new as usize, old as u32);
                }
                new += 1;
            }
        });
    }
    let new_indices: Vec<u32> = (0..corners).into_par_iter().with_min_len(4096).map(|k| vert_remap[corner(k)]).collect();
    let new_positions: Vec<Vec3> = old_of_new.par_iter().map(|&old| positions[old as usize]).collect();
    (vert_remap, new_indices, new_positions)
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

        let leaf_count = if tri_count > 0 { leaf_counts(tri_count).0 } else { 0 };
        let blank = Node { bounds: Aabb::EMPTY, a: 0, b: 0, parent: NO_PARENT };
        let mut nodes = vec![blank; (2 * leaf_count).saturating_sub(1)];
        let mut leaves = vec![Leaf { tri_start: 0, tri_end: 0, vert_start: 0, vert_end: 0, node: 0 }; leaf_count];
        if tri_count > 0 {
            let top = Place { node: 0, leaf: 0, parent: NO_PARENT };
            build_rec(&mut ids, 0, &centroids, top, &SharedMut::new(&mut nodes), &SharedMut::new(&mut leaves));
        }

        // Triangles in leaf order; vertices numbered by first use, which makes
        // each leaf's owned vertices one contiguous run.
        let (vert_remap, new_indices, new_positions) = number_by_first_use(positions, indices, &ids, &mut leaves);
        let vert_leaf: Vec<u32> = leaves
            .par_iter()
            .enumerate()
            .flat_map_iter(|(li, leaf)| std::iter::repeat_n(li as u32, (leaf.vert_end - leaf.vert_start) as usize))
            .collect();

        // Symmetric leaf adjacency through shared vertices: the leaves owning
        // each leaf's corners, both ways round.
        let owners = |(li, leaf): (usize, &Leaf)| {
            let corners = &new_indices[leaf.tri_start as usize * 3..leaf.tri_end as usize * 3];
            let mut owners: Vec<u32> =
                corners.iter().map(|&v| vert_leaf[v as usize]).filter(|&owner| owner != li as u32).collect();
            owners.sort_unstable();
            owners.dedup();
            owners.into_iter().flat_map(move |owner| [(li as u32, owner), (owner, li as u32)])
        };
        let mut pairs: Vec<(u32, u32)> = leaves.par_iter().enumerate().flat_map_iter(owners).collect();
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
        let mut bvh = Bvh {
            nodes,
            leaves,
            nbr,
            vert_leaf,
            loose: false,
            stamp,
            generation: 0,
            scratch: Vec::new(),
            kept: Vec::new(),
            kept_in: Vec::new(),
            stroke: 0,
            live: Vec::new(),
        };
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
            + (self.vert_leaf.len() + self.stamp.len() + self.scratch.len() + self.kept_in.len()) * 4
            + std::mem::size_of_val(self.kept.as_slice())
            + std::mem::size_of_val(self.live.as_slice())
    }

    /// A stroke begins: each leaf's box as it stands now is what the stroke
    /// found, the first time the stroke refits it.
    pub fn begin_stroke(&mut self) {
        self.stroke = self.stroke.wrapping_add(1);
        if self.stroke == 0 {
            self.kept_in.fill(0);
            self.stroke = 1;
        }
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
    /// `in_stroke` is for strokes: the tree has to go on enclosing the
    /// stroke-start surface as well as the live one, which lets brushes
    /// raycast and gather against either without a second index. A leaf's box
    /// is then its live box together with the box it had when the stroke
    /// first refitted it (which holds the stroke-start surface) and whatever
    /// dynamic topology has had it take in since ([`Self::enclose`]). It used
    /// to take in its previous box every time instead, which held all of that
    /// too but also every place the surface had passed through on the way: a
    /// long Move or Snake Hook stroke left boxes many times the size of what
    /// was in them, and every dab and raycast after it had to open them.
    pub fn refit_leaves(
        &mut self,
        leaves: &[u32],
        positions: &[Vec3],
        indices: &[u32],
        in_stroke: bool,
    ) {
        if leaves.is_empty() {
            return;
        }
        let mut live = std::mem::take(&mut self.live);
        leaves
            .par_iter()
            .with_min_len(64)
            .map(|&l| Self::leaf_bounds(&self.leaves[l as usize], positions, indices, self.loose))
            .collect_into_vec(&mut live);
        if in_stroke {
            self.kept.resize(self.leaves.len(), Aabb::EMPTY);
            self.kept_in.resize(self.leaves.len(), 0);
            for (&l, b) in leaves.iter().zip(live.iter_mut()) {
                let l = l as usize;
                if self.kept_in[l] != self.stroke {
                    self.kept_in[l] = self.stroke;
                    self.kept[l] = self.nodes[self.leaves[l].node as usize].bounds;
                }
                *b = Aabb::union(self.kept[l], *b);
            }
        }

        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.fill(0);
            self.generation = 1;
        }
        let generation = self.generation;
        let mut ancestors = std::mem::take(&mut self.scratch);
        ancestors.clear();
        for (&l, &b) in leaves.iter().zip(&live) {
            let node = self.leaves[l as usize].node as usize;
            self.nodes[node].bounds = b;
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
            self.nodes[i as usize].bounds = Aabb::union(self.nodes[n.a as usize].bounds, self.nodes[n.b as usize].bounds);
        }
        self.scratch = ancestors;
        self.live = live;
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

    /// Appends every leaf whose bounds meet any of `spheres` (centre,
    /// radius), each once: the leaves [`Self::leaves_in_sphere`] finds for
    /// each, in one walk of the tree. (A box takes in its children's, so a
    /// leaf one sphere finds, it finds all the way down.)
    pub fn leaves_in_spheres(&self, spheres: impl Iterator<Item = (Vec3, f32)> + Clone, out: &mut Vec<u32>) {
        if self.nodes.is_empty() {
            return;
        }
        // Kept by the same test as one sphere's walk, NaN included.
        let meets = |bounds: &Aabb| spheres.clone().any(|(centre, radius)| !(bounds.dist_sq(centre) > radius * radius));
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if !meets(&node.bounds) {
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
