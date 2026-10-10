//! Cuts a mesh into two closed pieces along a screen-space shape (the part the
//! shape covers and the part it does not) and reports which faces of each
//! piece are the cut faces that mate with each other. The geometry of the
//! Split tool, and what registration keys build on: a key needs both sides of
//! a cut and needs to know where they touch.
//!
//! Built from two trims of the same input, one keeping each side. The trimmer
//! finds every crossing by walking the same region test from the uncovered end
//! of an edge to the covered one, whichever side it is keeping, so the two
//! pieces' rims are the same points exactly, and both cut faces are filled on
//! the same swept surface: they coincide, as mating faces must, without having
//! to be triangulated alike.
//!
//! Several passes (symmetry, live mirror copies) cut the remaining outside
//! piece again each time and collect every covered piece into one: the covered
//! part is the union of the shapes, which is what a mirrored cut means. A cut
//! face made by an early pass is followed through the later ones by
//! `Trimmed::source_triangles`.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::{SymmetryOp, TriMesh};

use crate::region::{CutView, ScreenRegion};
use crate::trim::{Trimmed, open_loops_note, trim};

/// One cut: the camera as it sees the copy of the object the shape is drawn
/// over, and the symmetry operation the shape is tested through (see
/// `trim::trim`).
#[derive(Clone, Copy, Debug)]
pub struct SplitPass {
    pub view: CutView,
    pub symmetry: SymmetryOp,
}

impl SplitPass {
    /// Every pass a split needs: one per place the object is drawn (`views`:
    /// itself, then its live mirror copies) and per symmetry operation
    /// (`symmetry`, or empty for none), in the order a trim cuts them.
    pub fn all(views: &[CutView], symmetry: &[SymmetryOp]) -> Vec<SplitPass> {
        let identity = [SymmetryOp::IDENTITY];
        let symmetry = if symmetry.is_empty() { &identity[..] } else { symmetry };
        views.iter().flat_map(|&view| symmetry.iter().map(move |&symmetry| SplitPass { view, symmetry })).collect()
    }
}

/// One side of the cut, in the input's own local space.
#[derive(Clone, Debug, Default)]
pub struct Piece {
    pub mesh: TriMesh,
    /// One mask value per vertex when a mask was passed in, empty otherwise
    /// (see `Trimmed::mask`).
    pub mask: Vec<f32>,
    /// The triangles of each cut face on this piece.
    pub cut_faces: Vec<Vec<u32>>,
}

/// A mesh cut in two.
#[derive(Clone, Debug)]
pub struct Split {
    /// What the shape did not cover: stays on the object that was cut.
    pub outside: Piece,
    /// What it covered: becomes a new object.
    pub inside: Piece,
    /// Cut faces that are the two sides of one cut, as (index into
    /// `outside.cut_faces`, index into `inside.cut_faces`).
    pub face_pairs: Vec<(usize, usize)>,
    /// Cut loops the trimmer could not close cleanly, over both pieces.
    pub open_loops: usize,
    /// Triangles of the new cut faces, over both pieces.
    pub cut_face_triangles: usize,
}

impl Split {
    /// What to add to the tool's own message when some cut loops would not
    /// close, or `None` on a clean cut.
    pub fn warning(&self) -> Option<String> {
        (self.open_loops > 0).then(|| open_loops_note(self.open_loops).trim_start().to_string())
    }
}

/// Splits a mesh along `region`, once per pass, each later pass cutting what
/// the earlier ones left of the uncovered side.
///
/// `mask`, when given, is one value per vertex and comes back per vertex of
/// each piece.
///
/// Fails, with a sentence saying why, when no pass splits anything: the shape
/// misses the object, or covers all of it.
pub fn split(
    positions: &[Vec3],
    indices: &[u32],
    mask: Option<&[f32]>,
    passes: &[SplitPass],
    region: &ScreenRegion,
) -> Result<Split, String> {
    if positions.is_empty() || indices.len() < 3 {
        return Err("No geometry.".to_string());
    }

    // What is left of the uncovered side so far; `None` until a pass has cut.
    let mut current: Option<Trimmed> = None;
    // The outside piece's cut faces so far, and the pair each belongs to.
    let mut outside_faces: Vec<Vec<u32>> = Vec::new();
    let mut outside_pair: Vec<usize> = Vec::new();
    // The covered parts, one per pass that cut, with each cut face's pair.
    let mut inside_parts: Vec<Piece> = Vec::new();
    let mut inside_pair: Vec<Vec<usize>> = Vec::new();
    let mut next_pair = 0;
    let (mut open_loops, mut cut_face_triangles) = (0, 0);
    let mut last_error: Option<String> = None;
    let mut covered_everything = false;

    for pass in passes {
        let (verts, tris, pass_mask) = match &current {
            Some(cut) => (&cut.mesh.positions[..], &cut.mesh.indices[..], mask.is_some().then_some(&cut.mask[..])),
            None => (positions, indices, mask),
        };
        // The two sides are independent trims of the same input, run side by
        // side, which halves the wait on a dense model.
        let side = |remove_covered| trim(verts, tris, pass_mask, &pass.view, &pass.symmetry, region, remove_covered);
        let (outside, inside) = rayon::join(|| side(true), || side(false));
        let outside = match outside {
            Ok(outside) => outside,
            Err(error) => {
                // "Would delete the whole object" from the side that keeps the
                // uncovered part means the shape covers all of it: nothing to
                // split off, from this pass.
                covered_everything |= error.contains("whole object");
                last_error = Some(error);
                continue;
            }
        };
        let inside = match inside {
            Ok(inside) => inside,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };

        // Earlier cut faces, followed through this pass: whatever of them
        // survived, split or not, is still cut face.
        follow(&mut outside_faces, &outside.source_triangles, tris.len() / 3);

        let (mut new_outside, mut new_inside) = (face_lists(&outside), face_lists(&inside));
        let pairs = pair_faces(&outside.mesh, &new_outside, &inside.mesh, &new_inside);
        let mut piece = Piece { cut_faces: Vec::new(), mask: inside.mask, mesh: inside.mesh };
        let mut piece_pairs = Vec::new();
        for (o, i) in pairs {
            outside_faces.push(std::mem::take(&mut new_outside[o]));
            outside_pair.push(next_pair);
            piece.cut_faces.push(std::mem::take(&mut new_inside[i]));
            piece_pairs.push(next_pair);
            next_pair += 1;
        }
        inside_parts.push(piece);
        inside_pair.push(piece_pairs);
        open_loops += outside.open_loops + inside.open_loops;
        cut_face_triangles += outside.cap_triangles + inside.cap_triangles;
        current = Some(outside);
    }

    let Some(rest) = current else {
        let reason = if covered_everything {
            "the shape covers the whole object".to_string()
        } else {
            last_error.unwrap_or_else(|| "the shape covered nothing".to_string())
        };
        return Err(format!("Nothing split - {reason}."));
    };

    // A face a later pass cut away entirely has no partner left to name.
    let mut outside = Piece { mesh: rest.mesh, mask: rest.mask, cut_faces: Vec::new() };
    let mut outside_index_of_pair = vec![None; next_pair];
    for (face, &pair) in outside_faces.into_iter().zip(&outside_pair) {
        if !face.is_empty() {
            outside_index_of_pair[pair] = Some(outside.cut_faces.len());
            outside.cut_faces.push(face);
        }
    }
    let (inside, pair_of_face) = concatenate(inside_parts, inside_pair);
    let face_pairs =
        pair_of_face.iter().enumerate().filter_map(|(i, &pair)| outside_index_of_pair[pair].map(|o| (o, i))).collect();
    Ok(Split { outside, inside, face_pairs, open_loops, cut_face_triangles })
}

fn face_lists(cut: &Trimmed) -> Vec<Vec<u32>> {
    cut.cut_faces.iter().map(|range| range.clone().collect()).collect()
}

/// Replaces each face (a list of input triangles) with the output triangles of
/// a pass that came from any of them.
fn follow(faces: &mut [Vec<u32>], source: &[u32], input_triangles: usize) {
    if faces.is_empty() {
        return;
    }
    const NO_FACE: u32 = u32::MAX;
    let mut face_of = vec![NO_FACE; input_triangles];
    for (f, face) in faces.iter_mut().enumerate() {
        for &t in face.iter() {
            face_of[t as usize] = f as u32;
        }
        face.clear();
    }
    for (t, &from) in source.iter().enumerate() {
        // `from` is past the end for a triangle of this pass's own cut face.
        if let Some(&f) = face_of.get(from as usize)
            && f != NO_FACE
        {
            faces[f as usize].push(t as u32);
        }
    }
}

/// Matches each cut face on the outside piece with the one on the inside piece
/// that is the other side of the same cut loop. Both trims closed the same
/// loops, so they correspond one to one; matched by where they are
/// (area-weighted centroid), closest pairs first, and only between faces of
/// similar area.
fn pair_faces(a: &TriMesh, faces_a: &[Vec<u32>], b: &TriMesh, faces_b: &[Vec<u32>]) -> Vec<(usize, usize)> {
    let of_a: Vec<(Vec3, f32)> = faces_a.par_iter().map(|face| centroid(a, face)).collect();
    let of_b: Vec<(Vec3, f32)> = faces_b.par_iter().map(|face| centroid(b, face)).collect();
    let mut candidates = Vec::new();
    for (i, &(centre_a, area_a)) in of_a.iter().enumerate() {
        for (j, &(centre_b, area_b)) in of_b.iter().enumerate() {
            let (big, small) = (area_a.max(area_b), area_a.min(area_b));
            if big > 0.0 && small >= big * 0.8 {
                candidates.push((centre_a.distance_squared(centre_b), i, j));
            }
        }
    }
    candidates.sort_by(|x, y| x.0.total_cmp(&y.0));

    let (mut used_a, mut used_b) = (vec![false; of_a.len()], vec![false; of_b.len()]);
    let mut pairs = Vec::new();
    for (_, i, j) in candidates {
        if !used_a[i] && !used_b[j] {
            used_a[i] = true;
            used_b[j] = true;
            pairs.push((i, j));
        }
    }
    pairs
}

/// A face's area-weighted centre and its area.
fn centroid(mesh: &TriMesh, face: &[u32]) -> (Vec3, f32) {
    let (mut sum, mut area) = (Vec3::ZERO, 0.0f64);
    for &t in face {
        let [a, b, c] = mesh.triangle(t as usize);
        let weight = (b - a).cross(c - a).length() * 0.5;
        sum += (a + b + c) * (weight / 3.0);
        area += weight as f64;
    }
    (if area > 0.0 { sum / area as f32 } else { Vec3::ZERO }, area as f32)
}

/// Joins the covered parts into one piece, and returns the pair each of its
/// cut faces belongs to.
fn concatenate(mut parts: Vec<Piece>, pairs: Vec<Vec<usize>>) -> (Piece, Vec<usize>) {
    if parts.len() == 1 {
        return (parts.pop().unwrap_or_default(), pairs.into_iter().flatten().collect());
    }
    let mut whole = Piece::default();
    let mut pair_of_face = Vec::new();
    for (part, part_pairs) in parts.into_iter().zip(pairs) {
        let first_triangle = whole.mesh.triangle_count() as u32;
        whole.mesh.append(&part.mesh);
        whole.mask.extend_from_slice(&part.mask);
        whole
            .cut_faces
            .extend(part.cut_faces.into_iter().map(|face| face.into_iter().map(|t| t + first_triangle).collect()));
        pair_of_face.extend(part_pairs);
    }
    (whole, pair_of_face)
}
