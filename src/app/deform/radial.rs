//! The Lattice under radial symmetry: with strokes repeated round an axis
//! for the object, a drag of a point of the cage is repeated as a stroke
//! is, through the same list of copies the brushes and the masked gizmo drag
//! keep (`Settings::symmetry_group`): every turn round the axis, and every
//! mirror image when mirror axes are ticked as well. Pull one point of the
//! cage out beside one tentacle and the clay beside every tentacle comes out.
//!
//! The cage is a box and a turn of a box is not one, so the copies are not
//! made of the cage's own points, as the mirror images are without turns
//! (mirror.rs). The deformer repeats what the dragged points do instead
//! (deform/lattice.rs, `Cage::repeated`), and a model that was symmetric
//! before the drag is symmetric after it, to rounding, whichever way the
//! gizmo's axes are turned and however lopsided the box. The cage shows its
//! points as they were dragged, and a faint dot where each copy of a dragged
//! point has gone.
//!
//! A point moved on its own is held on whatever its copies leave where it
//! is: a point on the axis goes along the axis and no other way, as a vertex
//! on the axis does under a stroke (sculpt/share.rs, `held_on`), and one on a
//! mirror plane among the copies stays on the plane. It is moved by the
//! average of its copies' moves, which is that. And its copies among the
//! other points of the cage are its own from then on: one of those that had
//! been moved before is put back, or the two drags would be counted twice.
//!
//! Without turns in the symmetry none of this has a say, and the Lattice is
//! what it always was. A custom axis tilted against a ticked mirror plane
//! gives a list of copies that is not closed, as for the brushes: each copy
//! is made once, and the model is as symmetric as such a list allows.

use glam::{DAffine3, DVec3, Mat4};

use super::super::Running;
use super::mirror::PAIRS;
use crate::deform::{Bounds, Frame, lattice};
use crate::scene::{Space, Symmetry};
use crate::sculpt::{Radial, Settings};

#[cfg(test)]
#[path = "radial_tests.rs"]
mod tests;

/// Every copy a drag of the cage is repeated as under `symmetry` but the
/// drag itself, as maps of the gizmo's `frame`; `None` with no turns in it.
/// `model` is the object's placement: its own symmetry sits in its own space,
/// the world's in the world.
pub(super) fn copies(symmetry: &Symmetry, model: Mat4, frame: &Frame) -> Option<Vec<DAffine3>> {
    if symmetry.radial.repeats() < Radial::MIN_COUNT {
        return None;
    }
    // The very list a stroke under this symmetry is made as, about the
    // origin of the space it sits in.
    let mut settings = Settings::default();
    settings.set_symmetry(symmetry.mirror, symmetry.radial, None);
    let group = settings.symmetry_group();
    if group.radial_count() < 2 {
        return None;
    }
    let into = match symmetry.space {
        Space::Local => frame.from_mesh(model),
        Space::World => frame.from_mesh(Mat4::IDENTITY),
    };
    let back = into.inverse();
    let copies =
        group.iter().filter(|op| !op.is_identity()).map(|op| into * DAffine3::from_mat4(op.to_mat4().as_dmat4()) * back);
    Some(copies.collect())
}

/// How far apart two places in the gizmo's frame are, as the largest share
/// of the cage's size along any axis: what `PAIRS` is measured in.
fn apart(d: DVec3, size: DVec3) -> f64 {
    (d.abs() / size.max(DVec3::splat(1e-12))).max_element()
}

/// The cage's points where the even grid puts them, and the cage's size.
fn at_rest(cage: &lattice::Lattice, bounds: &Bounds, axis: usize, span: (f64, f64)) -> (Vec<DVec3>, DVec3) {
    let mut rest = cage.clone();
    rest.rest();
    let points = (0..rest.len()).map(|i| lattice::point(&rest, i, bounds, axis, span)).collect();
    (points, lattice::cage_box(bounds, axis, span).1)
}

/// Point `moved` of the cage, just moved on its own, held on what its
/// `copies` leave where it is, and its copies among the other points of the
/// cage put back (see the top of this file).
pub(super) fn hold(
    cage: &mut lattice::Lattice,
    moved: usize,
    bounds: &Bounds,
    axis: usize,
    span: (f64, f64),
    copies: &[DAffine3],
) {
    let (points, size) = at_rest(cage, bounds, axis, span);
    let home = points[moved];
    let way = DVec3::from_array(cage.moved[moved].map(f64::from)) * size;
    let (mut sum, mut n) = (way, 1u32);
    for copy in copies {
        if apart(copy.transform_point3(home) - home, size) <= PAIRS {
            sum += copy.matrix3 * way;
            n += 1;
        }
    }
    if n > 1 {
        cage.moved[moved] = (sum / f64::from(n) / size.max(DVec3::splat(1e-12))).as_vec3().to_array();
    }
    for (j, &point) in points.iter().enumerate() {
        let twin = || copies.iter().any(|copy| apart(copy.transform_point3(home) - point, size) <= PAIRS);
        if j != moved && cage.moved[j] != [0.0; 3] && twin() {
            cage.moved[j] = [0.0; 3];
        }
    }
}

/// Where the copies of the cage's dragged points are, in the gizmo's frame,
/// for the faint dots that show them: each dragged point as each copy
/// carries it, but for the copies that leave it where it is.
pub(super) fn ghosts(
    cage: &lattice::Lattice,
    bounds: &Bounds,
    axis: usize,
    span: (f64, f64),
    copies: &[DAffine3],
) -> Vec<DVec3> {
    let (points, size) = at_rest(cage, bounds, axis, span);
    let mut out = Vec::new();
    for (i, &home) in points.iter().enumerate() {
        if cage.moved.get(i).is_none_or(|m| *m == [0.0; 3]) {
            continue;
        }
        let now = lattice::point(cage, i, bounds, axis, span);
        for copy in copies {
            if apart(copy.transform_point3(home) - home, size) > PAIRS {
                out.push(copy.transform_point3(now));
            }
        }
    }
    out
}

impl Running {
    /// The copies the Lattice's moves are repeated as, for the session's
    /// first object (as its mirror planes are, mirror.rs); `None` with
    /// another deformer up, or no turns in that object's symmetry. Worked
    /// out again only when the symmetry changes: a symmetry can have 255.
    pub(super) fn deform_lattice_copies(&mut self) -> Option<Vec<DAffine3>> {
        if self.doc.opts.deform.params.kind != crate::deform::Kind::Lattice {
            return None;
        }
        let session = self.transform.deform.session.as_ref()?;
        let part = session.parts.first()?;
        let symmetry = self.doc.scene.symmetry(part.object);
        if let Some((was, copies)) = &session.radial {
            if *was == symmetry {
                return copies.clone();
            }
        }
        let made = copies(&symmetry, part.model, &session.frame);
        self.transform.deform.session.as_mut()?.radial = Some((symmetry, made.clone()));
        made
    }

    /// Before the cage's settings are put on the model under radial
    /// symmetry: a point of it moved on its own since is held on its axis
    /// or plane, and its copies among the other points are put back.
    pub(super) fn deform_radial_lattice(&mut self, copies: &[DAffine3]) {
        let Some((moved, clean)) = self.lattice_lone_change() else { return };
        let Some(session) = self.transform.deform.session.as_ref() else { return };
        let span = session.bounds.span(clean.axis, clean.range);
        let mut cage = clean.lattice.clone();
        hold(&mut cage, moved, &session.bounds, clean.axis, span, copies);
        self.doc.opts.deform.params.lattice = cage;
    }
}
