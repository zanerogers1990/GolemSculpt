//! Vectors that ride on the surface (the app's sculpt layers): through a
//! new level, and up from an edited coarse one.

use glam::{Quat, Vec3};

use super::fixtures::*;
use crate::{LevelMesh, Rider, Split};

const SMOOTH2: [Split; 2] = [Split::Smooth; 2];

/// Something added to every vertex: fine, and different from one vertex to
/// the next, as sculpted detail is.
fn offsets(positions: &[Vec3], size: f32) -> Vec<Vec3> {
    positions.iter().map(|&p| p.normalize_or_zero() * size * (p.x * 23.0).sin() * (p.y * 17.0).cos()).collect()
}

/// A rider that the edit did nothing to on a level of `count` vertices.
fn left(finest: &[Vec3], count: usize) -> Rider<'_> {
    Rider { finest, level: &finest[..count] }
}

fn sum(a: &[Vec3], b: &[Vec3], scale: f32) -> Vec<Vec3> {
    a.iter().zip(b).map(|(a, b)| *a + *b * scale).collect()
}

#[test]
fn the_split_of_a_sum_is_the_sum_of_the_splits() {
    for base in [ico_sphere(1), cube(), quad_torus(9, 6), triangle_grid(5, 4), quad_grid(4, 3)] {
        for split in [Split::Smooth, Split::Flat] {
            let riding = offsets(&base.positions, 0.05);
            let with = LevelMesh { positions: sum(&base.positions, &riding, 0.6), ..base.clone() };
            let (levels, fine_with) = raised(&with, &[split]);
            let (_, fine_base) = raised(&base, &[split]);
            // What the rider adds to the new level is the rider put through the split.
            let up = levels.points_up(0, &riding).unwrap();
            let off = furthest(&fine_with.positions, &sum(&fine_base.positions, &up, 0.6));
            assert!(off < 5e-7, "{split:?}: {off}");
            // Which is not the rider carried up as a mask is, unless the split is flat.
            let plain = levels.values_up(0, 1, &riding).unwrap();
            assert_eq!(furthest(&up, &plain) < 1e-7, split == Split::Flat, "{split:?}");
            assert!(levels.points_up(1, &up).is_err() && levels.points_up(0, &up).is_err());
        }
    }
}

#[test]
fn riders_come_back_to_the_bit_where_the_edit_does_not_reach() {
    for base in [ico_sphere(1), quad_torus(9, 6)] {
        let (mut levels, mut top) = raised(&base, &SMOOTH2);
        roughen(&mut top, 0.01, &mut Rng(3));
        let (pores, scar) = (offsets(&top.positions, 0.02), offsets(&top.positions, -0.05));
        let whole = left(&pores, pores.len());
        assert!(levels.riders_up(&top.positions, &[whole]).is_err(), "on the finest level there is nothing to carry");
        let coarse = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
        let count = coarse.positions.len();
        // Nothing edited: every value as it was.
        let same = levels.riders_up(&coarse.positions, &[left(&pores, count), left(&scar, count)]).unwrap();
        assert!(same_bits(&same[0], &pores) && same_bits(&same[1], &scar));
        assert!(levels.riders_up(&coarse.positions, &[]).unwrap().is_empty());
        let short = Rider { finest: &pores[..count], level: &pores[..count] };
        assert!(levels.riders_up(&coarse.positions, &[short]).is_err(), "a rider is a list for the finest level");
        let long = Rider { finest: &pores, level: &pores[..count + 1] };
        assert!(levels.riders_up(&coarse.positions, &[long]).is_err(), "with its values for the level in hand");

        // One coarse vertex moved: only what is near it is touched, and an empty rider stays empty.
        let mut edited = coarse.positions.clone();
        edited[0] += Vec3::new(0.05, 0.02, -0.03);
        let nothing = vec![Vec3::ZERO; pores.len()];
        let carried = levels.riders_up(&edited, &[left(&pores, count), left(&nothing, count)]).unwrap();
        let up = levels.clone().step_to(2, &edited, &coarse.mask).unwrap().mesh;
        let touched: Vec<usize> = (0..pores.len()).filter(|&v| carried[0][v] != pores[v]).collect();
        assert!(!touched.is_empty() && touched.len() < pores.len() / 2, "{} of {}", touched.len(), pores.len());
        assert!(carried[1].iter().all(|o| *o == Vec3::ZERO));
        // What was touched is round the vertex that moved, and where the positions were touched too.
        let away = |v: usize| top.positions[v].distance(coarse.positions[0]);
        let reach = touched.iter().map(|&v| away(v)).fold(0.0, f32::max);
        let moved = (0..pores.len()).filter(|&v| up.positions[v] != top.positions[v]).map(away).fold(0.0, f32::max);
        let size = (0..pores.len()).map(away).fold(0.0, f32::max);
        assert!(reach < size * 0.8 && reach <= moved * 1.5, "{reach} {moved} {size}");
    }
}

/// The caller says what became of a rider on the level that was edited: here
/// it was turned with the level, as a pose turns what stands on a limb.
#[test]
fn a_coarse_level_turned_whole_turns_the_riders_whole() {
    let turn = Quat::from_axis_angle(Vec3::new(0.2, 1.0, -0.4).normalize(), 2.1);
    for from in [0, 1] {
        for base in [ico_sphere(1), cube()] {
            let (mut levels, mut top) = raised(&base, &SMOOTH2);
            roughen(&mut top, 0.01, &mut Rng(5));
            let pores = offsets(&top.positions, 0.02);
            let coarse = levels.step_to(from, &top.positions, &top.mask).unwrap().mesh;
            let moved: Vec<Vec3> = coarse.positions.iter().map(|&p| turn * p + Vec3::new(0.3, 0.0, -0.1)).collect();
            let turned: Vec<Vec3> = pores[..moved.len()].iter().map(|&o| turn * o).collect();
            let carried = levels.riders_up(&moved, &[Rider { finest: &pores, level: &turned }]).unwrap().remove(0);
            let off = (0..pores.len()).map(|v| carried[v].distance(turn * pores[v])).fold(0.0, f32::max);
            assert!(off < 1e-6, "from level {from}: {off}");
        }
    }
}

/// The reason riders are carried by the positions' own rule. A model with a
/// rider in its positions is twisted on the coarsest level; the same model
/// without the rider is twisted the same way. The first, less the rider as
/// it comes up, is the second: nothing of the rider stays behind.
///
/// The rider here is fine detail only: nothing at the vertices of the level
/// that is edited. (What an edit does to a rider there is the caller's to
/// say.)
///
/// Over one level nothing is left at all. Over two, a little: the second
/// step's turns are read off faces the rider is part of, so the model's own
/// detail over its base is turned a little differently with the rider on it.
#[test]
fn positions_less_a_rider_are_what_they_would_have_been_without_it() {
    let (one, two) = (&SMOOTH2[..1], &SMOOTH2[..]);
    let cases = [(ico_sphere(2), one, 0.004), (quad_torus(12, 8), one, 0.004), (ico_sphere(2), two, 0.0)];
    let cases = cases.into_iter().chain([(quad_torus(12, 8), two, 0.0), (ico_sphere(2), two, 0.004)]);
    for (base, splits, rough) in cases {
        let (mut plain_levels, mut plain) = raised(&base, splits);
        roughen(&mut plain, rough, &mut Rng(9));
        // The rider stands out square to the Y axis, the axis of the twist, so
        // that the twist turns it and does nothing else to it.
        let side = |p: Vec3| Vec3::new(p.x, 0.0, p.z).normalize_or_zero();
        let mut rider: Vec<Vec3> =
            plain.positions.iter().map(|&p| side(p) * 0.02 * (p.y * 40.0).sin() * (p.x * 31.0).cos()).collect();
        rider[..base.positions.len()].fill(Vec3::ZERO);
        let (mut levels, top) = raised(&base, splits);
        let with = sum(&plain.positions, &rider, 0.6);
        let twist = |p: Vec3| Quat::from_rotation_y(1.5 * p.y) * p;

        let coarse = levels.step_to(0, &with, &top.mask).unwrap().mesh;
        let edited: Vec<Vec3> = coarse.positions.iter().map(|&p| twist(p)).collect();
        let carried = levels.riders_up(&edited, &[left(&rider, edited.len())]).unwrap().remove(0);
        let up = levels.step_to(splits.len(), &edited, &coarse.mask).unwrap().mesh;

        let plain_coarse = plain_levels.step_to(0, &plain.positions, &plain.mask).unwrap().mesh;
        let plain_edited: Vec<Vec3> = plain_coarse.positions.iter().map(|&p| twist(p)).collect();
        let expected = plain_levels.step_to(splits.len(), &plain_edited, &plain_coarse.mask).unwrap().mesh;

        let left = furthest(&sum(&up.positions, &carried, -0.6), &expected.positions);
        // What would be left had the rider not turned at all.
        let stale = furthest(&sum(&up.positions, &rider, -0.6), &expected.positions);
        println!(
            "a rider of 0.012 under a twist, {} level(s) up, the model itself rough by {rough}: {left:.2e} of it left \
             behind ({stale:.2e} had it not turned)",
            splits.len()
        );
        assert!(stale > 0.004, "the twist was enough to tell ({stale})");
        let most = if splits.len() == 1 { 2e-6 } else { stale / 8.0 };
        assert!(left < most, "{left} against {stale}");
    }
}
