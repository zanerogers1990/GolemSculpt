//! The spatial index of a mesh that grows: what dynamic topology asks of it.
//!
//! The tree is built once over a mesh whose triangles and vertices have been
//! put in leaf order, and nothing may move after that: vertex and triangle
//! numbers are what undo records, the mask and hidden faces are written in.
//! So detail added under the brush never goes into an existing leaf. New
//! triangles and vertices are appended to the mesh, and the runs they make
//! become leaves of their own, hung on the tree beside the leaf nearest to
//! them. The old leaves keep every triangle they had, each now a fraction of
//! its old size.
//!
//! Three things follow from that, and this file keeps them true:
//!
//!   - A leaf can come to own a vertex none of its triangles still uses (an
//!     edge flip takes the last one away). Brushes find vertices through
//!     their leaf's bounds, so from the first change on the bounds take in a
//!     leaf's vertices as well as its triangles (`Bvh::loose`).
//!   - Hanging leaves on leaves makes the tree deeper, and the ray and sphere
//!     walks keep their path on a stack of fixed size. Past a set depth the
//!     tree is laid out afresh over the same leaves, which is cheap: a tree
//!     has one node or so per sixty triangles.
//!   - Which leaves are neighbours changes as triangles are rewritten, so the
//!     neighbour lists can take additions. They are never pruned: a stale
//!     neighbour costs a few normals recomputed for nothing, a missing one
//!     leaves a normal wrong.

use glam::Vec3;
use rayon::prelude::*;

use super::{Aabb, Bvh, LEAF_BIT, Leaf, NO_PARENT, Node};

/// The deepest the tree may get before it is laid out afresh. The walks in
/// bvh.rs hold one node per level on a stack of 64.
const MAX_DEPTH: usize = 48;

/// Below this many leaves a rebuild is quicker on one thread.
const PARALLEL_LEAVES: usize = 4096;

/// A leaf's runs of triangles and vertices: `[tri_start, tri_end, vert_start,
/// vert_end]`. What an undo record keeps of the leaves a stroke added.
pub type LeafRuns = [u32; 4];

impl Leaf {
    pub(crate) fn runs(&self) -> LeafRuns {
        [self.tri_start, self.tri_end, self.vert_start, self.vert_end]
    }

    pub(crate) fn tri_count(&self) -> usize {
        (self.tri_end - self.tri_start) as usize
    }

    pub(crate) fn tri_range(&self) -> std::ops::Range<usize> {
        self.tri_start as usize..self.tri_end as usize
    }
}

/// How much bigger a box gets (as surface area) by taking `more` in.
fn growth(bounds: Aabb, more: Aabb) -> f32 {
    let area = |b: Aabb| {
        let s = b.size();
        s.x * s.y + s.y * s.z + s.z * s.x
    };
    area(Aabb::union(bounds, more)) - area(bounds)
}

/// The axis a set of boxes is most spread out along, by their centres.
fn widest_axis(ids: &[u32], bounds: &[Aabb], first: u32) -> usize {
    let mut spread = Aabb::EMPTY;
    for &id in ids {
        spread.grow(bounds[(id - first) as usize].center());
    }
    let size = spread.size();
    if size.x >= size.y && size.x >= size.z {
        0
    } else if size.y >= size.z {
        1
    } else {
        2
    }
}

/// Splits `ids` in two halves along their widest axis.
fn halve<'a>(ids: &'a mut [u32], bounds: &[Aabb], first: u32) -> (&'a mut [u32], &'a mut [u32]) {
    let axis = widest_axis(ids, bounds, first);
    let mid = ids.len() / 2;
    let centre = |id: u32| bounds[(id - first) as usize].center()[axis];
    ids.select_nth_unstable_by(mid, |&x, &y| centre(x).total_cmp(&centre(y)));
    ids.split_at_mut(mid)
}

/// Lays a balanced tree over the leaves `ids` into `nodes`, whose first node
/// is number `base` of the whole tree; a tree over n leaves is 2n - 1 nodes,
/// parent before children. Returns the tree's bounds.
fn lay_out(ids: &mut [u32], bounds: &[Aabb], nodes: &mut [Node], base: u32, parent: u32) -> Aabb {
    debug_assert_eq!(nodes.len(), 2 * ids.len() - 1);
    if let [leaf] = *ids {
        nodes[0] = Node { bounds: bounds[leaf as usize], a: leaf | LEAF_BIT, b: 0, parent };
        return nodes[0].bounds;
    }
    let (left, right) = halve(ids, bounds, 0);
    let (root, rest) = nodes.split_first_mut().expect("a tree has a root");
    let (left_nodes, right_nodes) = rest.split_at_mut(2 * left.len() - 1);
    let (a, b) = (base + 1, base + 2 * left.len() as u32);
    let (left_bounds, right_bounds) = if left.len() + right.len() >= PARALLEL_LEAVES {
        rayon::join(|| lay_out(left, bounds, left_nodes, a, base), || lay_out(right, bounds, right_nodes, b, base))
    } else {
        (lay_out(left, bounds, left_nodes, a, base), lay_out(right, bounds, right_nodes, b, base))
    };
    *root = Node { bounds: Aabb::union(left_bounds, right_bounds), a, b, parent };
    root.bounds
}

impl Bvh {
    /// From here on a leaf's bounds take in the vertices it owns.
    pub(crate) fn loosen(&mut self) {
        self.loose = true;
    }

    /// The leaf holding triangle `t`. Leaves are in triangle order, those
    /// added later included, so this is a search rather than a table the
    /// size of the mesh.
    pub(crate) fn leaf_of_triangle(&self, t: u32) -> u32 {
        self.leaves.partition_point(|leaf| leaf.tri_end <= t) as u32
    }

    /// The box the tree has around leaf `leaf`.
    pub(crate) fn leaf_box(&self, leaf: u32) -> Aabb {
        self.nodes[self.leaves[leaf as usize].node as usize].bounds
    }

    /// Makes leaf `leaf`'s bounds, and those of everything above it, take in
    /// `more`. Never shrinks anything, as during a stroke. What the stroke's
    /// refits keep in the leaf's box takes it in too (see `refit_leaves`):
    /// `more` can hold where a vertex born mid-stroke counts as having
    /// started, which is in neither the live surface nor the box the stroke
    /// found.
    pub(crate) fn enclose(&mut self, leaf: u32, more: Aabb) {
        if self.kept_in.get(leaf as usize) == Some(&self.stroke) {
            let kept = &mut self.kept[leaf as usize];
            *kept = Aabb::union(*kept, more);
        }
        let mut at = self.leaves[leaf as usize].node;
        while at != NO_PARENT {
            let node = &mut self.nodes[at as usize];
            let wider = Aabb::union(node.bounds, more);
            if wider.min == node.bounds.min && wider.max == node.bounds.max {
                break;
            }
            node.bounds = wider;
            at = node.parent;
        }
    }

    /// Records that leaves `a` and `b` share a vertex.
    pub(crate) fn link(&mut self, a: u32, b: u32) {
        if a != b && !self.nbr.contains(a as usize, b) {
            self.nbr.add(a as usize, b);
            self.nbr.add(b as usize, a);
        }
    }

    /// The last leaf, if its runs end where the mesh's triangles and vertices
    /// end (`[.., triangles, .., vertices]`): the one leaf that can take more
    /// of both without anything moving.
    pub(crate) fn tail_leaf(&self, triangles: u32, vertices: u32) -> Option<u32> {
        let last = self.leaves.last()?;
        (last.tri_end == triangles && last.vert_end == vertices).then_some(self.leaves.len() as u32 - 1)
    }

    /// Lets the last leaf take the triangles and vertices appended after it,
    /// up to `triangles` and `vertices`; `more` is the box around them.
    pub(crate) fn extend_tail(&mut self, triangles: u32, vertices: u32, more: Aabb) {
        let leaf = self.leaves.len() as u32 - 1;
        let last = &mut self.leaves[leaf as usize];
        (last.tri_end, last.vert_end) = (triangles, vertices);
        self.vert_leaf.resize(vertices as usize, leaf);
        self.enclose(leaf, more);
    }

    /// Adds a leaf for each of `runs`, which must carry on from the last
    /// leaf's triangles and vertices and from each other, with `bounds` the
    /// box around each. They go in as one small tree beside the existing
    /// leaf that grows least by having them. Returns the first new leaf's
    /// number.
    pub(crate) fn add_leaves(&mut self, runs: &[LeafRuns], bounds: &[Aabb]) -> u32 {
        let first = self.leaves.len() as u32;
        if runs.is_empty() {
            return first;
        }
        for (k, run) in runs.iter().enumerate() {
            let [tri_start, tri_end, vert_start, vert_end] = *run;
            debug_assert_eq!(vert_start as usize, self.vert_leaf.len());
            debug_assert!(self.leaves.last().is_none_or(|last| last.tri_end == tri_start));
            self.leaves.push(Leaf { tri_start, tri_end, vert_start, vert_end, node: 0 });
            self.vert_leaf.resize(vert_end as usize, first + k as u32);
            self.nbr.push_row(&[]);
        }
        let all = bounds.iter().fold(Aabb::EMPTY, |all, b| Aabb::union(all, *b));

        // Down to the leaf that grows least, widening every box on the way.
        let mut depth = 0;
        let mut sibling = NO_PARENT;
        if !self.nodes.is_empty() {
            sibling = 0;
            while self.nodes[sibling as usize].leaf().is_none() {
                let node = &mut self.nodes[sibling as usize];
                node.bounds = Aabb::union(node.bounds, all);
                let (a, b) = (node.a, node.b);
                let costs = (growth(self.nodes[a as usize].bounds, all), growth(self.nodes[b as usize].bounds, all));
                sibling = if costs.0 <= costs.1 { a } else { b };
                depth += 1;
            }
        }

        // That leaf's node becomes a fork: the leaf moves to a node of its
        // own under it, with the new tree beside it. Both go at the end, so
        // children still come after their parents.
        let mut ids: Vec<u32> = (first..first + runs.len() as u32).collect();
        if sibling == NO_PARENT {
            self.hang(&mut ids, bounds, first, NO_PARENT);
        } else {
            let old = self.nodes[sibling as usize];
            let moved = self.nodes.len() as u32;
            self.nodes.push(Node { parent: sibling, ..old });
            self.leaves[(old.a & !LEAF_BIT) as usize].node = moved;
            let tree = self.hang(&mut ids, bounds, first, sibling);
            self.nodes[sibling as usize] =
                Node { bounds: Aabb::union(old.bounds, all), a: moved, b: tree, parent: old.parent };
        }
        self.stamp.resize(self.nodes.len(), 0);

        let below = runs.len().next_power_of_two().trailing_zeros() as usize + 1;
        if depth + 1 + below > MAX_DEPTH {
            self.rebuild_tree();
        }
        first
    }

    /// Appends a balanced tree over the new leaves `ids` to the nodes and
    /// returns its root. `bounds[id - first]` is the box around leaf `id`.
    fn hang(&mut self, ids: &mut [u32], bounds: &[Aabb], first: u32, parent: u32) -> u32 {
        let at = self.nodes.len() as u32;
        if let [leaf] = *ids {
            self.nodes.push(Node { bounds: bounds[(leaf - first) as usize], a: leaf | LEAF_BIT, b: 0, parent });
            self.leaves[leaf as usize].node = at;
            return at;
        }
        self.nodes.push(Node { bounds: Aabb::EMPTY, a: 0, b: 0, parent });
        let (left, right) = halve(ids, bounds, first);
        let a = self.hang(left, bounds, first, at);
        let b = self.hang(right, bounds, first, at);
        let around = Aabb::union(self.nodes[a as usize].bounds, self.nodes[b as usize].bounds);
        self.nodes[at as usize] = Node { bounds: around, a, b, parent };
        at
    }

    /// Lays the tree out afresh, balanced, over the leaves as they are. Each
    /// leaf keeps the bounds it has now, so this is safe in the middle of a
    /// stroke, when they are wider than the live surface on purpose.
    pub(crate) fn rebuild_tree(&mut self) {
        let bounds: Vec<Aabb> = self.leaves.iter().map(|leaf| self.nodes[leaf.node as usize].bounds).collect();
        self.lay_out(&bounds);
    }

    fn lay_out(&mut self, bounds: &[Aabb]) {
        let count = self.leaves.len();
        self.nodes.clear();
        if count > 0 {
            let filler = Node { bounds: Aabb::EMPTY, a: 0, b: 0, parent: NO_PARENT };
            self.nodes.resize(2 * count - 1, filler);
            let mut ids: Vec<u32> = (0..count as u32).collect();
            lay_out(&mut ids, bounds, &mut self.nodes, 0, NO_PARENT);
            for (at, node) in self.nodes.iter().enumerate() {
                if let Some(leaf) = node.leaf() {
                    self.leaves[leaf as usize].node = at as u32;
                }
            }
        }
        self.stamp.clear();
        self.stamp.resize(self.nodes.len(), 0);
        self.generation = 0;
    }

    /// Puts the leaves back to a state an undo record kept: the first `keep`
    /// leaves stay, the last of those takes the runs `tail` (it may have been
    /// extended since, or cut back), and one leaf follows for each of `more`.
    /// Then everything that hangs off the leaves is worked out again from
    /// the mesh as it is now: every leaf's exact bounds, the tree, which
    /// vertex is in which leaf and which leaves are neighbours. A pass over
    /// the whole mesh, for undo and redo only.
    pub(crate) fn restore_leaves(
        &mut self,
        keep: usize,
        tail: LeafRuns,
        more: &[LeafRuns],
        positions: &[Vec3],
        indices: &[u32],
    ) {
        self.leaves.truncate(keep);
        let leaf = |[tri_start, tri_end, vert_start, vert_end]: LeafRuns| Leaf {
            tri_start,
            tri_end,
            vert_start,
            vert_end,
            node: 0,
        };
        if let Some(last) = self.leaves.last_mut() {
            *last = leaf(tail);
        }
        self.leaves.extend(more.iter().map(|&runs| leaf(runs)));

        self.vert_leaf.clear();
        self.vert_leaf.resize(positions.len(), 0);
        for (number, leaf) in self.leaves.iter().enumerate() {
            self.vert_leaf[leaf.vert_start as usize..leaf.vert_end as usize].fill(number as u32);
        }
        let loose = self.loose;
        let bounds: Vec<Aabb> = self
            .leaves
            .par_iter()
            .with_min_len(64)
            .map(|leaf| Bvh::leaf_bounds(leaf, positions, indices, loose))
            .collect();
        self.lay_out(&bounds);
        self.relink(indices);
    }

    /// Works out which leaves are neighbours from scratch, as a build does.
    fn relink(&mut self, indices: &[u32]) {
        let (leaves, vert_leaf) = (&self.leaves, &self.vert_leaf);
        let mut pairs: Vec<(u32, u32)> = leaves
            .par_iter()
            .enumerate()
            .with_min_len(64)
            .flat_map_iter(|(number, leaf)| {
                let number = number as u32;
                indices[leaf.tri_start as usize * 3..leaf.tri_end as usize * 3]
                    .iter()
                    .map(move |&v| vert_leaf[v as usize])
                    .filter(move |&owner| owner != number)
                    .flat_map(move |owner| [(number, owner), (owner, number)])
            })
            .collect();
        pairs.par_sort_unstable();
        pairs.dedup();
        let mut offsets = vec![0u32; leaves.len() + 1];
        for &(leaf, _) in &pairs {
            offsets[leaf as usize + 1] += 1;
        }
        for i in 0..leaves.len() {
            offsets[i + 1] += offsets[i];
        }
        self.nbr = super::Lists::from_offsets(&offsets, pairs.iter().map(|&(_, other)| other).collect());
    }

    /// How many nodes lie between the root and the deepest leaf.
    #[cfg(test)]
    pub(crate) fn depth(&self) -> usize {
        let mut deepest = 0;
        for leaf in &self.leaves {
            let (mut at, mut depth) = (self.nodes[leaf.node as usize].parent, 0);
            while at != NO_PARENT {
                at = self.nodes[at as usize].parent;
                depth += 1;
            }
            deepest = deepest.max(depth);
        }
        deepest
    }

    /// Panics unless the tree is sound: every node reachable once from the
    /// root, children after their parents, each box around what is under it,
    /// each leaf's node pointing back at it, and the walks' stack deep
    /// enough. For tests of everything that changes the tree.
    #[cfg(test)]
    pub(crate) fn assert_sound(&self) {
        if self.leaves.is_empty() {
            assert!(self.nodes.is_empty());
            return;
        }
        assert_eq!(self.nodes.len(), 2 * self.leaves.len() - 1, "a binary tree over the leaves");
        assert_eq!(self.stamp.len(), self.nodes.len());
        assert_eq!(self.nodes[0].parent, NO_PARENT);
        let inside = |outer: Aabb, inner: Aabb| inner.min.cmpge(outer.min).all() && inner.max.cmple(outer.max).all();
        let mut seen = vec![false; self.leaves.len()];
        for (at, node) in self.nodes.iter().enumerate() {
            match node.leaf() {
                Some(leaf) => {
                    assert!(!std::mem::replace(&mut seen[leaf as usize], true), "leaf {leaf} hangs twice");
                    assert_eq!(self.leaves[leaf as usize].node as usize, at);
                }
                None => {
                    for child in [node.a, node.b] {
                        assert!(child as usize > at, "a child comes after its parent");
                        let below = &self.nodes[child as usize];
                        assert_eq!(below.parent as usize, at);
                        assert!(inside(node.bounds, below.bounds), "node {at} does not enclose node {child}");
                    }
                }
            }
        }
        assert!(seen.iter().all(|&s| s), "every leaf hangs on the tree");
        assert!(self.depth() <= MAX_DEPTH, "depth {}", self.depth());
        for pair in self.leaves.windows(2) {
            assert_eq!(pair[0].tri_end, pair[1].tri_start, "leaves are in triangle order");
            assert!(pair[0].vert_end <= pair[1].vert_start, "leaves are in vertex order");
        }
        for leaf in 0..self.leaves.len() as u32 {
            for &other in self.leaf_neighbours(leaf) {
                assert!(self.leaf_neighbours(other).contains(&leaf), "leaf neighbours go both ways");
            }
        }
    }
}
