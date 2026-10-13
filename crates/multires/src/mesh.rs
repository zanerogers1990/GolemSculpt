//! The faces of one level, and the mesh a level is handed over as.
//!
//! A level's faces come in three forms. A triangle mesh is its triangles. A
//! polygon mesh as the app gave it is its polygon faces and the triangles the
//! app cut them into, in whatever order and along whichever diagonals the app
//! chose; checking that the two describe the same surface is this module's
//! job, because everything that carries per-triangle data between levels
//! stands on it. And every level made here from a polygon mesh is all quads,
//! each cut the same way, so only the quads are kept and the triangles are
//! written out when asked for.

use glam::Vec3;
use rayon::prelude::*;

use crate::refine::{MOST_VERTICES, NONE, PER_TASK, Rows};

/// Polygon faces over a mesh's vertices, in the form the app keeps them:
/// face `f`'s corners are `indices[start[f]..start[f + 1]]`, wound like the
/// triangles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polys {
    pub start: Vec<u32>,
    pub indices: Vec<u32>,
}

impl Polys {
    pub fn face_count(&self) -> usize {
        self.start.len().saturating_sub(1)
    }
}

/// One level as the app sculpts it. `triangles` are the faces to draw and
/// sculpt; `polys`, for a polygon mesh, are the faces those triangles were cut
/// from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelMesh {
    pub positions: Vec<Vec3>,
    pub mask: Vec<f32>,
    pub triangles: Vec<u32>,
    pub polys: Option<Polys>,
}

/// The faces of one level.
#[derive(Clone)]
pub(crate) enum Faces {
    Triangles(Vec<u32>),
    Given(GivenPolys),
    /// Four corners a quad. Quad `q` is drawn as triangles `2q` (its corners
    /// 0, 1, 2) and `2q + 1` (corners 0, 2, 3).
    Quads(Vec<u32>),
}

/// A polygon mesh as the app gave it.
#[derive(Clone)]
pub(crate) struct GivenPolys {
    triangles: Vec<u32>,
    start: Vec<u32>,
    corners: Vec<u32>,
    /// The face each corner belongs to.
    corner_face: Vec<u32>,
    /// For the side of a face that starts at each corner, the one triangle of
    /// that face it is a side of.
    side_triangle: Vec<u32>,
}

impl Faces {
    /// The faces of a mesh handed in from outside, checked.
    pub(crate) fn given(vertex_count: usize, triangles: Vec<u32>, polys: Option<Polys>) -> Result<Faces, String> {
        if vertex_count > MOST_VERTICES || triangles.len() > u32::MAX as usize {
            return Err("The mesh is too large for subdivision levels.".into());
        }
        if !triangles.len().is_multiple_of(3) {
            return Err("The triangle list does not hold whole triangles.".into());
        }
        if triangles.par_iter().with_min_len(PER_TASK).any(|&v| v as usize >= vertex_count) {
            return Err("A triangle uses a vertex the mesh does not have.".into());
        }
        match polys {
            None => Ok(Faces::Triangles(triangles)),
            Some(polys) => Ok(Faces::Given(GivenPolys::checked(vertex_count, triangles, polys)?)),
        }
    }

    pub(crate) fn triangle_count(&self) -> usize {
        match self {
            Faces::Triangles(t) => t.len() / 3,
            Faces::Given(g) => g.triangles.len() / 3,
            Faces::Quads(q) => q.len() / 2,
        }
    }

    pub(crate) fn triangles(&self) -> Vec<u32> {
        match self {
            Faces::Triangles(t) => t.clone(),
            Faces::Given(g) => g.triangles.clone(),
            Faces::Quads(quads) => quad_triangles(quads),
        }
    }

    pub(crate) fn polys(&self) -> Option<Polys> {
        match self {
            Faces::Triangles(_) => None,
            Faces::Given(g) => Some(Polys { start: g.start.clone(), indices: g.corners.clone() }),
            Faces::Quads(quads) => Some(Polys { start: quad_starts(quads.len() / 4), indices: quads.clone() }),
        }
    }

    /// [`Self::triangles`] and [`Self::polys`] of faces no longer needed,
    /// without copying what can be handed over as it is.
    pub(crate) fn into_lists(self) -> (Vec<u32>, Option<Polys>) {
        match self {
            Faces::Triangles(triangles) => (triangles, None),
            Faces::Given(g) => (g.triangles, Some(Polys { start: g.start, indices: g.corners })),
            Faces::Quads(quads) => {
                (quad_triangles(&quads), Some(Polys { start: quad_starts(quads.len() / 4), indices: quads }))
            }
        }
    }

    /// The polygon faces, for a polygon mesh.
    pub(crate) fn poly_ref(&self) -> Option<PolyRef<'_>> {
        match self {
            Faces::Triangles(_) => None,
            Faces::Given(g) => Some(PolyRef { corners: &g.corners, given: Some(g) }),
            Faces::Quads(quads) => Some(PolyRef { corners: quads, given: None }),
        }
    }

    pub(crate) fn bytes(&self) -> usize {
        4 * match self {
            Faces::Triangles(t) => t.len(),
            Faces::Given(g) => {
                g.triangles.len() + g.start.len() + g.corners.len() + g.corner_face.len() + g.side_triangle.len()
            }
            Faces::Quads(q) => q.len(),
        }
    }
}

/// The two triangles of every quad: corners 0, 1, 2 and corners 0, 2, 3.
fn quad_triangles(quads: &[u32]) -> Vec<u32> {
    let mut triangles = vec![0u32; quads.len() / 4 * 6];
    triangles.par_chunks_exact_mut(6).with_min_len(PER_TASK).zip(quads.par_chunks_exact(4)).for_each(|(t, q)| {
        t.copy_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    });
    triangles
}

/// Where each of `count` quads starts in the corner list, and where the last ends.
fn quad_starts(count: usize) -> Vec<u32> {
    (0..count as u32 + 1).into_par_iter().with_min_len(PER_TASK).map(|q| q * 4).collect()
}

impl GivenPolys {
    fn checked(vertex_count: usize, triangles: Vec<u32>, polys: Polys) -> Result<GivenPolys, String> {
        let Polys { start, indices: corners } = polys;
        let whole = start.first() == Some(&0)
            && start.last().map(|&end| end as usize) == Some(corners.len())
            && start.windows(2).all(|w| w[0] <= w[1]);
        if !whole || corners.len() > (u32::MAX / 4) as usize {
            return Err("The polygon faces are not a whole list of faces.".into());
        }
        if corners.iter().any(|&v| v as usize >= vertex_count) {
            return Err("A polygon face uses a vertex the mesh does not have.".into());
        }
        let mut corner_face = vec![0u32; corners.len()];
        for (f, w) in start.windows(2).enumerate() {
            let face = &corners[w[0] as usize..w[1] as usize];
            if face.len() < 3 || face.iter().enumerate().any(|(k, v)| face[..k].contains(v)) {
                return Err("A polygon face needs three or more corners, all different.".into());
            }
            corner_face[w[0] as usize..w[1] as usize].fill(f as u32);
        }
        let mut given = GivenPolys { triangles, start, corners, corner_face, side_triangle: Vec::new() };
        given.side_triangle = given.match_triangles(vertex_count)?;
        Ok(given)
    }

    /// Finds the face each triangle was cut from and, through that, the
    /// triangle on each side of each face.
    ///
    /// The app builds its triangles from the faces and then reorders them for
    /// its spatial index, so nothing can be assumed about their order. What
    /// must hold is that they are the faces cut up: a face of `n` corners is
    /// `n - 2` triangles over its own corners, wound as it is, and each of its
    /// sides is a side of exactly one of them. A mesh whose triangles and faces
    /// have come apart is refused; subdividing it would carry the faces one
    /// way and the triangles another.
    fn match_triangles(&self, vertex_count: usize) -> Result<Vec<u32>, String> {
        const MISMATCH: &str = "The triangles are not the polygon faces cut into triangles.";
        let corners_at = Rows::build(vertex_count, |put| {
            for (corner, &v) in self.corners.iter().enumerate() {
                put(v, corner as u32);
            }
        });
        let mut side_triangle = vec![NONE; self.corners.len()];
        let mut cut_into = vec![0u32; self.start.len() - 1];
        for (t, c) in self.triangles.chunks_exact(3).enumerate() {
            // Two faces can hold the same three corners in the same order (two
            // quads folded over a shared pair of sides); the triangle belongs
            // to the one whose sides are still free.
            let free = |sides: &[Option<usize>; 3]| sides.iter().flatten().all(|&side| side_triangle[side] == NONE);
            let seat = corners_at
                .row(c[0] as usize)
                .iter()
                .filter_map(|&corner| self.seat(corner as usize, c))
                .find(|(_, sides)| free(sides));
            let Some((face, sides)) = seat else { return Err(MISMATCH.into()) };
            cut_into[face] += 1;
            for side in sides.into_iter().flatten() {
                side_triangle[side] = t as u32;
            }
        }
        let every_side = !side_triangle.contains(&NONE);
        let every_face = self.start.windows(2).zip(&cut_into).all(|(w, &cut)| w[1] - w[0] == cut + 2);
        if every_side && every_face { Ok(side_triangle) } else { Err(MISMATCH.into()) }
    }

    /// Whether triangle `c`, whose first corner is face corner `corner`, lies
    /// in that corner's face wound the same way; if so the face, and the face
    /// sides (by the corner each starts at) that are sides of the triangle.
    fn seat(&self, corner: usize, c: &[u32]) -> Option<(usize, [Option<usize>; 3])> {
        let f = self.corner_face[corner] as usize;
        let (from, to) = (self.start[f] as usize, self.start[f + 1] as usize);
        let n = to - from;
        // How many corners round the face from `corner` a vertex is.
        let steps = |v: u32| {
            let at = self.corners[from..to].iter().position(|&x| x == v)?;
            Some((at + n - (corner - from)) % n)
        };
        let (second, third) = (steps(c[1])?, steps(c[2])?);
        if !(0 < second && second < third) {
            return None;
        }
        let slot = |steps: usize| from + (corner - from + steps) % n;
        let sides = [
            (second == 1).then_some(corner),
            (third == second + 1).then(|| slot(second)),
            (third == n - 1).then(|| slot(third)),
        ];
        Some((f, sides))
    }
}

/// Polygon faces as the Catmull-Clark tables read them: the faces a level was
/// given, or the quads of one made here.
#[derive(Clone, Copy)]
pub(crate) struct PolyRef<'a> {
    pub(crate) corners: &'a [u32],
    given: Option<&'a GivenPolys>,
}

impl PolyRef<'_> {
    pub(crate) fn face_count(&self) -> usize {
        match self.given {
            Some(g) => g.start.len() - 1,
            None => self.corners.len() / 4,
        }
    }

    #[inline]
    pub(crate) fn face_of(&self, corner: usize) -> usize {
        match self.given {
            Some(g) => g.corner_face[corner] as usize,
            None => corner / 4,
        }
    }

    /// The first corner of face `f` and one past its last.
    #[inline]
    pub(crate) fn range(&self, f: usize) -> (usize, usize) {
        match self.given {
            Some(g) => (g.start[f] as usize, g.start[f + 1] as usize),
            None => (f * 4, f * 4 + 4),
        }
    }

    #[inline]
    pub(crate) fn face(&self, f: usize) -> &[u32] {
        let (from, to) = self.range(f);
        &self.corners[from..to]
    }

    /// The corner after `corner` round its face.
    #[inline]
    pub(crate) fn next(&self, corner: usize) -> usize {
        let (from, to) = self.range(self.face_of(corner));
        if corner + 1 == to { from } else { corner + 1 }
    }

    /// The corner before `corner` round its face.
    #[inline]
    pub(crate) fn prev(&self, corner: usize) -> usize {
        let (from, to) = self.range(self.face_of(corner));
        if corner == from { to - 1 } else { corner - 1 }
    }

    /// The triangle that has the face side starting at `corner` as one of its
    /// own sides.
    #[inline]
    pub(crate) fn side_triangle(&self, corner: usize) -> u32 {
        match self.given {
            Some(g) => g.side_triangle[corner],
            None => (corner / 4 * 2 + (corner % 4) / 2) as u32,
        }
    }
}
