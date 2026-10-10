//! The armature itself: a tree of spheres (position, radius, parent/child
//! links). Pure data: no input, no drawing.
//!
//! Everything here is in the rig's own space, whose x = 0 plane is the mirror
//! plane. Where that space stands in the world is `frame.rs`'s business.
//!
//! Symmetry is derived, not stored. A sphere off the mirror plane (x = 0) has
//! a reflection, but the reflection is not a node; [`Rig::geometry`] computes
//! it every time. There is only ever one half to edit, so the two halves
//! cannot drift apart through an add, move, insert, delete or undo.
//!
//! Deleted nodes are tombstoned (`alive = false`) rather than removed. Every
//! link is an index into the node list, and so are the selection and the drag
//! target; compacting would invalidate all of them at once.

use glam::Vec3;

use crate::mesh::bvh::Aabb;

/// How close to the mirror plane, as a fraction of a sphere's own radius,
/// counts as being on it. Inside this band [`snap_to_axis`] pins x to exactly
/// zero, so "is this sphere central?" is an exact test.
pub const AXIS_SNAP_FRACTION: f32 = 0.15;

/// How far apart (as a fraction of radius) a node and a would-be reflection
/// may be and still count as one sphere when a baked half is folded back up.
/// Tight on purpose: a bake makes exact reflections, so anything further off
/// is a deliberate edit and must not be silently undone.
const MERGE_TOLERANCE: f32 = 0.05;

const MIN_RADIUS: f32 = 1e-4;

#[derive(Clone, Debug)]
pub struct Node {
    pub position: Vec3,
    pub radius: f32,
    pub parent: Option<usize>,
    pub alive: bool,
    pub children: Vec<usize>,
}

impl Node {
    /// Whether this sphere sits on the mirror plane and so is its own
    /// reflection. Exact, because [`snap_to_axis`] pins anything close enough.
    pub fn on_axis(&self) -> bool {
        self.position.x == 0.0
    }

    fn central(&self) -> bool {
        self.position.x.abs() <= self.radius * AXIS_SNAP_FRACTION
    }
}

/// One sphere as it appears in the world: a node, or a node's reflection.
/// What is skinned, drawn and picked all come from the same list.
#[derive(Clone, Copy, Debug)]
pub struct SphereInstance {
    pub node: usize,
    pub mirrored: bool,
    pub centre: Vec3,
    pub radius: f32,
}

/// One tapered link as it appears in the world, named by its child node:
/// every non-root node has exactly one link, running up to its parent.
#[derive(Clone, Copy, Debug)]
pub struct LinkInstance {
    pub child: usize,
    pub mirrored: bool,
    pub a: Vec3,
    pub b: Vec3,
    pub radius_a: f32,
    pub radius_b: f32,
}

pub fn mirror(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

/// Pins x to exactly zero when the position is within the snap band, so a
/// centre-line sphere is exactly central rather than a hair off it (and is
/// never drawn twice, itself and a reflection a thousandth of a unit away).
pub fn snap_to_axis(mut position: Vec3, radius: f32) -> Vec3 {
    if position.x.abs() <= radius * AXIS_SNAP_FRACTION {
        position.x = 0.0;
    }
    position
}

#[derive(Default)]
pub struct Rig {
    nodes: Vec<Node>,
    version: u64,
    alive: usize,
}

impl Rig {
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Bumped by every change. Anything that mirrors the rig (the drawn
    /// armature, the skin, the pick lists) compares against its own last-seen
    /// value.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn alive_count(&self) -> usize {
        self.alive
    }

    pub fn is_empty(&self) -> bool {
        self.alive == 0
    }

    pub fn is_alive(&self, index: usize) -> bool {
        self.nodes.get(index).is_some_and(|n| n.alive)
    }

    /// The live node at `index`. `None` rather than a panic: indices held
    /// across frames (the selection, a drag target) can be deleted underneath
    /// their holder, and every such holder wants to quietly stand down.
    pub fn get(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index).filter(|n| n.alive)
    }

    /// The live parent of a node, if it has one.
    pub fn parent_of(&self, index: usize) -> Option<usize> {
        self.get(index)?.parent.filter(|&p| self.is_alive(p))
    }

    // ---- building ---------------------------------------------------------

    fn append(&mut self, position: Vec3, radius: f32, parent: Option<usize>) -> usize {
        self.nodes.push(Node {
            position,
            radius: radius.max(MIN_RADIUS),
            parent,
            alive: true,
            children: Vec::new(),
        });
        self.alive += 1;
        self.version += 1;
        self.nodes.len() - 1
    }

    pub fn add_root(&mut self, position: Vec3, radius: f32) -> usize {
        self.append(position, radius, None)
    }

    pub fn add_child(&mut self, parent: usize, position: Vec3, radius: f32) -> Option<usize> {
        if !self.is_alive(parent) {
            return None;
        }
        let index = self.append(position, radius, Some(parent));
        self.nodes[parent].children.push(index);
        Some(index)
    }

    /// Splices a new sphere into the middle of a link, between `child` and its
    /// parent. This is how a finished chain gains volume where it needs it.
    pub fn insert_between(&mut self, child: usize, position: Vec3, radius: f32) -> Option<usize> {
        let parent = self.parent_of(child)?;
        let mid = self.append(position, radius, Some(parent));
        // The parent swaps `child` for `mid` rather than gaining a second
        // entry, or the limb would fork instead of lengthening.
        match self.nodes[parent].children.iter().position(|&c| c == child) {
            Some(slot) => self.nodes[parent].children[slot] = mid,
            None => self.nodes[parent].children.push(mid),
        }
        self.nodes[mid].children.push(child);
        self.nodes[child].parent = Some(mid);
        Some(mid)
    }

    /// Tombstones `index` and everything below it.
    pub fn remove(&mut self, index: usize) {
        if !self.is_alive(index) {
            return;
        }
        if let Some(parent) = self.parent_of(index) {
            self.nodes[parent].children.retain(|&c| c != index);
        }
        self.nodes[index].parent = None;
        let mut doomed = Vec::new();
        self.collect_subtree(index, &mut doomed);
        for i in doomed {
            let node = &mut self.nodes[i];
            node.alive = false;
            node.children.clear();
            self.alive -= 1;
        }
        self.version += 1;
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.alive = 0;
        self.version += 1;
    }

    // ---- snapshots --------------------------------------------------------

    /// A copy of the whole tree, for undo. Whole-tree rather than a delta: a
    /// rig is a few dozen small nodes.
    pub fn snapshot(&self) -> Vec<Node> {
        self.nodes.clone()
    }

    /// Replaces the whole tree with a snapshot. Its indices were this list's
    /// own, so nothing needs remapping.
    pub fn restore(&mut self, snapshot: &[Node]) {
        self.nodes = snapshot.to_vec();
        self.alive = self.nodes.iter().filter(|n| n.alive).count();
        self.version += 1;
    }

    // ---- editing ----------------------------------------------------------

    pub fn set_position(&mut self, index: usize, position: Vec3) {
        if self.is_alive(index) {
            self.nodes[index].position = position;
            self.version += 1;
        }
    }

    pub fn set_radius(&mut self, index: usize, radius: f32) {
        if self.is_alive(index) {
            self.nodes[index].radius = radius.max(MIN_RADIUS);
            self.version += 1;
        }
    }

    /// Appends `index` and every live descendant to `into`, breadth-first.
    pub fn collect_subtree(&self, index: usize, into: &mut Vec<usize>) {
        if !self.is_alive(index) {
            return;
        }
        let mut cursor = into.len();
        into.push(index);
        while cursor < into.len() {
            for &c in &self.nodes[into[cursor]].children {
                if self.is_alive(c) {
                    into.push(c);
                }
            }
            cursor += 1;
        }
    }

    /// Every live node, parents before children. Walked from the roots,
    /// because index order does not guarantee it: [`Self::insert_between`]
    /// appends the new middle node after the child it adopts.
    pub fn collect_hierarchy(&self, into: &mut Vec<usize>) {
        for i in 0..self.nodes.len() {
            if self.nodes[i].alive && self.parent_of(i).is_none() {
                self.collect_subtree(i, into);
            }
        }
    }

    /// Moves `index` (with its subtree) under `new_parent`, or makes it a root.
    fn reparent(&mut self, index: usize, new_parent: Option<usize>) {
        if !self.is_alive(index) || new_parent == Some(index) {
            return;
        }
        if let Some(parent) = self.parent_of(index) {
            self.nodes[parent].children.retain(|&c| c != index);
        }
        let new_parent = new_parent.filter(|&p| self.is_alive(p));
        self.nodes[index].parent = new_parent;
        if let Some(p) = new_parent {
            self.nodes[p].children.push(index);
        }
        self.version += 1;
    }

    // ---- symmetry ---------------------------------------------------------

    /// Every sphere and link the rig puts in the world, reflections included.
    ///
    /// The mirroring rules in full:
    ///   - a node off the plane contributes its reflection as well as itself;
    ///   - a node on the plane is its own reflection, so it contributes once;
    ///   - a reflected link runs between the reflections of its two ends, and
    ///     an on-plane end reflects onto itself, which is what hangs both arms
    ///     off the one chest sphere.
    pub fn geometry(&self, symmetry: bool) -> (Vec<SphereInstance>, Vec<LinkInstance>) {
        let mut spheres = Vec::new();
        let mut links = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if !node.alive {
                continue;
            }
            let on_axis = node.on_axis();
            spheres.push(SphereInstance { node: i, mirrored: false, centre: node.position, radius: node.radius });
            if symmetry && !on_axis {
                spheres.push(SphereInstance {
                    node: i,
                    mirrored: true,
                    centre: mirror(node.position),
                    radius: node.radius,
                });
            }
            let Some(parent) = self.parent_of(i).map(|p| &self.nodes[p]) else { continue };
            links.push(LinkInstance {
                child: i,
                mirrored: false,
                a: parent.position,
                b: node.position,
                radius_a: parent.radius,
                radius_b: node.radius,
            });
            // A link with both ends on the plane reflects onto itself.
            if symmetry && !(on_axis && parent.on_axis()) {
                links.push(LinkInstance {
                    child: i,
                    mirrored: true,
                    a: mirror(parent.position),
                    b: mirror(node.position),
                    radius_a: parent.radius,
                    radius_b: node.radius,
                });
            }
        }
        (spheres, links)
    }

    /// Turns the derived reflection into real nodes, so the rig keeps the
    /// shape it shows once symmetry stops being applied. Returns how many
    /// nodes were created. Without it, half the blockout would vanish the
    /// moment symmetry was switched off.
    pub fn bake_symmetry(&mut self) -> usize {
        let mut order = Vec::new();
        self.collect_hierarchy(&mut order);
        let mut mirror_of: Vec<Option<usize>> = vec![None; self.nodes.len()];
        let mut created = 0;
        for index in order {
            // On the plane: its own reflection, so children mirroring under
            // it hang off the original node rather than a copy.
            if self.nodes[index].on_axis() {
                mirror_of[index] = Some(index);
                continue;
            }
            let position = mirror(self.nodes[index].position);
            let radius = self.nodes[index].radius;
            let made = match self.parent_of(index).and_then(|p| mirror_of[p]) {
                Some(parent) => self.add_child(parent, position, radius),
                None => Some(self.add_root(position, radius)),
            };
            mirror_of[index] = made;
            created += made.is_some() as usize;
        }
        created
    }

    /// The inverse of [`Self::bake_symmetry`], for switching symmetry back
    /// on: finds real nodes that are exact reflections of a node on the other
    /// side (same radius, hanging off the reflection of that node's parent)
    /// and removes one of each pair, since the derived reflection is about to
    /// stand in for it. Returns how many nodes were removed.
    ///
    /// A reflection that no longer matches (an arm moved while symmetry was
    /// off) is left alone: which side wins is the artist's call. Unmatched
    /// children of a removed node (fingers added to one hand only) are
    /// reflected across and re-hung under the surviving twin, so their derived
    /// reflection reappears exactly where they were.
    pub fn merge_mirror_pairs(&mut self) -> usize {
        let mut order = Vec::new();
        self.collect_hierarchy(&mut order);
        let roots: Vec<usize> =
            (0..self.nodes.len()).filter(|&i| self.nodes[i].alive && self.parent_of(i).is_none()).collect();

        let mut matched: Vec<Option<usize>> = vec![None; self.nodes.len()];
        let mut claimed = vec![false; self.nodes.len()];
        let mut pairs = Vec::new();

        for i in order {
            let node = &self.nodes[i];
            if node.central() {
                matched[i] = Some(i);
                continue;
            }
            if node.position.x < 0.0 || claimed[i] {
                continue;
            }
            let candidates: &[usize] = match self.parent_of(i) {
                Some(parent) => match matched[parent] {
                    Some(twin) => &self.nodes[twin].children,
                    None => continue,
                },
                None => &roots,
            };
            let target = mirror(node.position);
            let tolerance = node.radius * MERGE_TOLERANCE;
            let mut best: Option<(usize, f32)> = None;
            for &candidate in candidates {
                if candidate == i || !self.is_alive(candidate) || claimed[candidate] {
                    continue;
                }
                let other = &self.nodes[candidate];
                if other.central() || other.position.x >= 0.0 {
                    continue;
                }
                if (other.radius - node.radius).abs() > tolerance {
                    continue;
                }
                let distance = other.position.distance(target);
                if distance > tolerance || best.is_some_and(|(_, d)| distance >= d) {
                    continue;
                }
                best = Some((candidate, distance));
            }
            let Some((twin, _)) = best else { continue };
            matched[i] = Some(twin);
            matched[twin] = Some(i);
            claimed[twin] = true;
            pairs.push((i, twin));
        }

        // Deepest pairs first, so a node's unmatched children are re-homed
        // before any removal higher up the chain takes them down with it.
        let mut subtree = Vec::new();
        for &(keep, twin) in pairs.iter().rev() {
            let children = self.nodes[twin].children.clone();
            for child in children.into_iter().rev() {
                if !self.is_alive(child) || claimed[child] {
                    continue;
                }
                subtree.clear();
                self.collect_subtree(child, &mut subtree);
                for &s in &subtree {
                    self.nodes[s].position = mirror(self.nodes[s].position);
                }
                self.reparent(child, Some(keep));
            }
        }

        let before = self.alive;
        for &(_, twin) in &pairs {
            self.remove(twin);
        }
        before - self.alive
    }

    /// Pins every node inside the snap band exactly onto the plane: nodes
    /// left a hair off centre while symmetry was off would otherwise be drawn
    /// twice.
    pub fn snap_central_to_axis(&mut self) -> usize {
        let mut snapped = 0;
        for node in &mut self.nodes {
            if node.alive && node.position.x != 0.0 && node.central() {
                node.position.x = 0.0;
                snapped += 1;
            }
        }
        if snapped > 0 {
            self.version += 1;
        }
        snapped
    }

    // ---- queries ----------------------------------------------------------

    /// Bounds of every live sphere (centre and radius), with reflections when
    /// symmetry is on.
    pub fn bounds(&self, symmetry: bool) -> Aabb {
        let mut bounds = Aabb::EMPTY;
        for node in self.nodes.iter().filter(|n| n.alive) {
            let r = Vec3::splat(node.radius);
            bounds.grow(node.position - r);
            bounds.grow(node.position + r);
            if symmetry && !node.on_axis() {
                bounds.grow(mirror(node.position) - r);
                bounds.grow(mirror(node.position) + r);
            }
        }
        bounds
    }

    /// Smallest live radius. Drives the skin's resolution: voxels are sized
    /// so the thinnest sphere in the rig still survives the grid.
    pub fn min_radius(&self) -> f32 {
        let min = self.nodes.iter().filter(|n| n.alive).map(|n| n.radius).fold(f32::MAX, f32::min);
        if min == f32::MAX { 0.0 } else { min }
    }

    pub fn mean_radius(&self) -> f32 {
        if self.alive == 0 {
            return 0.0;
        }
        self.nodes.iter().filter(|n| n.alive).map(|n| n.radius).sum::<f32>() / self.alive as f32
    }
}
