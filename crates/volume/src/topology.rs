//! Which surface patches pass through a lattice cell, so that the dual
//! (Surface Nets) extraction comes out 2-manifold.
//!
//! Plain Surface Nets places one vertex per cell the surface passes through
//! and stitches a quad around every lattice edge the field changes sign
//! across. That is only right when the surface crosses the cell as a single
//! patch. Two cases break it:
//!
//!   - An ambiguous face: the four corners of a cell face alternate inside
//!     and outside. All four face edges then emit quads, and all four quads
//!     share the edge between the two cells either side of that face: an edge
//!     used by four triangles. Tight concave creases produce this (two limbs
//!     nearly touching across a thin wedge of outside).
//!   - Several separate patches inside one cell (two diagonally opposite
//!     inside corners), which collapses separate sheets onto one vertex.
//!
//! The fix is one vertex per patch rather than per cell. A patch is found
//! from the corner signs alone: corners of one sign joined by a cube edge are
//! one region, an ambiguous face also joins one of its two diagonals, and
//! each boundary loop on the cube's surface separates exactly one inside
//! region from one outside region. So a crossing edge's patch is simply the
//! (inside region, outside region) pair it runs between.
//!
//! Which diagonal an ambiguous face joins must be decided identically by both
//! cells that share it, and must not re-create the problem: if the diagonal
//! left separated on the face is still connected through the rest of both
//! cells, the two patches it bounds are one patch on each side and the shared
//! edge is used four times again. At most one diagonal can be connected
//! around a single cell, so joining the one connected through both cells
//! always avoids it. [`joins_inside`] applies that rule, falling back to the
//! asymptotic decider (join the diagonal whose corner values have the larger
//! product). That is exact for a face whose cells have no other ambiguous
//! face; [`resolve_joins`] handles the rest, where decisions interact.
//!
//! Nothing here reads positions, only signs and values, so a field that is
//! symmetric under a reflection extracts to a symmetric surface.
//!
//! Conventions: corner `c` of a cell is at offset (c & 1, c >> 1 & 1,
//! c >> 2 & 1); an inside mask has bit `c` set when corner `c` is inside;
//! edges 0-3 run along X, 4-7 along Y, 8-11 along Z.

use std::sync::LazyLock;

/// The corner each edge starts at and the one it ends at. The end is always
/// the start moved one step up the edge's axis (`edge / 4`).
pub const EDGE_A: [usize; 12] = [0, 2, 4, 6, 0, 1, 4, 5, 0, 1, 2, 3];
pub const EDGE_B: [usize; 12] = [1, 3, 5, 7, 2, 3, 6, 7, 4, 5, 6, 7];

/// Faces 0-5 are x-, x+, y-, y+, z-, z+. Corners are listed in cyclic order
/// and aligned across each opposite pair: corner `k` of face `2a` and corner
/// `k` of face `2a + 1` differ only along axis `a`, so the neighbour across a
/// face sees the same samples at the same positions.
pub const FACE_CORNERS: [[usize; 4]; 6] =
    [[0, 2, 6, 4], [1, 3, 7, 5], [0, 1, 5, 4], [2, 3, 7, 6], [0, 1, 3, 2], [4, 5, 7, 6]];

/// "No neighbour" in the tables [`resolve_joins`] takes.
pub const NO_CELL: u32 = u32::MAX;

struct Tables {
    /// For each mask, the same-sign region each corner belongs to, named by
    /// its lowest corner.
    region: [[u8; 8]; 256],
    ambiguous: [u8; 256],
    simple: [bool; 256],
    /// `flip_edge[f][e]`: the edge whose corners are edge `e`'s with offset
    /// bits `f` flipped.
    flip_edge: [[u8; 12]; 8],
}

static TABLES: LazyLock<Tables> = LazyLock::new(Tables::build);

fn find(parent: &[u8; 8], mut x: usize) -> usize {
    while parent[x] as usize != x {
        x = parent[x] as usize;
    }
    x
}

fn union(parent: &mut [u8; 8], a: usize, b: usize) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a != b {
        parent[a.max(b)] = a.min(b) as u8;
    }
}

#[inline]
fn bit(mask: u8, corner: usize) -> bool {
    mask >> corner & 1 != 0
}

impl Tables {
    fn build() -> Tables {
        let mut t =
            Tables { region: [[0; 8]; 256], ambiguous: [0; 256], simple: [false; 256], flip_edge: [[0; 12]; 8] };
        for mask in 0..256usize {
            let mut parent = [0, 1, 2, 3, 4, 5, 6, 7u8];
            for e in 0..12 {
                if bit(mask as u8, EDGE_A[e]) == bit(mask as u8, EDGE_B[e]) {
                    union(&mut parent, EDGE_A[e], EDGE_B[e]);
                }
            }
            for c in 0..8 {
                t.region[mask][c] = find(&parent, c) as u8;
            }
            for (f, fc) in FACE_CORNERS.iter().enumerate() {
                let s = fc.map(|c| bit(mask as u8, c));
                if s[0] == s[2] && s[1] == s[3] && s[0] != s[1] {
                    t.ambiguous[mask] |= 1 << f;
                }
            }
        }
        for mask in 1..255usize {
            t.simple[mask] = t.ambiguous[mask] == 0 && t.components(mask as u8, 0).0 == 1;
        }
        for flips in 0..8 {
            for e in 0..12 {
                let (a, b) = (EDGE_A[e] ^ flips, EDGE_B[e] ^ flips);
                let found = (0..12).find(|&m| (EDGE_A[m], EDGE_B[m]) == (a.min(b), a.max(b)));
                t.flip_edge[flips][e] = found.expect("a flipped edge is an edge") as u8;
            }
        }
        t
    }

    /// Same-sign edge regions of the cell, plus the chosen diagonal of each
    /// face in `faces`.
    fn join_regions(&self, mask: u8, faces: u8, joins: u8) -> [u8; 8] {
        let mut parent = self.region[mask as usize];
        for (f, fc) in FACE_CORNERS.iter().enumerate() {
            if faces >> f & 1 == 0 {
                continue;
            }
            let inside_at = if bit(mask, fc[0]) { 0 } else { 1 };
            let at = if joins >> f & 1 != 0 { inside_at } else { 1 - inside_at };
            union(&mut parent, fc[at], fc[at + 2]);
        }
        parent
    }

    fn components(&self, mask: u8, joins: u8) -> (usize, u32) {
        let parent = self.join_regions(mask, self.ambiguous[mask as usize], joins);
        let mut keys = [0usize; 4];
        let mut count = 0;
        let mut edge_patches = 0u32;
        for e in 0..12 {
            let (a, b) = (EDGE_A[e], EDGE_B[e]);
            if bit(mask, a) == bit(mask, b) {
                continue;
            }
            let (inside, outside) = if bit(mask, a) { (a, b) } else { (b, a) };
            let key = find(&parent, inside) * 8 + find(&parent, outside);
            let index = keys[..count].iter().position(|&k| k == key).unwrap_or_else(|| {
                keys[count] = key;
                count += 1;
                count - 1
            });
            edge_patches |= (index as u32) << (2 * e);
        }
        (count, edge_patches)
    }
}

/// True when the cell holds exactly one patch and has no ambiguous face: the
/// overwhelmingly common case.
#[inline]
pub fn is_simple(mask: u8) -> bool {
    TABLES.simple[mask as usize]
}

/// Bit `f` is set when face `f` of the cell is ambiguous.
#[inline]
pub fn ambiguous_faces(mask: u8) -> u8 {
    TABLES.ambiguous[mask as usize]
}

/// The axis face `f` is perpendicular to.
#[inline]
pub fn face_axis(face: usize) -> usize {
    face >> 1
}

/// Which way the neighbour across face `f` lies along [`face_axis`].
#[inline]
pub fn face_step(face: usize) -> i32 {
    if face & 1 == 0 { -1 } else { 1 }
}

/// Whether edge `e` of a cell with this mask has the surface crossing it.
#[inline]
pub fn edge_crosses(mask: u8, edge: usize) -> bool {
    bit(mask, EDGE_A[edge]) != bit(mask, EDGE_B[edge])
}

/// The edge that edge `e` becomes when the cell is reflected along the axes
/// in `flips` (bit 0 = x, bit 1 = y, bit 2 = z).
#[inline]
pub fn flip_edge(flips: usize, edge: usize) -> usize {
    TABLES.flip_edge[flips][edge] as usize
}

/// A cell's inside mask as its reflection along the axes in `flips` carries it.
pub fn flip_mask(mask: u8, flips: usize) -> u8 {
    (0..8).filter(|&c| bit(mask, c)).fold(0, |m, c| m | 1 << (c ^ flips))
}

/// The cell's surface patches: how many there are (1 to 4) and, two bits per
/// edge in edge order, which patch each crossing edge belongs to. `joins` has
/// bit `f` set when ambiguous face `f` joins its inside diagonal; bits for
/// unambiguous faces are ignored.
pub fn components(mask: u8, joins: u8) -> (usize, u32) {
    TABLES.components(mask, joins)
}

/// The patch edge `e` belongs to, from the second value [`components`] returns.
#[inline]
pub fn edge_patch(edge_patches: u32, edge: usize) -> usize {
    (edge_patches >> (2 * edge) & 3) as usize
}

/// The products of the inside diagonal's and the outside diagonal's corner
/// values on a face; `values` are the face's corners in [`FACE_CORNERS`] order.
fn diagonal_products(mask: u8, face: usize, values: [f32; 4]) -> (f32, f32) {
    let (even, odd) = (values[0] * values[2], values[1] * values[3]);
    if bit(mask, FACE_CORNERS[face][0]) { (even, odd) } else { (odd, even) }
}

/// Whether ambiguous face `face` of a cell joins its inside diagonal, judged
/// from the two cells sharing it alone. `neighbour` is the mask of the cell
/// across the face; `values` are the face's corner values in [`FACE_CORNERS`]
/// order. The neighbour evaluating the same face (as `face ^ 1`, with the
/// masks swapped) gets the same answer.
pub fn joins_inside(face: usize, mask: u8, neighbour: Option<u8>, values: [f32; 4]) -> bool {
    let t = &*TABLES;
    if let Some(other) = neighbour {
        let (fa, fb) = (FACE_CORNERS[face], FACE_CORNERS[face ^ 1]);
        let inside_at = if bit(mask, fa[0]) { 0 } else { 1 };
        let linked = |at: usize| {
            t.region[mask as usize][fa[at]] == t.region[mask as usize][fa[at + 2]]
                && t.region[other as usize][fb[at]] == t.region[other as usize][fb[at + 2]]
        };
        let (inside_linked, outside_linked) = (linked(inside_at), linked(1 - inside_at));
        if inside_linked != outside_linked {
            return inside_linked;
        }
    }
    let (inside, outside) = diagonal_products(mask, face, values);
    inside > outside
}

/// How decisively the asymptotic decider picks a side of this face: 0 is a
/// coin toss, 1 is certain. Decides which of two interacting faces gives way.
pub fn decider_confidence(mask: u8, face: usize, values: [f32; 4]) -> f32 {
    let (inside, outside) = diagonal_products(mask, face, values);
    (inside - outside).abs() / (inside + outside + 1e-30)
}

/// Whether one diagonal of `face` is connected through the rest of the cell:
/// every same-sign edge and every other ambiguous face's join, but not
/// `face`'s own.
pub fn diagonal_linked(mask: u8, joins: u8, face: usize, inside_diagonal: bool) -> bool {
    let t = &*TABLES;
    let parent = t.join_regions(mask, t.ambiguous[mask as usize] & !(1 << face), joins);
    let fc = FACE_CORNERS[face];
    let inside_at = if bit(mask, fc[0]) { 0 } else { 1 };
    let at = if inside_diagonal { inside_at } else { 1 - inside_at };
    find(&parent, fc[at]) == find(&parent, fc[at + 2])
}

fn face_values(values: &[f32; 8], face: usize) -> [f32; 4] {
    FACE_CORNERS[face].map(|c| values[c])
}

/// Decides every ambiguous face of one extraction together. Returns the
/// join-inside bits for each cell and how many faces were still collapsing
/// when the rounds ran out (0 whenever resolution converged, which is what
/// is expected).
///
/// [`joins_inside`] is exact for a face whose two cells have no other
/// ambiguous face. When a cell has several, each decision changes what is
/// connected around the cell for the others, and the local rule, which
/// assumes the other faces join nothing, can pick the collapsing side. So
/// after the local pass every face is re-checked against the actual decisions
/// of its cells' other faces, and a collapsing face is flipped, which always
/// cures that face. Flips run in rounds, each decided from the previous
/// round's state; where collapsing faces share a cell, only the one the
/// decider was least sure about flips, so neighbours do not flip each other
/// back and forth. Every choice reads signs and values only, so
/// mirror-symmetric input still resolves symmetrically.
///
/// `masks[c]` and `values[c]` describe cell `c`; `neighbours[c][f]` is the
/// cell across face `f`, or [`NO_CELL`]. Only cells that are not single clean
/// patches need be listed: the neighbour across an ambiguous face has that
/// face too, so it is always among them.
pub fn resolve_joins(masks: &[u8], values: &[[f32; 8]], neighbours: &[[u32; 6]]) -> (Vec<u8>, usize) {
    const MAX_ROUNDS: usize = 16;
    let n = masks.len();
    let across = |i: usize, f: usize| -> Option<usize> {
        let j = neighbours[i][f];
        (ambiguous_faces(masks[i]) >> f & 1 != 0 && j != NO_CELL).then_some(j as usize)
    };
    let confidence = |i: usize, f: usize| decider_confidence(masks[i], f, face_values(&values[i], f));

    let mut joins: Vec<u8> = (0..n)
        .map(|i| {
            (0..6).filter(|&f| ambiguous_faces(masks[i]) >> f & 1 != 0).fold(0u8, |bits, f| {
                let other = across(i, f).map(|j| masks[j]);
                bits | (joins_inside(f, masks[i], other, face_values(&values[i], f)) as u8) << f
            })
        })
        .collect();

    let mut priority = vec![f32::MAX; n];
    let mut collapsing: Vec<(usize, usize, usize)> = Vec::new();
    for _ in 0..MAX_ROUNDS {
        collapsing.clear();
        for i in 0..n {
            // Each interior face once, from the cell on its minus side.
            for f in [1, 3, 5] {
                let Some(j) = across(i, f) else { continue };
                let separated_inside = joins[i] >> f & 1 == 0;
                if diagonal_linked(masks[i], joins[i], f, separated_inside)
                    && diagonal_linked(masks[j], joins[j], f ^ 1, separated_inside)
                {
                    collapsing.push((i, f, j));
                }
            }
        }
        if collapsing.is_empty() {
            return (joins, 0);
        }
        for &(i, _, j) in &collapsing {
            priority[i] = f32::MAX;
            priority[j] = f32::MAX;
        }
        for &(i, f, j) in &collapsing {
            let p = confidence(i, f);
            priority[i] = priority[i].min(p);
            priority[j] = priority[j].min(p);
        }
        for &(i, f, j) in &collapsing {
            let p = confidence(i, f);
            if p <= priority[i] && p <= priority[j] {
                joins[i] ^= 1 << f;
                joins[j] ^= 1 << (f ^ 1);
            }
        }
    }
    (joins, collapsing.len())
}

/// [`resolve_joins`] for a caller that knows its cells by lattice coordinate
/// rather than by index: add every cell that is not a single clean patch,
/// resolve, then ask for each one's joins.
#[derive(Default)]
pub struct AmbiguityResolver {
    cells: Vec<[i32; 3]>,
    masks: Vec<u8>,
    values: Vec<[f32; 8]>,
    joins: Vec<u8>,
    /// Cell numbers ordered by coordinate, for lookup.
    order: Vec<u32>,
    residual: usize,
}

impl AmbiguityResolver {
    pub fn new() -> AmbiguityResolver {
        AmbiguityResolver::default()
    }

    pub fn clear(&mut self) {
        self.cells.clear();
        self.masks.clear();
        self.values.clear();
        self.joins.clear();
        self.order.clear();
        self.residual = 0;
    }

    /// Adds a cell. Adding the same cell twice keeps the first.
    pub fn add(&mut self, cell: [i32; 3], mask: u8, corner_values: [f32; 8]) {
        self.cells.push(cell);
        self.masks.push(mask);
        self.values.push(corner_values);
    }

    pub fn cell_count(&self) -> usize {
        self.order.len()
    }

    /// Faces still collapsing when the last round ran out; 0 when resolution
    /// converged.
    pub fn residual_collapses(&self) -> usize {
        self.residual
    }

    fn find(&self, cell: [i32; 3]) -> Option<usize> {
        let at = self.order.binary_search_by(|&c| self.cells[c as usize].cmp(&cell)).ok()?;
        Some(self.order[at] as usize)
    }

    pub fn resolve(&mut self) {
        let mut order: Vec<u32> = (0..self.cells.len() as u32).collect();
        order.sort_by_key(|&c| (self.cells[c as usize], c));
        order.dedup_by_key(|c| self.cells[*c as usize]);
        self.order = order;

        let neighbours: Vec<[u32; 6]> = (0..self.cells.len())
            .map(|i| {
                std::array::from_fn(|f| {
                    let mut at = self.cells[i];
                    at[face_axis(f)] += face_step(f);
                    self.find(at).map_or(NO_CELL, |j| j as u32)
                })
            })
            .collect();
        (self.joins, self.residual) = resolve_joins(&self.masks, &self.values, &neighbours);
    }

    /// The join-inside bits [`Self::resolve`] settled on for a cell (0 for a
    /// cell never added).
    pub fn joins_for(&self, cell: [i32; 3]) -> u8 {
        self.find(cell).map_or(0, |i| self.joins[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_corner_and_flat_cells_are_simple() {
        assert!(is_simple(0b0000_0001));
        assert!(is_simple(0b0000_1111));
        assert!(is_simple(0b0101_0101));
        assert!(!is_simple(0) && !is_simple(255));
        // Two corners across a face diagonal: an ambiguous face.
        assert!(!is_simple(0b0000_1001));
        assert_eq!(ambiguous_faces(0b0000_1001), 1 << 4);
        // Two corners across the cube's long diagonal: two patches, no
        // ambiguous face.
        assert!(!is_simple(0b1000_0001));
        assert_eq!(ambiguous_faces(0b1000_0001), 0);
        assert_eq!(components(0b1000_0001, 0).0, 2);
    }

    #[test]
    fn every_crossing_edge_gets_a_patch_and_every_patch_an_edge() {
        for mask in 1..255u8 {
            for joins in 0..64u8 {
                let (count, patches) = components(mask, joins);
                assert!((1..=4).contains(&count), "mask {mask:#b}");
                let mut seen = [false; 4];
                for e in (0..12).filter(|&e| edge_crosses(mask, e)) {
                    seen[edge_patch(patches, e)] = true;
                }
                assert!(seen[..count].iter().all(|&s| s) && seen[count..].iter().all(|&s| !s));
            }
        }
    }

    #[test]
    fn the_inside_join_decides_how_many_patches_an_ambiguous_face_makes() {
        let mask = 0b0000_1001;
        assert_eq!(components(mask, 0).0, 2);
        assert_eq!(components(mask, 1 << 4).0, 1);
    }

    #[test]
    fn both_cells_sharing_a_face_decide_it_alike() {
        // Every pair of cells stacked along x that agree on the shared face.
        let values = [0.3, -0.7, 0.5, -0.2];
        for a in 0..=255u8 {
            if ambiguous_faces(a) & 2 == 0 {
                continue;
            }
            for b in 0..=255u8 {
                let shared = FACE_CORNERS[1].iter().zip(FACE_CORNERS[0]).all(|(&ca, cb)| bit(a, ca) == bit(b, cb));
                if !shared {
                    continue;
                }
                let v = if bit(a, FACE_CORNERS[1][0]) { values.map(|v: f32| -v) } else { values };
                assert_eq!(joins_inside(1, a, Some(b), v), joins_inside(0, b, Some(a), v));
            }
        }
    }

    #[test]
    fn flips_are_their_own_inverse_and_keep_edges_edges() {
        for flips in 0..8 {
            for e in 0..12 {
                assert_eq!(flip_edge(flips, flip_edge(flips, e)), e);
                assert_eq!(flip_edge(flips, e) / 4, e / 4);
            }
            for mask in 0..=255u8 {
                assert_eq!(flip_mask(flip_mask(mask, flips), flips), mask);
                assert_eq!(is_simple(mask), is_simple(flip_mask(mask, flips)));
            }
        }
    }

    #[test]
    fn the_resolver_finds_its_cells_by_coordinate() {
        let mut r = AmbiguityResolver::new();
        let values = [-0.5, 0.2, 0.2, -0.5, 1.0, 1.0, 1.0, 1.0];
        r.add([3, -2, 7], 0b0000_1001, values);
        r.add([3, -2, 7], 0b0000_1001, values);
        r.resolve();
        assert_eq!(r.cell_count(), 1);
        assert_eq!(r.residual_collapses(), 0);
        // Inside product 0.25 beats outside 0.04: the inside diagonal joins.
        assert_eq!(r.joins_for([3, -2, 7]), 1 << 4);
        assert_eq!(r.joins_for([0, 0, 0]), 0);
    }
}
