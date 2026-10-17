//! The Lattice under mirror symmetry: with mirror axes ticked for the
//! object, a point of the cage that is moved takes its mirror image across
//! each plane with it, the mirror image of the way, so the two halves of
//! the model are reshaped alike. A point on a plane stays on it.
//!
//! The cage is laid over the box the model fills in the gizmo's frame, so its
//! points pair up across a plane when that box is even about the plane: a
//! mirrored model, with the gizmo's axes along the object's (or the world's,
//! for a World symmetry). Each point is paired with the point nearest its
//! mirror image, if one is near enough; across a plane where that fails for
//! any point, nothing is mirrored, and the status line's warning that the
//! halves will differ (deform.rs) says why.
//!
//! Only a change of one point is mirrored, which is what a drag of a point
//! or the card's number boxes make. A change of several at once (a new
//! cage, Reset, the Claude bridge setting a list of moves) is taken as it is.

use glam::{DAffine3, DMat3, DVec3, Mat4};

use super::super::Running;
use crate::deform::{Bounds, Frame, Kind, lattice};
use crate::scene::{Space, Symmetry};

/// How near a point's mirror image has to come to another point for the two
/// to pair, as a share of the cage's size along each axis. The points of a
/// cage are a fifth of it apart at the closest.
const PAIRS: f64 = 0.02;

/// The object's mirror planes as reflections of the world, one for each
/// ticked axis. `model` is the object's placement: its own planes go
/// through its origin along its axes, the world's through the world's.
pub(super) fn planes(symmetry: &Symmetry, model: Mat4) -> Vec<DAffine3> {
    let model = DAffine3::from_mat4(model.as_dmat4());
    (0..3)
        .filter(|&a| symmetry.mirror[a])
        .map(|a| {
            let mut signs = DVec3::ONE;
            signs[a] = -1.0;
            let flip = DAffine3::from_scale(signs);
            match symmetry.space {
                Space::Local => model * flip * model.inverse(),
                Space::World => flip,
            }
        })
        .collect()
}

/// A plane in the gizmo's frame: which point of the cage each point's mirror
/// image is, and what the plane does to a way a point is moved.
struct Across {
    partner: Vec<usize>,
    linear: DMat3,
}

/// Snaps the numbers of a reflection that are a hair from 0 or ±1 to them,
/// so that a cage lined up with the plane is mirrored to the bit.
fn snapped(m: DMat3) -> DMat3 {
    let snap = |x: f64| {
        let r = x.round();
        if r.abs() <= 1.0 && (x - r).abs() < 1e-6 { r } else { x }
    };
    DMat3::from_cols_array(&m.to_cols_array().map(snap))
}

/// How the cage pairs up across `plane` (a reflection of the world), if
/// every point has a partner.
fn across(rest: &[DVec3], size: DVec3, frame: &Frame, plane: DAffine3) -> Option<Across> {
    let into = DAffine3::from_rotation_translation(frame.rotation, frame.origin);
    let map = into.inverse() * plane * into;
    let reach = |d: DVec3| (d.abs() / size.max(DVec3::splat(1e-12))).max_element();
    let partner = rest
        .iter()
        .map(|&q| {
            let image = map.transform_point3(q);
            let (j, d) = rest
                .iter()
                .enumerate()
                .map(|(j, &r)| (j, reach(r - image)))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            (d <= PAIRS).then_some(j)
        })
        .collect::<Option<Vec<usize>>>()?;
    Some(Across { partner, linear: snapped(map.matrix3.into()) })
}

/// Moves the mirror images of point `moved` of the cage to match it, across
/// every plane of `planes` (and every pair of them, and all three). The
/// point itself goes to the average of where it was put and its mirror
/// images that land on it, which keeps a point on a plane on the plane.
/// False if the cage does not pair up across some plane, which is then left
/// out.
pub(super) fn mirror_point(
    cage: &mut lattice::Lattice,
    moved: usize,
    bounds: &Bounds,
    axis: usize,
    span: (f64, f64),
    frame: &Frame,
    planes: &[DAffine3],
) -> bool {
    let mut rest = cage.clone();
    rest.rest();
    let points: Vec<DVec3> = (0..rest.len()).map(|i| lattice::point(&rest, i, bounds, axis, span)).collect();
    let (_, size) = lattice::cage_box(bounds, axis, span);
    let mut every = true;
    // Every way across the planes: none, each, each two, all three.
    let mut ways: Vec<(Vec<usize>, DMat3)> = vec![((0..rest.len()).collect(), DMat3::IDENTITY)];
    for &plane in planes {
        let Some(plane) = across(&points, size, frame, plane) else {
            every = false;
            continue;
        };
        let more: Vec<(Vec<usize>, DMat3)> = ways
            .iter()
            .map(|(partner, linear)| (partner.iter().map(|&j| plane.partner[j]).collect(), plane.linear * *linear))
            .collect();
        ways.extend(more);
    }
    if ways.len() == 1 {
        return every;
    }
    // The way the point was moved, in the frame's units.
    let way = DVec3::from_array(cage.moved[moved].map(f64::from)) * size;
    let mut sums: Vec<(DVec3, u32)> = vec![(DVec3::ZERO, 0); cage.len()];
    for (partner, linear) in &ways {
        let sum = &mut sums[partner[moved]];
        sum.0 += *linear * way;
        sum.1 += 1;
    }
    for (j, (sum, n)) in sums.into_iter().enumerate() {
        if n > 0 {
            let way = if n == 1 { sum } else { sum / n as f64 };
            cage.moved[j] = (way / size.max(DVec3::splat(1e-12))).as_vec3().to_array();
        }
    }
    every
}

impl Running {
    /// Before the cage's settings are put on the model: if one point of it
    /// has been moved since, its mirror images across the selected object's
    /// planes are moved to match.
    pub(super) fn deform_mirror_lattice(&mut self) {
        let clean = self.doc.opts.deform.params.sanitized();
        if clean.kind != Kind::Lattice {
            return;
        }
        let Some(session) = self.transform.deform.session.as_ref() else { return };
        let Some(part) = session.parts.first() else { return };
        // What the model shows now: the last settings put on it, or a cage at rest.
        let was = match &session.shown {
            Some(shown) if shown.kind == Kind::Lattice && shown.lattice.count == clean.lattice.count => {
                shown.lattice.moved.clone()
            }
            _ => vec![[0.0; 3]; clean.lattice.len()],
        };
        let mut changed = (0..clean.lattice.len()).filter(|&i| clean.lattice.moved[i] != was[i]);
        let (Some(moved), None) = (changed.next(), changed.next()) else { return };
        let planes = planes(&self.doc.scene.symmetry(part.object), part.model);
        if planes.is_empty() {
            return;
        }
        let span = session.bounds.span(clean.axis, clean.range);
        let mut cage = clean.lattice.clone();
        mirror_point(&mut cage, moved, &session.bounds, clean.axis, span, &session.frame, &planes);
        self.doc.opts.deform.params.lattice = cage;
    }
}

#[cfg(test)]
mod tests {
    use glam::{DQuat, Mat4, Quat};

    use super::*;
    use crate::deform::lattice::Lattice;

    fn across(axes: [bool; 3]) -> Vec<DAffine3> {
        planes(&Symmetry { mirror: axes, ..Default::default() }, Mat4::IDENTITY)
    }

    const SQUARE: Frame = Frame { origin: DVec3::ZERO, rotation: DQuat::IDENTITY };

    fn even() -> Bounds {
        Bounds::new(DVec3::splat(-1.0), DVec3::splat(1.0))
    }

    /// Moves point `ijk` of a 3 x 3 x 3 cage `by` and mirrors it.
    fn moved(ijk: [usize; 3], by: [f32; 3], bounds: &Bounds, frame: &Frame, planes: &[DAffine3]) -> (Lattice, bool) {
        let mut cage = Lattice::default();
        let i = cage.index(ijk);
        cage.moved[i] = by;
        let span = bounds.span(1, [0.0, 1.0]);
        let every = mirror_point(&mut cage, i, bounds, 1, span, frame, planes);
        (cage, every)
    }

    #[test]
    fn a_point_takes_its_mirror_image_with_it_the_mirror_image_of_the_way() {
        let (cage, every) = moved([0, 2, 1], [0.1, 0.05, -0.02], &even(), &SQUARE, &across([true, false, false]));
        assert!(every);
        assert_eq!(cage.moved[cage.index([0, 2, 1])], [0.1, 0.05, -0.02]);
        assert_eq!(cage.moved[cage.index([2, 2, 1])], [-0.1, 0.05, -0.02], "to the bit");
        let others = (0..cage.len()).filter(|&i| cage.moved[i] != [0.0; 3]).count();
        assert_eq!(others, 2, "no other point moves");

        // Across X and Z, a corner takes the other three corners of its row.
        let (cage, _) = moved([0, 0, 0], [0.1, 0.2, 0.3], &even(), &SQUARE, &across([true, false, true]));
        assert_eq!(cage.moved[cage.index([2, 0, 0])], [-0.1, 0.2, 0.3]);
        assert_eq!(cage.moved[cage.index([0, 0, 2])], [0.1, 0.2, -0.3]);
        assert_eq!(cage.moved[cage.index([2, 0, 2])], [-0.1, 0.2, -0.3]);
        assert_eq!((0..cage.len()).filter(|&i| cage.moved[i] != [0.0; 3]).count(), 4);
    }

    #[test]
    fn a_point_on_the_plane_stays_on_it() {
        let (cage, _) = moved([1, 0, 0], [0.1, 0.2, 0.0], &even(), &SQUARE, &across([true, false, false]));
        assert_eq!(cage.moved[cage.index([1, 0, 0])], [0.0, 0.2, 0.0]);
    }

    #[test]
    fn with_the_gizmo_turned_a_quarter_the_pairs_follow_the_plane() {
        // The gizmo's Z is the object's X: the plane across X flips the cage along its Z.
        let frame = Frame { origin: DVec3::ZERO, rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2).as_dquat() };
        let (cage, every) = moved([1, 0, 0], [0.1, 0.2, 0.3], &even(), &frame, &across([true, false, false]));
        assert!(every);
        let partner = cage.moved[cage.index([1, 0, 2])];
        let want = [0.1f32, 0.2, -0.3];
        assert!(partner.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6), "{partner:?}");
    }

    #[test]
    fn a_cage_uneven_about_the_plane_is_left_as_it_was() {
        let lopsided = Bounds::new(DVec3::new(-1.0, -1.0, -1.0), DVec3::new(2.0, 1.0, 1.0));
        let (cage, every) = moved([0, 2, 1], [0.1, 0.0, 0.0], &lopsided, &SQUARE, &across([true, false, false]));
        assert!(!every);
        assert_eq!((0..cage.len()).filter(|&i| cage.moved[i] != [0.0; 3]).count(), 1);
    }

    #[test]
    fn the_worlds_plane_and_the_objects_own() {
        // An object off the world's origin: its own plane goes through it, the world's does not.
        let model = Mat4::from_translation(glam::Vec3::X * 3.0);
        let own = planes(&Symmetry { mirror: [true, false, false], ..Default::default() }, model);
        let world = planes(&Symmetry { mirror: [true, false, false], space: Space::World, ..Default::default() }, model);
        assert!(own[0].transform_point3(DVec3::new(4.0, 1.0, 0.0)).distance(DVec3::new(2.0, 1.0, 0.0)) < 1e-12);
        assert!(world[0].transform_point3(DVec3::new(4.0, 1.0, 0.0)).distance(DVec3::new(-4.0, 1.0, 0.0)) < 1e-12);
    }
}
