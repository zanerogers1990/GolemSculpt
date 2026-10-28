//! Turns "inside or outside at every lattice sample, and where the surface
//! crosses each lattice edge" into a closed triangle mesh.
//!
//! Dual contouring in the Surface Nets layout: one vertex for each patch of
//! surface passing through a lattice cell ([`crate::topology`]), placed on
//! the planes its edge crossings describe ([`crate::qef`]), and one quad
//! around every lattice edge whose two ends differ. The result is closed and
//! 2-manifold by construction: the samples on the lattice's border are
//! outside, every sign change therefore lies between four whole cells, and
//! each of those has a vertex for it.
//!
//! Where the signs and crossings come from is the caller's business
//! ([`CellSource`]): a scan-converted mesh, several combined, or a sampled
//! field. Only cells the surface passes through are ever stored, so memory
//! follows surface area.
//!
//! Exact symmetry: when a mirror axis is asked for, the result is exactly
//! symmetric across that coordinate plane whatever the source is like. The
//! source is read on its positive side only and reflected (see `mirror.rs`),
//! so the positive half wins: what is there appears on both sides, and what
//! was on the negative side alone is gone. Every cell then has a twin with
//! the mirror-image sign pattern, but twins solved separately still differ in
//! the last bits, and mirrored brushes reproduce whatever difference they
//! start from. So every vertex is replaced by the average of itself and the
//! reflection of its twin, which comes out bit for bit a reflection from
//! either side, and quads are split along diagonals that are themselves
//! mirror images.

use std::sync::atomic::{AtomicUsize, Ordering};

use glam::Vec3;
use rayon::prelude::*;

use crate::grid::Grid;
use crate::mirror::Mirrored;
use crate::parallel::{self, Slots};
use crate::qef;
use crate::topology::{self, EDGE_A, EDGE_B, NO_CELL};

/// Where the surface crosses a cell's edges. Only the entries of edges whose
/// two ends differ mean anything.
#[derive(Clone, Copy)]
pub struct Hermite {
    /// How far along the edge the crossing is, 0 at the edge's first corner
    /// ([`EDGE_A`]) to 1 at its second.
    pub t: [f32; 12],
    /// Unit normal of the surface at the crossing, pointing out of the solid,
    /// or zero when it is not known.
    pub normal: [Vec3; 12],
}

impl Default for Hermite {
    fn default() -> Hermite {
        Hermite { t: [0.5; 12], normal: [Vec3::ZERO; 12] }
    }
}

/// What the extraction reads the solid through.
pub trait CellSource: Sync {
    /// Writes the inside bits of the samples in row (j, k), bit `i` for
    /// sample `i` along x, and returns whether any is set. `out` holds one
    /// bit per sample; `scratch` is spare room of the caller's that survives
    /// between calls. Samples on the lattice's border must be outside.
    fn row(&self, j: usize, k: usize, out: &mut [u64], scratch: &mut Vec<u64>) -> bool;

    /// Fills in the crossing on every edge of `cell` whose ends differ
    /// according to `mask`.
    fn hermite(&self, cell: [usize; 3], mask: u8, out: &mut Hermite);

    /// A distance-like value at each corner of `cell`, negative inside. Only
    /// asked for the rare cells that are not one clean patch, to choose
    /// between the two ways of joining an ambiguous face. Must depend on the
    /// sample alone, not on which of its eight cells is asking.
    fn corner_values(&self, cell: [usize; 3], mask: u8) -> [f32; 8];
}

#[derive(Clone, Copy, Debug)]
pub struct ExtractOptions {
    /// The coordinate planes (x = 0, y = 0, z = 0) the result must be exactly
    /// symmetric across. It always is, whatever the source: the half on the
    /// positive side of each plane is kept and reflected onto the other.
    pub mirror: [bool; 3],
    /// Refuse to build more vertices than this.
    pub max_vertices: usize,
    /// How aligned a cell's normals must be for its vertex to be placed as
    /// a smooth surface rather than solved for a crease (see
    /// [`qef::solve_with`]): the sharpest, [`qef::PLANAR_COSINE`], unless
    /// asked otherwise.
    pub planar_cosine: f32,
}

impl Default for ExtractOptions {
    fn default() -> ExtractOptions {
        ExtractOptions { mirror: [false; 3], max_vertices: 60_000_000, planar_cosine: qef::PLANAR_COSINE }
    }
}

/// Counts worth reporting or testing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Cells the surface passes through.
    pub cells: usize,
    /// Of those, the ones that are not a single clean patch.
    pub crease_cells: usize,
    /// Ambiguous faces the resolver could not settle. Expected 0.
    pub residual_collapses: usize,
    /// Quads dropped because a cell around a sign change had no vertex.
    /// Expected 0; anything else leaves a hole.
    pub skipped_quads: usize,
    /// Cells made an exact mirror image of their twin, summed over the
    /// requested axes, and cells that could not be because the twin's sign
    /// pattern was not the mirror image. The second is always 0: the
    /// negative half is read from the positive, so every cell has its twin.
    pub mirrored_cells: usize,
    pub mirror_skipped: usize,
    /// Of a boolean: times a lattice edge was read that more surfaces cross
    /// than the quick way has room for, and that was read the slower way.
    /// Correct either way; counted to show how rare it is.
    pub long_edges: usize,
}

/// An extracted surface: a closed mesh wound counter-clockwise seen from
/// outside, with a normal per vertex taken from the source rather than from
/// the new triangles.
#[derive(Clone, Debug, Default)]
pub struct Surface {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub indices: Vec<u32>,
    pub stats: Stats,
}

impl Surface {
    /// The triangles alone, for callers with no use for the normals.
    pub fn into_mesh(self) -> sculpt_geom::TriMesh {
        sculpt_geom::TriMesh::new(self.positions, self.indices)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Cell {
    pub i: u16,
    pub j: u16,
    pub k: u16,
    pub mask: u8,
}

impl Cell {
    #[inline]
    pub(crate) fn at(&self) -> [usize; 3] {
        [self.i as usize, self.j as usize, self.k as usize]
    }
}

/// Every cell the surface passes through, in order of (k, j, i), with an
/// index of where each row of cells starts.
pub(crate) struct Cells {
    rows: [usize; 2],
    row_start: Vec<u32>,
    pub cells: Vec<Cell>,
}

impl Cells {
    /// The number of the cell at (i, j, k), if the surface passes through it.
    #[inline]
    pub(crate) fn find(&self, i: i64, j: i64, k: i64) -> Option<usize> {
        if i < 0 || j < 0 || k < 0 || j >= self.rows[0] as i64 || k >= self.rows[1] as i64 {
            return None;
        }
        let row = j as usize + self.rows[0] * k as usize;
        let (from, to) = (self.row_start[row] as usize, self.row_start[row + 1] as usize);
        let at = self.cells[from..to].partition_point(|c| (c.i as i64) < i);
        (from + at < to && self.cells[from + at].i as i64 == i).then_some(from + at)
    }
}

/// The four sample rows around one row of cells, kept while walking a slab.
struct RowWindow {
    /// Rows (j, k), (j, k + 1), (j + 1, k), (j + 1, k + 1).
    rows: [Vec<u64>; 4],
    any: [bool; 4],
    scratch: Vec<u64>,
}

impl RowWindow {
    fn new(words: usize) -> RowWindow {
        RowWindow { rows: std::array::from_fn(|_| vec![0; words]), any: [false; 4], scratch: Vec::new() }
    }

    /// Slides one row up in j: the upper pair becomes the lower pair and the
    /// new upper pair is read from the source.
    fn advance<S: CellSource>(&mut self, source: &S, j: usize, k: usize) {
        self.rows.swap(0, 2);
        self.rows.swap(1, 3);
        self.any.swap(0, 2);
        self.any.swap(1, 3);
        for (slot, dk) in [(2, 0), (3, 1)] {
            self.any[slot] = source.row(j, k + dk, &mut self.rows[slot], &mut self.scratch);
        }
    }
}

/// Finds the cells of slab `k` whose corners are not all alike, 64 at a time.
fn slab_cells<S: CellSource>(source: &S, window: &mut RowWindow, n: [usize; 3], k: usize) -> (Vec<u32>, Vec<Cell>) {
    let [nx, ny, _] = n;
    let mut counts = vec![0u32; ny - 1];
    let mut cells = Vec::new();
    window.advance(source, 0, k);
    for j in 0..ny - 1 {
        window.advance(source, j + 1, k);
        if !window.any.iter().any(|&a| a) {
            continue;
        }
        let before = cells.len();
        let r = &window.rows;
        let word = |w: usize| {
            let get = |row: &Vec<u64>| row.get(w).copied().unwrap_or(0);
            let v = [get(&r[0]), get(&r[1]), get(&r[2]), get(&r[3])];
            (v[0] | v[1] | v[2] | v[3], v[0] & v[1] & v[2] & v[3])
        };
        for w in 0..r[0].len() {
            // A cell is mixed unless all eight corners agree.
            let ((any, all), (any_next, all_next)) = (word(w), word(w + 1));
            let any_up = any >> 1 | any_next << 63;
            let all_up = all >> 1 | all_next << 63;
            let mut mixed = (any | any_up) & !(all & all_up);
            while mixed != 0 {
                let i = (w << 6) + mixed.trailing_zeros() as usize;
                mixed &= mixed - 1;
                if i + 1 >= nx {
                    break;
                }
                let pair = |row: &Vec<u64>| {
                    let at = |i: usize| (row[i >> 6] >> (i & 63) & 1) as u8;
                    at(i) | at(i + 1) << 1
                };
                let mask = pair(&r[0]) | pair(&r[2]) << 2 | pair(&r[1]) << 4 | pair(&r[3]) << 6;
                cells.push(Cell { i: i as u16, j: j as u16, k: k as u16, mask });
            }
        }
        counts[j] = (cells.len() - before) as u32;
    }
    (counts, cells)
}

pub(crate) fn find_cells<S: CellSource>(source: &S, grid: &Grid) -> Cells {
    let n = grid.n;
    let rows = [n[1] - 1, n[2] - 1];
    let words = n[0].div_ceil(64);
    let slabs: Vec<(Vec<u32>, Vec<Cell>)> = (0..rows[1])
        .into_par_iter()
        .map_init(|| RowWindow::new(words), |window, k| slab_cells(source, window, n, k))
        .collect();

    let mut row_start = Vec::with_capacity(rows[0] * rows[1] + 1);
    let mut total = 0u32;
    for (counts, _) in &slabs {
        for &count in counts {
            row_start.push(total);
            total += count;
        }
    }
    row_start.push(total);
    let cells = parallel::concat(slabs.into_iter().map(|(_, cells)| cells).collect());
    Cells { rows, row_start, cells }
}

/// Cells per unit of parallel work.
const CHUNK: usize = 4096;

/// The cells of chunk `c`.
#[inline]
fn chunk(c: usize, cells: usize) -> std::ops::Range<usize> {
    (c * CHUNK).clamp(0, cells)..((c + 1) * CHUNK).clamp(0, cells)
}

/// The cells that are not one clean patch, and how each was divided.
struct Creases {
    /// Their cell numbers, ascending.
    cells: Vec<u32>,
    /// Which patch each edge of the cell belongs to (see
    /// [`topology::components`]).
    edge_patches: Vec<u32>,
    residual: usize,
}

impl Creases {
    #[inline]
    fn patches_of(&self, cell: usize) -> u32 {
        let at = self.cells.binary_search(&(cell as u32)).expect("a cell that is not simple is a crease cell");
        self.edge_patches[at]
    }
}

fn resolve_creases<S: CellSource>(source: &S, cells: &Cells) -> Creases {
    let list: Vec<u32> = cells
        .cells
        .par_iter()
        .enumerate()
        .filter(|(_, c)| !topology::is_simple(c.mask))
        .map(|(i, _)| i as u32)
        .collect();
    if list.is_empty() {
        return Creases { cells: list, edge_patches: Vec::new(), residual: 0 };
    }
    let masks: Vec<u8> = list.iter().map(|&c| cells.cells[c as usize].mask).collect();
    let values: Vec<[f32; 8]> = list
        .par_iter()
        .map(|&c| {
            let cell = cells.cells[c as usize];
            source.corner_values(cell.at(), cell.mask)
        })
        .collect();
    let neighbours: Vec<[u32; 6]> = list
        .par_iter()
        .map(|&c| {
            let cell = cells.cells[c as usize];
            std::array::from_fn(|f| {
                if topology::ambiguous_faces(cell.mask) >> f & 1 == 0 {
                    return NO_CELL;
                }
                let mut at = cell.at().map(|v| v as i64);
                at[topology::face_axis(f)] += topology::face_step(f) as i64;
                let found = cells.find(at[0], at[1], at[2]).and_then(|n| list.binary_search(&(n as u32)).ok());
                found.map_or(NO_CELL, |n| n as u32)
            })
        })
        .collect();
    let (joins, residual) = topology::resolve_joins(&masks, &values, &neighbours);
    let edge_patches = masks.iter().zip(&joins).map(|(&mask, &joins)| topology::components(mask, joins).1).collect();
    Creases { cells: list, edge_patches, residual }
}

/// Where each cell's vertices start: one per patch, numbered consecutively.
fn vertex_bases(cells: &Cells, creases: &Creases) -> Vec<u32> {
    let patch_count = |cell: u32, patches: u32| {
        let mask = cells.cells[cell as usize].mask;
        let crossing = (0..12).filter(|&e| topology::edge_crosses(mask, e));
        crossing.map(|e| topology::edge_patch(patches, e) as u32).max().map_or(1, |last| last + 1)
    };
    // Every cell has one vertex, and a crease cell one more for each patch
    // past its first.
    let mut extra = 0u32;
    let mut next = creases.cells.iter().zip(&creases.edge_patches).peekable();
    let mut base = Vec::with_capacity(cells.cells.len() + 1);
    for cell in 0..cells.cells.len() as u32 {
        base.push(cell + extra);
        if let Some((_, &patches)) = next.next_if(|(c, _)| **c == cell) {
            extra += patch_count(cell, patches) - 1;
        }
    }
    base.push(cells.cells.len() as u32 + extra);
    base
}

/// The corner offsets of a cell, as steps along each axis.
#[inline]
fn corner(c: usize) -> Vec3 {
    Vec3::new((c & 1) as f32, (c >> 1 & 1) as f32, (c >> 2 & 1) as f32)
}

/// Solves one patch of a cell (or the whole cell when `patch` is `None`):
/// position, normal and feature rank. `planar_cosine` is
/// [`ExtractOptions::planar_cosine`].
fn solve_patch(
    grid: &Grid,
    cell: Cell,
    hermite: &Hermite,
    patch: Option<(u32, usize)>,
    planar_cosine: f32,
) -> (Vec3, Vec3, u8) {
    let mut points = [Vec3::ZERO; 12];
    let mut normals = [Vec3::ZERO; 12];
    let mut count = 0;
    for e in 0..12 {
        if !topology::edge_crosses(cell.mask, e) {
            continue;
        }
        if patch.is_some_and(|(patches, p)| topology::edge_patch(patches, e) != p) {
            continue;
        }
        points[count] = corner(EDGE_A[e]).lerp(corner(EDGE_B[e]), hermite.t[e]);
        normals[count] = hermite.normal[e];
        count += 1;
    }
    let (local, rank) = qef::solve_with(&points[..count], &normals[..count], planar_cosine);
    let [i, j, k] = cell.at();
    let normal = normals[..count].iter().copied().sum::<Vec3>();
    let normal = normal.try_normalize().unwrap_or_else(|| way_out(cell.mask));
    (grid.position(i, j, k) + local * grid.cell, normal, rank)
}

/// The way out of the solid as a cell's own corners show it: from its inside
/// corners towards its outside ones. The normal of last resort, for a vertex
/// whose crossings brought none, as on the cap closing over a hole, where
/// the source has no surface to take one from.
fn way_out(mask: u8) -> Vec3 {
    let towards = |c: usize| (corner(c) - 0.5) * if mask >> c & 1 != 0 { -1.0 } else { 1.0 };
    (0..8).map(towards).sum::<Vec3>().normalize_or(Vec3::Y)
}

/// The per-vertex arrays the stages after cell finding fill in and rewrite.
struct Vertices {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    /// Feature rank from the solve: 2 on a crease, 3 at a corner.
    ranks: Vec<u8>,
}

/// Everything the stages after cell finding share.
struct Lattice<'a> {
    grid: &'a Grid,
    cells: &'a Cells,
    creases: &'a Creases,
    /// The first vertex of each cell, and one past the last cell's.
    base: &'a [u32],
}

impl Lattice<'_> {
    fn cell_count(&self) -> usize {
        self.cells.cells.len()
    }

    fn chunk_count(&self) -> usize {
        self.cell_count().div_ceil(CHUNK)
    }

    /// Where each chunk of cells' vertices end.
    fn vertex_ends(&self) -> Vec<usize> {
        (0..self.chunk_count()).map(|c| self.base[chunk(c, self.cell_count()).end] as usize).collect()
    }

    /// The vertices of cell `ci`.
    #[inline]
    fn vertices_of(&self, ci: usize) -> std::ops::Range<usize> {
        self.base[ci] as usize..self.base[ci + 1] as usize
    }

    /// The vertex cell `ci` contributes to the quad around its edge `edge`:
    /// its only vertex, or for a crease cell that of the patch owning the edge.
    #[inline]
    fn vertex_at(&self, at: [i64; 3], edge: usize) -> Option<u32> {
        let ci = self.cells.find(at[0], at[1], at[2])?;
        let mask = self.cells.cells[ci].mask;
        let patch = if topology::is_simple(mask) { 0 } else { topology::edge_patch(self.creases.patches_of(ci), edge) };
        Some(self.base[ci] + patch as u32)
    }
}

fn solve_vertices<S: CellSource>(source: &S, lattice: &Lattice, planar_cosine: f32) -> Vertices {
    let cells = &lattice.cells.cells;
    let (positions, normals, ranks, _) = parallel::build3(&lattice.vertex_ends(), |c, positions, normals, ranks| {
        let mut hermite = Hermite::default();
        for ci in chunk(c, cells.len()) {
            let cell = cells[ci];
            source.hermite(cell.at(), cell.mask, &mut hermite);
            let simple = topology::is_simple(cell.mask);
            let patches = if simple { 0 } else { lattice.creases.patches_of(ci) };
            for p in 0..lattice.vertices_of(ci).len() {
                let patch = (!simple).then_some((patches, p));
                let (position, normal, rank) = solve_patch(lattice.grid, cell, &hermite, patch, planar_cosine);
                positions.push(position);
                normals.push(normal);
                ranks.push(rank);
            }
        }
    });
    Vertices { positions, normals, ranks }
}

/// The midpoint of `a` and the reflection of `b` across the plane of `axis`.
/// Asked the other way round it gives the exact reflection of this answer.
#[inline]
fn mirror_average(a: Vec3, b: Vec3, axis: usize) -> Vec3 {
    let mut mid = (a + b) * 0.5;
    mid[axis] = (a[axis] - b[axis]) * 0.5;
    mid
}

/// Which patch of the twin each patch of a multi-patch cell mirrors onto,
/// worked out from the edges they own: edge `e`'s patch here must be the
/// mirrored edge's patch there, for every edge the surface crosses. `None`
/// when that is not a consistent one-to-one match (the two cells' ambiguous
/// faces were decided differently).
fn mirror_patch_map(mask: u8, here: u32, there: u32, axis: usize) -> Option<[usize; 4]> {
    const UNSET: usize = usize::MAX;
    let (mut map, mut back) = ([UNSET; 4], [UNSET; 4]);
    for e in (0..12).filter(|&e| topology::edge_crosses(mask, e)) {
        let p = topology::edge_patch(here, e);
        let q = topology::edge_patch(there, topology::flip_edge(1 << axis, e));
        if map[p] == UNSET && back[q] == UNSET {
            map[p] = q;
            back[q] = p;
        } else if map[p] != q || back[q] != p {
            return None;
        }
    }
    Some(map)
}

/// The cell `ci` mirrors onto across the coordinate plane of `axis` and
/// which of its patches each of `ci`'s lands on, if the two really are
/// mirror images of one another.
fn mirror_twin(lattice: &Lattice, ci: usize, axis: usize) -> Option<(usize, [usize; 4])> {
    let cell = lattice.cells.cells[ci];
    let mut at = cell.at().map(|v| v as i64);
    at[axis] = 2 * lattice.grid.plane_cell(axis) - at[axis];
    let ti = lattice.cells.find(at[0], at[1], at[2])?;
    if lattice.cells.cells[ti].mask != topology::flip_mask(cell.mask, 1 << axis) {
        return None;
    }
    if topology::is_simple(cell.mask) {
        return Some((ti, [0; 4]));
    }
    let patches = &lattice.creases;
    Some((ti, mirror_patch_map(cell.mask, patches.patches_of(ci), patches.patches_of(ti), axis)?))
}

/// Makes the vertices exactly mirror-symmetric across the coordinate plane of
/// `axis`, wherever the two halves of the lattice agree on the topology.
/// Returns the new vertices, how many cells were mirrored and how many had to
/// be left.
///
/// Every vertex's new value is a function of the old values of itself and its
/// twin only, the same function seen from either side, so the cells can be
/// worked in any order and in parallel.
fn mirror_vertices(lattice: &Lattice, axis: usize, old: &Vertices) -> (Vertices, usize, usize) {
    let (positions, normals, ranks, counts) = parallel::build3(&lattice.vertex_ends(), |c, positions, normals, ranks| {
        let (mut mirrored, mut skipped) = (0usize, 0usize);
        for ci in chunk(c, lattice.cell_count()) {
            let own = lattice.vertices_of(ci);
            let Some((ti, map)) = mirror_twin(lattice, ci, axis) else {
                positions.extend(&old.positions[own.clone()]);
                normals.extend(&old.normals[own.clone()]);
                ranks.extend(&old.ranks[own]);
                skipped += 1;
                continue;
            };
            for (p, v) in own.enumerate() {
                let w = lattice.base[ti] as usize + map[p];
                let (mut position, mut normal) = (old.positions[v], old.normals[v]);
                if v == w {
                    // Its own mirror image: exactly onto the plane.
                    position[axis] = 0.0;
                    normal[axis] = 0.0;
                } else {
                    position = mirror_average(position, old.positions[w], axis);
                    normal = mirror_average(normal, old.normals[w], axis);
                }
                positions.push(position);
                normals.push(normal.try_normalize().unwrap_or(old.normals[v]));
                ranks.push(old.ranks[v].max(old.ranks[w]));
            }
            mirrored += 1;
        }
        (mirrored, skipped)
    });
    let (mirrored, skipped) = counts.into_iter().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    (Vertices { positions, normals, ranks }, mirrored, skipped)
}

/// Which diagonal to split a quad along: true for corners 1-3, false for 0-2.
/// `v` are the quad's corners in order round it, `sharp` whether each sits on
/// a crease or corner, and `parity` the choice for a quad with nothing sharp
/// about it. In order:
///
///  1. Fewer all-crease triangles. Three crease vertices make a fin standing
///     on the crease; the other split puts both triangles flat on the faces.
///  2. Nothing sharp at all, which is most quads: `parity`, the regular
///     pattern.
///  3. Never a degenerate triangle (three crease vertices in a row are
///     collinear).
///  4. Join two crease vertices across the quad: the crease runs that way.
///  5. Otherwise the shorter diagonal, and `parity` when they are equal.
///
/// The inputs are flags and squared lengths, which come out the same for a
/// quad and its mirror image, so the choice is a mirror image too, provided
/// `parity` is.
pub fn choose_diagonal(v: [Vec3; 4], sharp: [bool; 4], parity: bool) -> bool {
    let [s0, s1, s2, s3] = sharp;
    let fins02 = (s0 && s1 && s2) as u8 + (s0 && s2 && s3) as u8;
    let fins13 = (s0 && s1 && s3) as u8 + (s1 && s2 && s3) as u8;
    if fins02 != fins13 {
        return fins02 > fins13;
    }
    if !(s0 || s1 || s2 || s3) {
        return parity;
    }
    let d = |a: usize, b: usize| v[a].distance_squared(v[b]);
    let (e01, e12, e23, e30, l02, l13) = (d(0, 1), d(1, 2), d(2, 3), d(3, 0), d(0, 2), d(1, 3));
    let flat02 = degenerate(e01, e12, l02) || degenerate(e23, e30, l02);
    let flat13 = degenerate(e01, e30, l13) || degenerate(e12, e23, l13);
    if flat02 != flat13 {
        return flat02;
    }
    if (s0 && s2) != (s1 && s3) {
        return s1 && s3;
    }
    if l02 != l13 { l13 < l02 } else { parity }
}

/// A triangle whose area is a negligible fraction of its longest edge
/// squared, from its squared edge lengths (Heron, sorted so the answer does
/// not depend on their order).
fn degenerate(a: f32, b: f32, c: f32) -> bool {
    let mut s = [a, b, c];
    s.sort_unstable_by(f32::total_cmp);
    let [a, b, c] = s;
    2.0 * (a * b + b * c + c * a) - (a * a + b * b + c * c) < 1e-4 * c * c
}

/// Stands in for the corners of a quad that had to be dropped, until they are
/// swept out.
const DROPPED: u32 = u32::MAX;

/// The lattice edges cell owns that the solid changes across: the three
/// leaving its lowest corner, less any on the lattice's lower faces.
///
/// A lattice edge is shared by four cells, and the one at the upper end in
/// both cross directions is its single owner. So walking the cells and
/// testing these three edges reaches every quad exactly once.
#[inline]
fn owned_edges(cell: Cell) -> impl Iterator<Item = usize> {
    let at = cell.at();
    let inside = cell.mask & 1 != 0;
    (0..3).filter(move |&axis| {
        let crosses = (cell.mask >> (1 << axis) & 1 != 0) != inside;
        crosses && at[(axis + 1) % 3] >= 1 && at[(axis + 2) % 3] >= 1
    })
}

/// Writes the quads owned by chunk `c` of the cells, as triangles. Returns
/// how many had to be dropped.
///
/// Each of the four cells round an edge gives the vertex of the patch that
/// owns that edge, which is why every corner names the edge's number within
/// its cell.
///
/// A plain quad is split along the diagonal that makes the triangulation its
/// own mirror image across every coordinate plane. A fixed diagonal is not:
/// reflecting a quad whose cells step across the mirror axis swaps its
/// diagonals, so a fixed choice gives every such quad the opposite split from
/// its twin, every smoothing and normal pass then treats the two halves
/// differently, and mirrored strokes drift apart. The rule: the (low, low) to
/// (high, high) diagonal when the quad lies on the same side of both cross
/// planes, the other when it does not.
fn emit_quads(c: usize, lattice: &Lattice, vertices: &Vertices, out: &mut Slots<u32>) -> usize {
    let plane = [0, 1, 2].map(|axis| lattice.grid.plane_cell(axis));
    let mut dropped = 0;
    for ci in chunk(c, lattice.cell_count()) {
        let cell = lattice.cells.cells[ci];
        let at = cell.at().map(|v| v as i64);
        let inside = cell.mask & 1 != 0;
        let above = [at[0] > plane[0], at[1] > plane[1], at[2] > plane[2]];
        for axis in owned_edges(cell) {
            // The two cross axes, in the order that makes (axis, a, b)
            // right-handed: walking the four cells low-low, high-low,
            // high-high, low-high then runs counter-clockwise seen from up
            // the axis.
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let step = |da: i64, db: i64| {
                let mut c = at;
                c[a] -= da;
                c[b] -= db;
                // Seen from a cell stepped back, the edge is at its upper
                // end. The four edges along an axis are numbered by their
                // offset on the lower cross axis plus twice that on the higher.
                let within = if a < b { da + 2 * db } else { db + 2 * da };
                lattice.vertex_at(c, axis * 4 + within as usize)
            };
            let (Some(i0), Some(i1), Some(i2), Some(i3)) = (step(1, 1), step(0, 1), step(0, 0), step(1, 0)) else {
                out.extend(&[DROPPED; 6]);
                dropped += 1;
                continue;
            };
            let ids = [i0, i1, i2, i3];
            let anti = choose_diagonal(
                ids.map(|i| vertices.positions[i as usize]),
                ids.map(|i| vertices.ranks[i as usize] >= 2),
                above[a] != above[b],
            );
            // Counter-clockwise seen from up the axis faces up it, which is
            // outward when the lower end of the edge is the solid one.
            let order = match (anti, inside) {
                (false, true) => [0, 1, 2, 0, 2, 3],
                (false, false) => [0, 2, 1, 0, 3, 2],
                (true, true) => [0, 1, 3, 1, 2, 3],
                (true, false) => [0, 3, 1, 1, 3, 2],
            };
            out.extend(&order.map(|c| ids[c]));
        }
    }
    dropped
}

/// The triangles of every quad, and how many quads were dropped.
fn build_indices(lattice: &Lattice, vertices: &Vertices) -> (Vec<u32>, usize) {
    let cells = &lattice.cells.cells;
    let counts: Vec<usize> = (0..lattice.chunk_count())
        .into_par_iter()
        .map(|c| chunk(c, cells.len()).map(|ci| owned_edges(cells[ci]).count() * 6).sum())
        .collect();
    let mut end = 0;
    let ends: Vec<usize> = counts
        .iter()
        .map(|count| {
            end += count;
            end
        })
        .collect();
    let dropped = AtomicUsize::new(0);
    let mut indices = parallel::build(&ends, |c, out| {
        let lost = emit_quads(c, lattice, vertices, out);
        if lost > 0 {
            dropped.fetch_add(lost, Ordering::Relaxed);
        }
    });
    let dropped = dropped.into_inner();
    if dropped > 0 {
        indices.retain(|&i| i != DROPPED);
    }
    (indices, dropped)
}

/// Extracts the surface of `source` on `grid`. With mirror axes, the surface
/// of the source's positive half and its reflection (see [`ExtractOptions`]).
pub fn extract<S: CellSource>(source: &S, grid: &Grid, options: &ExtractOptions) -> Result<Surface, String> {
    if options.mirror.contains(&true) {
        let (mirrored, wide) = Mirrored::new(source, grid, options.mirror);
        return extract_as_is(&mirrored, &wide, options);
    }
    extract_as_is(source, grid, options)
}

fn extract_as_is<S: CellSource>(source: &S, grid: &Grid, options: &ExtractOptions) -> Result<Surface, String> {
    if grid.n.iter().any(|&n| n < 2) {
        return Err("The grid is too small to hold a surface.".into());
    }
    // Cells are numbered in 16 bits a side, and a table is kept with an
    // entry for every row of them (`budget` asks about both beforehand, for
    // those who go through it).
    let rows_too_many = crate::budget::row_table_bytes(grid) > crate::budget::MAX_BYTES;
    if grid.n.iter().any(|&n| n > crate::grid::MAX_SAMPLES) || rows_too_many {
        return Err("That resolution is too fine for this model. Try a lower one.".into());
    }
    let cells = find_cells(source, grid);
    if cells.cells.is_empty() {
        return Err("There is no solid volume to make a surface from. Is the model closed?".into());
    }
    if cells.cells.len() > options.max_vertices {
        return Err("That resolution would make too many triangles. Try a lower one.".into());
    }
    let creases = resolve_creases(source, &cells);
    let base = vertex_bases(&cells, &creases);
    let lattice = Lattice { grid, cells: &cells, creases: &creases, base: &base };
    let mut vertices = solve_vertices(source, &lattice, options.planar_cosine);

    let mut stats = Stats {
        cells: cells.cells.len(),
        crease_cells: creases.cells.len(),
        residual_collapses: creases.residual,
        ..Stats::default()
    };
    for axis in (0..3).filter(|&a| options.mirror[a]) {
        let (mirrored, done, skipped) = mirror_vertices(&lattice, axis, &vertices);
        vertices = mirrored;
        stats.mirrored_cells += done;
        stats.mirror_skipped += skipped;
    }

    let (indices, dropped) = build_indices(&lattice, &vertices);
    stats.skipped_quads = dropped;
    if indices.is_empty() {
        return Err("No surface came out. Try a higher resolution.".into());
    }
    Ok(Surface { positions: vertices.positions, normals: vertices.normals, indices, stats })
}

/// Fills in a cell's crossings from a function that answers for one lattice
/// edge at a time: `edge(axis, sample, lower_inside)` gives the fraction
/// along the edge from `sample` one step up `axis`, and the outward normal.
/// An edge with no recorded crossing is given its midpoint and no normal.
pub(crate) fn hermite_from_edges(
    cell: [usize; 3],
    mask: u8,
    out: &mut Hermite,
    edge: impl Fn(usize, [usize; 3], bool) -> Option<(f32, Vec3)>,
) {
    for e in (0..12).filter(|&e| topology::edge_crosses(mask, e)) {
        let a = EDGE_A[e];
        let at = [cell[0] + (a & 1), cell[1] + (a >> 1 & 1), cell[2] + (a >> 2 & 1)];
        (out.t[e], out.normal[e]) = edge(e / 4, at, mask >> a & 1 != 0).unwrap_or((0.5, Vec3::ZERO));
    }
}

/// Corner values for a source that only knows crossings: at each corner, how
/// far along the lattice lines through it the nearest crossing is, in cells
/// (at most 1), negative inside. `nearest(axis, sample)` gives the fraction
/// along the edge from `sample` one step up `axis` of the crossing on it.
pub(crate) fn values_from_edges(
    grid: &Grid,
    cell: [usize; 3],
    mask: u8,
    nearest: impl Fn(usize, [usize; 3]) -> Option<f32>,
) -> [f32; 8] {
    std::array::from_fn(|c| {
        let at = [cell[0] + (c & 1), cell[1] + (c >> 1 & 1), cell[2] + (c >> 2 & 1)];
        let mut distance = 1.0f32;
        for axis in 0..3 {
            if at[axis] + 1 < grid.n[axis] {
                distance = distance.min(nearest(axis, at).unwrap_or(1.0));
            }
            if at[axis] > 0 {
                let mut below = at;
                below[axis] -= 1;
                distance = distance.min(1.0 - nearest(axis, below).unwrap_or(0.0));
            }
        }
        let distance = distance.clamp(1e-6, 1.0);
        if mask >> c & 1 != 0 { -distance } else { distance }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diagonal_follows_a_crease_and_otherwise_the_parity() {
        let flat = [Vec3::ZERO, Vec3::X, Vec3::X + Vec3::Y, Vec3::Y];
        assert!(choose_diagonal(flat, [false; 4], true));
        assert!(!choose_diagonal(flat, [false; 4], false));
        // A crease from corner 1 to corner 3: split along it, whatever the parity.
        assert!(choose_diagonal(flat, [false, true, false, true], false));
        assert!(!choose_diagonal(flat, [true, false, true, false], true));
    }

    #[test]
    fn the_diagonal_of_a_mirrored_quad_is_the_mirrored_diagonal() {
        let quad =
            [Vec3::new(0.1, 0.0, 0.3), Vec3::new(1.2, 0.1, 0.2), Vec3::new(1.0, 1.3, 0.0), Vec3::new(0.0, 0.9, 0.1)];
        let flip = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
        // Mirrored across the first cross axis, corners 0 and 1 trade places,
        // as do 2 and 3, and the parity flips.
        let mirrored = [flip(quad[1]), flip(quad[0]), flip(quad[3]), flip(quad[2])];
        for bits in 0..16u8 {
            let s = [0, 1, 2, 3].map(|b| bits >> b & 1 != 0);
            let ms = [s[1], s[0], s[3], s[2]];
            for parity in [false, true] {
                assert_eq!(choose_diagonal(quad, s, parity), !choose_diagonal(mirrored, ms, !parity), "{bits:#b}");
            }
        }
    }

    #[test]
    fn the_mirror_average_is_an_exact_reflection_from_either_side() {
        let (a, b) = (Vec3::new(0.31, 0.2, -0.7), Vec3::new(-0.3099999, 0.2000001, -0.70000005));
        let (from_a, from_b) = (mirror_average(a, b, 0), mirror_average(b, a, 0));
        assert_eq!(from_a.x.to_bits(), (-from_b.x).to_bits());
        assert_eq!((from_a.y.to_bits(), from_a.z.to_bits()), (from_b.y.to_bits(), from_b.z.to_bits()));
    }

    #[test]
    fn the_normal_of_last_resort_points_out_of_the_solid() {
        // The lower four corners inside: out is up.
        assert_eq!(way_out(0b0000_1111), Vec3::Z);
        assert_eq!(way_out(0b1111_0000), -Vec3::Z);
        // One corner inside: out is away from it.
        assert!(way_out(1).distance(Vec3::ONE.normalize()) < 1e-6);
        assert!(way_out(0).is_finite() && way_out(255).is_finite());
    }

    #[test]
    fn a_cell_owns_the_crossed_edges_leaving_its_lowest_corner() {
        let cell = |i, j, k, mask| Cell { i, j, k, mask };
        // Corner 0 inside, the rest outside: all three edges cross.
        assert_eq!(owned_edges(cell(3, 3, 3, 1)).collect::<Vec<_>>(), [0, 1, 2]);
        // Corners 0 and 1 inside: the x edge does not cross.
        assert_eq!(owned_edges(cell(3, 3, 3, 0b11)).collect::<Vec<_>>(), [1, 2]);
        // On the lattice's lower faces there is no cell to step back into.
        assert_eq!(owned_edges(cell(3, 0, 3, 1)).collect::<Vec<_>>(), [1]);
        assert_eq!(owned_edges(cell(0, 0, 0, 1)).count(), 0);
    }
}
