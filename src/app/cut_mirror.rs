//! Cut & Mirror, on the app's side: the two buttons at the bottom of the
//! Symmetry card.
//!
//! What it computes is `sculpt_symmetry::ops::mirror_and_weld`: the selected
//! object's mesh is cut at one of its own planes (X, Y or Z through its own
//! centre), the side that is not wanted is thrown away, and the side that is
//! kept is reflected into its place, the two halves sharing one row of
//! vertices on the plane. It is the repair for a model whose two sides no
//! longer match, which mirrored strokes cannot put right and a mirrored
//! Remesh only can by rebuilding every triangle.
//!
//! The plane is always the object's own, as in the Unity app: the routine
//! cuts the mesh where its own coordinate is zero, and a World symmetry's
//! plane cuts the mesh somewhere else that depends on where the object
//! stands. The card says so when the object's symmetry is the world's.
//!
//! Here is what the app adds: the mesh that comes back takes the old one's
//! place as one undo step (the old mesh is kept whole, so Ctrl+Z gives back the
//! mask, the hidden parts and the quads with it), the mask is carried across,
//! and the status line says what was kept, what was replaced, and whether
//! the result is a closed surface. Like everything that rebuilds a mesh, the
//! result starts with everything showing and as triangles.

use sculpt_symmetry::{Axis, ops};

use super::Running;
#[cfg(test)]
use super::symmetry_repair::DEFAULT_TOLERANCE;
use super::symmetry_repair::tolerance_scale;
use crate::mesh::Mesh;
use crate::printable::Cure;

/// `mesh` cut at its own `axis` plane and rebuilt as the side that is kept
/// and its mirror image: the positive side when `from_positive`. Returns the
/// new mesh, with the mask carried across, and the line that says what was
/// done; or why nothing was.
///
/// `tolerance_scale` is the card's Match Tolerance: here, how far off the
/// plane a vertex may sit and still be taken as on it, so that the slider
/// means one thing to every button under it. One is half a triangle's
/// width, about.
pub(crate) fn cut_and_mirrored_at(
    mesh: &Mesh,
    axis: Axis,
    tolerance_scale: f32,
    from_positive: bool,
) -> Result<(Mesh, String), String> {
    if mesh.triangle_count() == 0 {
        return Err("Nothing to cut and mirror.".into());
    }
    let built =
        ops::mirror_and_weld(&mesh.positions, &mesh.indices, Some(&mesh.mask), axis, tolerance_scale, from_positive)?;
    let mut text = format!("{}.", built.message());
    if mesh.any_hidden() {
        text += " Its hidden parts were cut with the rest, and everything is showing again.";
    }
    let mut cut = Mesh::new(built.mesh.positions, built.mesh.indices, built.mask);
    // Face groups: the side kept keeps its own, and the rebuilt side is their mirror image.
    super::groups::mirrored(mesh, &mut cut, axis as usize, from_positive);
    Ok((cut, text))
}

/// [`cut_and_mirrored_at`] with Match Tolerance where the slider starts: what
/// the checks of other parts of the app cut their models with.
#[cfg(test)]
pub(crate) fn cut_and_mirrored(mesh: &Mesh, axis: Axis, from_positive: bool) -> Result<(Mesh, String), String> {
    cut_and_mirrored_at(mesh, axis, DEFAULT_TOLERANCE, from_positive)
}

impl Running {
    /// The Cut & Mirror buttons: rebuilds the selected object as one side of
    /// its own `axis` plane and that side's mirror image, at the card's Match
    /// Tolerance. One undo step.
    pub(super) fn cut_and_mirror(&mut self, axis: Axis, from_positive: bool) {
        let tolerance = tolerance_scale(self.doc.opts.symmetry_tolerance);
        match cut_and_mirrored_at(&self.doc.mesh, axis, tolerance, from_positive) {
            Ok((mesh, text)) => {
                self.replace_mesh(mesh, true);
                self.toast("Cut & Mirror");
                // The cut runs through whatever holes the model had: say if it is not closed.
                self.status_checked(text, Cure::Remesh);
            }
            Err(why) => self.status(why, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use glam::Vec3;

    use super::*;
    use crate::history::History;
    use crate::mesh::primitives::{self, Primitive};
    use crate::printable;

    /// A ball pushed out of shape on its +X side only, and moved a little off
    /// its own centre, so the plane cuts through triangles.
    fn lopsided() -> Mesh {
        let ball = primitives::build(Primitive::Sphere, 8_000, 0.5);
        let bump = |p: Vec3| if p.x > 0.0 { p * (1.0 + 0.6 * p.x * (p.y * 5.0).cos().abs()) } else { p };
        let positions = ball.positions.iter().map(|&p| bump(p) + Vec3::new(0.013, 0.0, 0.0)).collect();
        Mesh::new(positions, ball.indices.clone(), None)
    }

    fn places(mesh: &Mesh) -> HashSet<[u32; 3]> {
        mesh.positions.iter().map(|&p| (p + Vec3::ZERO).to_array().map(f32::to_bits)).collect()
    }

    #[test]
    fn the_result_is_closed_and_exactly_symmetric_and_keeps_the_side_asked_for() {
        let model = lopsided();
        let reach = |mesh: &Mesh| (mesh.bounds().min.x, mesh.bounds().max.x);
        let (low, high) = reach(&model);
        assert!(high > -low + 0.05, "the +X side sticks out further: {low} to {high}");

        for (from_positive, kept) in [(true, high), (false, -low)] {
            let (cut, text) = cut_and_mirrored(&model, Axis::X, from_positive).unwrap();
            assert!(printable::census(&cut).is_closed(), "{text}");
            let all = places(&cut);
            let mirrored = |p: &Vec3| all.contains(&(Vec3::new(-p.x, p.y, p.z) + Vec3::ZERO).to_array().map(f32::to_bits));
            assert!(cut.positions.iter().all(mirrored), "every vertex has its mirror image, to the bit");
            assert!(cut.positions.iter().filter(|p| p.x == 0.0).count() > 20, "the seam is on the plane");
            let (low, high) = reach(&cut);
            assert!((high - kept).abs() < 1e-5 && low == -high, "{low} to {high}, kept {kept}");
            let (from, to) = if from_positive { ("+X", "-X") } else { ("-X", "+X") };
            assert!(text.starts_with(&format!("Cut & mirrored {from} onto {to}: kept ")), "{text}");
            assert!(text.ends_with(&format!("now {} vertices.", cut.vertex_count())), "{text}");
        }
    }

    /// Match Tolerance is how wide the band is that counts as the plane: a
    /// wider one takes more of the model's own vertices for the seam, where a
    /// narrow one cuts new ones beside them.
    #[test]
    fn match_tolerance_reaches_the_cut() {
        let model = lopsided();
        let vertices = |scale: f32| cut_and_mirrored_at(&model, Axis::X, scale, true).unwrap().0.vertex_count();
        let (tight, usual, loose) = (vertices(0.25), vertices(DEFAULT_TOLERANCE), vertices(8.0));
        assert!(tight > loose, "{tight} vertices at the tight end, {loose} at the loose end");
        assert_eq!(usual, cut_and_mirrored(&model, Axis::X, true).unwrap().0.vertex_count());
        for scale in [0.25, 8.0] {
            let (cut, text) = cut_and_mirrored_at(&model, Axis::X, scale, true).unwrap();
            assert!(printable::census(&cut).is_closed(), "x{scale}: {text}");
        }
    }

    #[test]
    fn the_mask_comes_across_on_both_sides_and_hidden_parts_are_spoken_of() {
        let mut model = lopsided();
        for v in 0..model.vertex_count() {
            model.mask[v] = if model.positions[v].y > 0.1 { 1.0 } else { 0.0 };
        }
        let (cut, text) = cut_and_mirrored(&model, Axis::X, true).unwrap();
        assert!(!text.contains("hidden"), "{text}");
        for (p, &m) in cut.positions.iter().zip(&cut.mask) {
            // Away from the mask's own edge, where a new vertex on the cut is between two values.
            if (p.y - 0.1).abs() > 0.05 {
                assert_eq!(m, if p.y > 0.1 { 1.0 } else { 0.0 }, "mask at {p}");
            }
        }

        // The whole model is cut, hidden parts included, and the line says they show again.
        let low: Vec<u32> = (0..model.triangle_count() as u32)
            .filter(|&t| model.indices[t as usize * 3..t as usize * 3 + 3].iter().all(|&v| model.positions[v as usize].y < -0.2))
            .collect();
        assert!(model.hide(&low).is_some());
        let (cut, text) = cut_and_mirrored(&model, Axis::X, true).unwrap();
        assert!(text.ends_with("everything is showing again."), "{text}");
        assert!(!cut.any_hidden() && printable::census(&cut).is_closed());
    }

    #[test]
    fn nothing_on_the_side_to_keep_is_refused_in_words_and_changes_nothing() {
        let mut far = primitives::build(Primitive::Cube, 600, 0.5);
        far.positions.iter_mut().for_each(|p| p.x -= 3.0);
        let far = Mesh::new(far.positions.clone(), far.indices.clone(), None);
        assert_eq!(cut_and_mirrored(&far, Axis::X, true).err().unwrap(), "Nothing on the +X side to mirror");
        // The other way it is the whole cube and its image: two cubes.
        let (both, _) = cut_and_mirrored(&far, Axis::X, false).unwrap();
        assert_eq!(both.triangle_count(), 2 * far.triangle_count());
        let empty = Mesh::new(Vec::new(), Vec::new(), None);
        assert_eq!(cut_and_mirrored(&empty, Axis::Y, true).err().unwrap(), "Nothing to cut and mirror.");
    }

    /// As the app records it (`Running::replace_mesh`): the old mesh is the
    /// undo step, so one Ctrl+Z gives back every vertex, the mask and what was
    /// hidden, and one Ctrl + Shift + Z the cut.
    #[test]
    fn one_undo_step_gives_the_model_back_as_it_was() {
        let mut mesh = lopsided();
        mesh.mask[7] = 0.5;
        assert!(mesh.hide(&[3, 4, 5]).is_some());
        let (before, uid) = (mesh.positions.clone(), mesh.uid);

        let mut history = History::default();
        let (cut, _) = cut_and_mirrored(&mesh, Axis::X, true).unwrap();
        let cut_vertices = cut.vertex_count();
        history.push_replace(std::mem::replace(&mut mesh, cut));
        assert_eq!(history.steps(), (1, 0));

        assert_eq!(history.undo(&mut mesh), Some(true));
        assert_eq!((mesh.uid, &mesh.positions, mesh.mask[7], mesh.hidden_triangle_count()), (uid, &before, 0.5, 3));
        assert!(!history.can_undo());
        assert_eq!(history.redo(&mut mesh), Some(true));
        assert_eq!(mesh.vertex_count(), cut_vertices);
        assert!(!mesh.any_hidden());
    }
}
