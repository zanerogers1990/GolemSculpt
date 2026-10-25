//! Reconstruct Subdiv: the lower levels of a quad mesh that was made by
//! subdividing, found again from its faces alone.
//!
//! A model sculpted in ZBrush or Blender arrives as its finest level only:
//! a file holds one mesh. But a mesh that was made by Catmull-Clark
//! subdivision still carries every level below it in its connections, and
//! finding them again lets a sculptor step down and change a big form with
//! the detail carried, as if the model had been subdivided here. ZBrush calls
//! this Reconstruct Subdiv.
//!
//! # One step taken back
//!
//! One Catmull-Clark step leaves three kinds of vertex (`catmull_clark`):
//! the coarse vertices, kept; one on every coarse edge; and one in the middle
//! of every coarse face. Every fine face is a quad whose corners are, in
//! order round it, a kept vertex, an edge's, a face's and an edge's. So the
//! step is taken back by sorting the vertices into those three kinds:
//!
//! * Next to a quad that is sorted, a quad sharing a side with it can be
//!   sorted only one way, because the side has an edge's vertex at one end
//!   and a kept or a face's vertex at the other, and that fixes where the
//!   pattern starts. So one quad decides a whole piece of the mesh, and the
//!   sorting is one pass over its faces, as a flood from that quad. Where two
//!   quads disagree about a vertex, the piece was not made by subdividing.
//! * The first quad can start its pattern at any of its four corners, so the
//!   flood is run twice (starting at its first corner and at its second) and
//!   each answer is also read with kept and face vertices swapped, which
//!   floods the same way.
//! * Of those, an answer is only taken where an edge's vertex has four faces
//!   (two on an open rim, which it lies on) and a face's vertex has three or
//!   more and is not on a rim.
//! * More than one answer can pass. A subdivided cube is also a subdivided
//!   octahedron: swap its corners and its face middles and every rule still
//!   holds. The answer taken is the one with the most coarse quads (face
//!   vertices with four faces) because the meshes people subdivide are
//!   mostly quads; where that ties, the one that keeps more of the vertices
//!   that do not have four edges, because those are where a base mesh has
//!   its poles. A grid with no such vertices at all, a torus say, passes
//!   every way and every way is right: each is the same grid shifted. There
//!   the answer taken keeps the piece's lowest-numbered vertex, because
//!   ZBrush and Blender number a subdivided mesh's coarse vertices first, so
//!   a file from either gives back the grid it was made from.
//!
//! Each coarse face is then read off round its middle vertex: the quads
//! there, in order, each give the face one corner, its kept vertex. A coarse
//! vertex is numbered in the order of the fine vertex it was.
//!
//! # Checked, not trusted
//!
//! The rules above are what every subdivided mesh satisfies, not proof that a
//! mesh was subdivided. So the levels found are built into a real
//! [`Levels`] from the coarsest faces, and its finest level's faces are
//! matched against the faces given, vertex for vertex, through the levels in
//! between. Only a stack whose finest level is the given mesh exactly,
//! renumbered, is handed back; if the deepest stack fails, a shallower one is
//! tried.
//!
//! Every step is linear in the faces: the vertex-to-corner rows and each
//! side's neighbour are made in one sweep each (the latter on every thread),
//! and the flood touches each face once per try.
//!
//! What it cannot do: a mesh with an edge that more than two faces share, or
//! with faces wound against their neighbours, or two pieces that touch at a
//! single vertex, is refused, as are levels with fewer than four faces in
//! any piece (they would hardly be a model).

use rayon::prelude::*;

use crate::levels::{Levels, LevelsParts};
use crate::mesh::Polys;
use crate::refine::{NONE, PER_TASK, Rows, Split};

/// The three kinds of vertex, and a vertex not yet sorted.
const KEPT: u8 = 0;
const EDGE: u8 = 1;
const MIDDLE: u8 = 2;
const UNSORTED: u8 = u8::MAX;

/// The kinds round a fine quad, from its kept corner.
const PATTERN: [u8; 4] = [KEPT, EDGE, MIDDLE, EDGE];

/// A level with fewer faces than this in any one piece is not gone down to.
const FEWEST_FACES: usize = 4;

/// More steps down than any mesh that fits in memory could take.
const MOST_STEPS: usize = 16;

/// How a reading of a piece scores, better first: its coarse quads, its kept
/// vertices without four edges, and whether its lowest vertex is kept.
type Score = (usize, usize, bool);

/// One subdivision taken back: the coarser mesh, and how each of its corners
/// made a face of the finer one.
#[derive(Clone, Debug, PartialEq)]
pub struct Coarser {
    pub vertex_count: usize,
    /// For each coarse vertex, the fine vertex it is (ascending).
    pub kept: Vec<u32>,
    /// The coarse faces, over the coarse vertices, wound as the fine ones.
    pub polys: Polys,
    /// For each corner of `polys`, the fine face made at it.
    pub corner_face: Vec<u32>,
    /// For each fine face, which of its corners is the kept vertex: the face
    /// is that corner, then the vertex on the edge leaving it, the face's
    /// middle, and the vertex on the edge arriving at it.
    pub turn: Vec<u8>,
}

/// What [`reconstruct`] found: the given mesh as the finest level of a stack.
pub struct Rebuilt {
    /// The stack, on its finest level, which has every face of the given
    /// mesh. Its kept finest positions are none: the caller's positions,
    /// renumbered by `vertex`, are the finest level.
    pub levels: Levels,
    /// For each vertex of the finest level, the given vertex it is.
    pub vertex: Vec<u32>,
    /// For each face of the finest level, the given face it is.
    pub face: Vec<u32>,
    /// The finest level's faces, in its own numbering (as
    /// `levels.polys(levels.top())` would write them out again).
    pub polys: Polys,
}

/// Takes back one Catmull-Clark subdivision of a mesh of quads, if it was
/// one. `None` if a face is not a quad, if the mesh cannot be sorted into
/// kept, edge and face vertices by the rules in the module notes, or if a
/// piece of it would be left with fewer than four faces.
///
/// This step is not checked against a subdivision of what it returns;
/// [`reconstruct`] does that for the levels it hands back.
pub fn unsubdivide(vertex_count: usize, polys: &Polys) -> Option<Coarser> {
    let fine = Fine::new(vertex_count, polys)?;
    let (turn, kind) = fine.sort()?;
    fine.coarse_faces(&turn, &kind)
}

/// Every level below a quad mesh that was made by subdividing it, as a stack
/// whose finest level is the mesh. `None` if there is not even one.
pub fn reconstruct(vertex_count: usize, polys: &Polys) -> Option<Rebuilt> {
    let mut steps: Vec<Coarser> = Vec::new();
    while steps.len() < MOST_STEPS {
        let below = match steps.last() {
            Some(step) => unsubdivide(step.vertex_count, &step.polys),
            None => unsubdivide(vertex_count, polys),
        };
        match below {
            Some(step) => steps.push(step),
            None => break,
        }
    }
    while !steps.is_empty() {
        if let Some(rebuilt) = rebuild(vertex_count, polys, &steps) {
            return Some(rebuilt);
        }
        steps.pop();
    }
    None
}

/// A kind with kept and middle swapped: the other reading of a flood.
fn swapped(kind: u8) -> u8 {
    match kind {
        KEPT => MIDDLE,
        MIDDLE => KEPT,
        other => other,
    }
}

/// The corner after `c` round its quad.
#[inline]
fn next(c: usize) -> usize {
    (c & !3) | ((c + 1) & 3)
}

/// The corner before `c` round its quad.
#[inline]
fn prev(c: usize) -> usize {
    (c & !3) | ((c + 3) & 3)
}

/// A mesh of quads with what the sorting reads: the corners at each vertex,
/// the neighbour across each side, and which vertices are on a rim.
struct Fine<'a> {
    quads: &'a [u32],
    at: Rows,
    /// For the side starting at each corner, the corner of the neighbouring
    /// quad where the same side starts the other way; `NONE` on a rim.
    across: Vec<u32>,
    rim: Vec<bool>,
}

impl<'a> Fine<'a> {
    fn new(vertex_count: usize, polys: &'a Polys) -> Option<Fine<'a>> {
        let faces = polys.face_count();
        let quads = &polys.indices[..];
        let whole = faces > 0
            && quads.len() == faces * 4
            && polys.start.par_iter().enumerate().all(|(f, &s)| s as usize == f * 4)
            && quads.len() < (u32::MAX / 2) as usize
            && vertex_count < NONE as usize;
        if !whole {
            return None;
        }
        let sound =
            |q: &[u32]| q.iter().all(|&v| (v as usize) < vertex_count) && (0..4).all(|k| !q[k + 1..].contains(&q[k]));
        if !quads.par_chunks_exact(4).with_min_len(PER_TASK).all(sound) {
            return None;
        }
        let at = Rows::build(vertex_count, |put| {
            for (corner, &v) in quads.iter().enumerate() {
                put(v, corner as u32);
            }
        });
        // A side's neighbour runs it the other way. Two neighbours is an edge
        // more than two faces share; one running it the same way is a face
        // wound backwards. Either is refused.
        let across: Option<Vec<u32>> = (0..quads.len())
            .into_par_iter()
            .with_min_len(PER_TASK)
            .map(|c| {
                let (a, b) = (quads[c], quads[next(c)]);
                let mut found = NONE;
                for &d in at.row(b as usize) {
                    if quads[next(d as usize)] == a {
                        if found != NONE {
                            return None;
                        }
                        found = d;
                    }
                }
                let backwards = at.row(a as usize).iter().any(|&d| d as usize != c && quads[next(d as usize)] == b);
                (!backwards).then_some(found)
            })
            .collect();
        let across = across?;
        let rim = (0..vertex_count)
            .into_par_iter()
            .with_min_len(PER_TASK)
            .map(|v| at.row(v).iter().any(|&c| across[c as usize] == NONE || across[prev(c as usize)] == NONE))
            .collect();
        Some(Fine { quads, at, across, rim })
    }

    fn face_count(&self) -> usize {
        self.quads.len() / 4
    }

    /// Every face's turn (which corner is its kept vertex) and every vertex's
    /// kind, piece by piece, by the rules in the module notes.
    fn sort(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let faces = self.face_count();
        let (order, link, pieces) = self.flood();
        let mut turn = vec![0u8; faces];
        let mut kind = vec![UNSORTED; self.at_len()];
        let mut touched = Vec::new();
        for piece in pieces.windows(2) {
            let piece = &order[piece[0]..piece[1]];
            // The first flood's answer, if it passes, is set aside while the
            // second is tried (its vertices, their kinds and the piece's
            // turns) and put back if it scores better, not flooded again.
            let mut aside: Option<(Vec<u32>, Vec<u8>, Vec<u8>, bool, Score)> = None;
            let mut kept: Option<(bool, Score)> = None;
            for first in 0..2u8 {
                let fits = self.flood_kinds(piece, &link, first, &mut turn, &mut kind, &mut touched);
                let best = if fits { self.best_reading(&touched, &kind) } else { None };
                match (first, best) {
                    (0, Some((swap, score))) => {
                        let kinds = touched.iter().map(|&v| kind[v as usize]).collect();
                        let turns = piece.iter().map(|&f| turn[f as usize]).collect();
                        aside = Some((std::mem::take(&mut touched), kinds, turns, swap, score));
                    }
                    (_, Some((swap, score))) if aside.as_ref().is_none_or(|a| score > a.4) => {
                        kept = Some((swap, score));
                        break;
                    }
                    _ => {}
                }
                touched.drain(..).for_each(|v| kind[v as usize] = UNSORTED);
                if let Some((vertices, _, _, _, _)) = &aside {
                    vertices.iter().for_each(|&v| kind[v as usize] = UNSORTED);
                }
            }
            let swap = match (kept, aside) {
                (Some((swap, _)), _) => swap,
                (None, Some((vertices, kinds, turns, swap, _))) => {
                    for (&v, &k) in vertices.iter().zip(&kinds) {
                        kind[v as usize] = k;
                    }
                    for (&f, &t) in piece.iter().zip(&turns) {
                        turn[f as usize] = t;
                    }
                    touched = vertices;
                    swap
                }
                (None, None) => return None,
            };
            if swap {
                piece.iter().for_each(|&f| turn[f as usize] = (turn[f as usize] + 2) & 3);
                for &v in &touched {
                    kind[v as usize] = swapped(kind[v as usize]);
                }
            }
            let middles = touched.iter().filter(|&&v| kind[v as usize] == MIDDLE).count();
            if middles < FEWEST_FACES {
                return None;
            }
            touched.clear();
        }
        // A vertex no face uses has no kind, and no level could number it.
        if kind.contains(&UNSORTED) {
            return None;
        }
        Some((turn, kind))
    }

    fn at_len(&self) -> usize {
        self.rim.len()
    }

    /// The faces in flood order, piece after piece; for each face but the
    /// first of a piece, its own corner on the side it was reached across;
    /// and where each piece starts in the order, and where the last ends.
    fn flood(&self) -> (Vec<u32>, Vec<u32>, Vec<usize>) {
        let faces = self.face_count();
        let mut seen = vec![false; faces];
        let mut order = Vec::with_capacity(faces);
        let mut link = vec![NONE; faces];
        let mut pieces = Vec::new();
        for seed in 0..faces {
            if seen[seed] {
                continue;
            }
            pieces.push(order.len());
            seen[seed] = true;
            order.push(seed as u32);
            let mut head = order.len() - 1;
            while head < order.len() {
                let f = order[head] as usize;
                head += 1;
                for c in f * 4..f * 4 + 4 {
                    let d = self.across[c];
                    if d != NONE && !seen[d as usize / 4] {
                        seen[d as usize / 4] = true;
                        link[d as usize / 4] = d;
                        order.push(d / 4);
                    }
                }
            }
        }
        pieces.push(order.len());
        (order, link, pieces)
    }

    /// Floods one piece from its first face turned to `first`, giving each
    /// face its turn and each vertex its kind, and noting in `touched` each
    /// vertex it gave a kind. False where two faces disagree about a vertex.
    fn flood_kinds(
        &self,
        piece: &[u32],
        link: &[u32],
        first: u8,
        turn: &mut [u8],
        kind: &mut [u8],
        touched: &mut Vec<u32>,
    ) -> bool {
        for &f in piece {
            let f = f as usize;
            let t = match link[f] {
                NONE => first as usize,
                d => {
                    // The side runs from `a` to `b` in the face it was reached
                    // from and back from `b` to `a` here; whichever end is a
                    // kept or middle vertex there fixes where the pattern starts here.
                    let c = self.across[d as usize] as usize;
                    let from = turn[c / 4] as usize;
                    let (pc, pd) = (c & 3, d as usize & 3);
                    let ib = (pc + 1 + 4 - from) & 3;
                    if ib % 2 == 0 { (pd + 4 - ib) & 3 } else { (pd + 1 + 4 - ((pc + 4 - from) & 3)) & 3 }
                }
            };
            turn[f] = t as u8;
            for p in 0..4 {
                let v = self.quads[f * 4 + p] as usize;
                let k = PATTERN[(p + 4 - t) & 3];
                match kind[v] {
                    UNSORTED => {
                        kind[v] = k;
                        touched.push(v as u32);
                    }
                    was if was == k => {}
                    _ => return false,
                }
            }
        }
        true
    }

    /// The better of the two readings of a flood that fits (as it is, or with
    /// kept and middle swapped), if either keeps the rules; the first where
    /// they tie.
    fn best_reading(&self, touched: &[u32], kind: &[u8]) -> Option<(bool, Score)> {
        let mut best: Option<(bool, Score)> = None;
        for swap in [false, true] {
            if let Some(score) = self.judge(touched, kind, swap).filter(|&s| best.is_none_or(|(_, b)| s > b)) {
                best = Some((swap, score));
            }
        }
        best
    }

    /// Whether a piece sorted as `kind` (with kept and middle swapped if
    /// `swap`) keeps the rules for edge and middle vertices, and if so how it
    /// scores: its coarse quads, then its kept vertices without four edges,
    /// then whether its lowest-numbered vertex is kept.
    fn judge(&self, touched: &[u32], kind: &[u8], swap: bool) -> Option<Score> {
        let (mut quads, mut poles) = (0, 0);
        for &v in touched {
            let v = v as usize;
            let k = if swap { swapped(kind[v]) } else { kind[v] };
            let (faces, rim) = (self.at.row(v).len(), self.rim[v]);
            match k {
                EDGE if !(faces == 4 && !rim || faces == 2 && rim) => return None,
                MIDDLE if rim || faces < 3 => return None,
                MIDDLE => quads += (faces == 4) as usize,
                KEPT => poles += (faces + rim as usize != 4) as usize,
                _ => {}
            }
        }
        let lowest = touched.iter().min().is_some_and(|&v| (kind[v as usize] == KEPT) != swap);
        Some((quads, poles, lowest))
    }

    /// The coarse mesh of a sorted fine one: each middle vertex's quads, in
    /// order round it, give its face's corners.
    fn coarse_faces(&self, turn: &[u8], kind: &[u8]) -> Option<Coarser> {
        let quads = self.quads;
        let kept: Vec<u32> = (0..kind.len() as u32).filter(|&v| kind[v as usize] == KEPT).collect();
        let mut number = vec![NONE; kind.len()];
        for (coarse, &v) in kept.iter().enumerate() {
            number[v as usize] = coarse as u32;
        }
        let middles = kind.iter().filter(|&&k| k == MIDDLE).count();
        let mut start = Vec::with_capacity(middles + 1);
        start.push(0u32);
        let mut corners = Vec::with_capacity(self.face_count());
        let mut corner_face = Vec::with_capacity(self.face_count());
        // The face each coarse vertex was last made a corner of: a face may
        // not have the same corner twice.
        let mut last = vec![NONE; kept.len()];
        for v in (0..kind.len()).filter(|&v| kind[v] == MIDDLE) {
            let row = self.at.row(v);
            let face = (start.len() - 1) as u32;
            let first = row[0] as usize / 4;
            let mut q = first;
            loop {
                let t = turn[q] as usize;
                debug_assert_eq!(quads[q * 4 + ((t + 2) & 3)] as usize, v, "the quad's middle is this vertex");
                let corner = number[quads[q * 4 + t] as usize];
                if last[corner as usize] == face {
                    return None;
                }
                last[corner as usize] = face;
                corners.push(corner);
                corner_face.push(q as u32);
                // On to the quad across the side from the edge vertex leaving
                // this quad's corner to the middle: the next corner's.
                let d = self.across[q * 4 + ((t + 1) & 3)];
                if d == NONE {
                    return None;
                }
                q = d as usize / 4;
                let went = corners.len() - start[start.len() - 1] as usize;
                if q == first {
                    if went != row.len() {
                        return None;
                    }
                    break;
                }
                if went >= row.len() {
                    return None;
                }
            }
            start.push(corners.len() as u32);
        }
        Some(Coarser {
            vertex_count: kept.len(),
            kept,
            polys: Polys { start, indices: corners },
            corner_face,
            turn: turn.to_vec(),
        })
    }
}

/// Faces fanned from their first corner into triangles, as the stack wants
/// them beside its polygon faces.
fn fanned(polys: &Polys) -> Vec<u32> {
    let mut triangles = Vec::with_capacity((polys.indices.len() - 2 * polys.face_count()) * 3);
    for w in polys.start.windows(2) {
        let face = &polys.indices[w[0] as usize..w[1] as usize];
        for k in 1..face.len() - 1 {
            triangles.extend_from_slice(&[face[0], face[k], face[k + 1]]);
        }
    }
    triangles
}

/// Whether `list` names each of `0..count` exactly once.
fn each_once(list: &[u32], count: usize) -> bool {
    if list.len() != count {
        return false;
    }
    let mut hit = vec![false; count];
    list.iter().all(|&i| (i as usize) < count && !std::mem::replace(&mut hit[i as usize], true))
}

/// The stack `steps` describe (the given mesh, then each coarser one), built
/// from its coarsest faces and checked against the given mesh through every
/// level: see the module notes. `None` if it is not that mesh.
fn rebuild(vertex_count: usize, given: &Polys, steps: &[Coarser]) -> Option<Rebuilt> {
    let depth = steps.len();
    let base = &steps[depth - 1];
    let parts = LevelsParts {
        vertex_count: base.vertex_count,
        triangles: fanned(&base.polys),
        polys: Some(base.polys.clone()),
        splits: vec![Split::Smooth; depth],
        current: depth,
        positions: Vec::new(),
        mask: Vec::new(),
    };
    let levels = Levels::from_parts(parts).ok()?;

    // For each corner of the stack's level, the corner of the found mesh of
    // that level it is. The coarsest level keeps the faces it was given.
    let mut corner_of: Vec<u32> = (0..base.polys.indices.len() as u32).collect();
    let mut vertex: Vec<u32> = (0..base.vertex_count as u32).collect();
    let mut face = Vec::new();
    let mut polys = Polys::default();
    for level in 1..=depth {
        // The step that took the found mesh of `level` down to `level - 1`,
        // and that mesh's faces and vertex count.
        let step = &steps[depth - level];
        let (finer, finer_vertices) = match level {
            l if l == depth => (given, vertex_count),
            _ => (&steps[depth - level - 1].polys, steps[depth - level - 1].vertex_count),
        };
        // The level's quad at corner `c` of the level below is the found
        // mesh's quad made at the corner `c` is, from its kept vertex on.
        polys = levels.polys(level)?;
        let quads = &polys.indices;
        if quads.len() != corner_of.len() * 4 || levels.vertex_count(level) != finer_vertices {
            return None;
        }
        let mut corners = vec![0u32; quads.len()];
        corners.par_chunks_exact_mut(4).with_min_len(PER_TASK).zip(&corner_of).for_each(|(out, &c)| {
            let q = step.corner_face[c as usize] as usize;
            let t = step.turn[q] as usize;
            for (p, out) in out.iter_mut().enumerate() {
                *out = (q * 4 + ((t + p) & 3)) as u32;
            }
        });
        face = corners.par_chunks_exact(4).map(|c| c[0] / 4).collect();
        if !each_once(&face, finer.face_count()) {
            return None;
        }
        vertex = vec![NONE; finer_vertices];
        for (&lv, &c) in quads.iter().zip(&corners) {
            vertex[lv as usize] = finer.indices[c as usize];
        }
        let agree = quads
            .par_iter()
            .zip(&corners)
            .with_min_len(PER_TASK)
            .all(|(&lv, &c)| vertex[lv as usize] == finer.indices[c as usize]);
        if !agree || !each_once(&vertex, finer_vertices) {
            return None;
        }
        corner_of = corners;
    }
    Some(Rebuilt { levels, vertex, face, polys })
}
