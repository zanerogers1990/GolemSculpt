//! Editing a coarse level and stepping back up: the detail sculpted at the
//! finer levels must come back, the same size, turned with the surface, and
//! nothing the edit does not reach may change at all.

use glam::{Mat3, Quat, Vec3};

use super::fixtures::*;
use crate::{LevelMesh, Levels, Split};

/// A stack of `levels + 1` levels over `base`, sculpted all over at the top,
/// and on its finest level.
fn sculpted(base: &LevelMesh, levels: usize, seed: u64) -> (Levels, LevelMesh) {
    sculpted_by(base, levels, seed, 0.01)
}

/// [`sculpted`] with the roughness given: every vertex nudged by up to `amount`.
fn sculpted_by(base: &LevelMesh, levels: usize, seed: u64, amount: f32) -> (Levels, LevelMesh) {
    let (stack, mut top) = raised(base, &vec![Split::Smooth; levels]);
    roughen(&mut top, amount, &mut Rng(seed));
    (stack, top)
}

/// Steps from the finest level down to `level`, edits it with `edit`, and
/// steps back up to the finest.
fn edited_at(stack: &mut Levels, level: usize, top: &LevelMesh, edit: impl Fn(&mut Vec<Vec3>)) -> LevelMesh {
    let mut mesh = stack.step_to(level, &top.positions, &top.mask).unwrap().mesh;
    edit(&mut mesh.positions);
    stack.step_to(stack.top(), &mesh.positions, &mesh.mask).unwrap().mesh
}

/// Moving a coarse level rigidly is the one edit whose right answer is known
/// everywhere: the whole sculpt moves with it, detail and all.
#[test]
fn turning_or_shifting_a_coarse_level_turns_the_whole_sculpt_with_it() {
    let pivot = Vec3::new(0.3, -0.2, 0.1);
    let moves = [
        (Quat::from_axis_angle(Vec3::new(1.0, 2.0, -0.5).normalize(), 0.65), Vec3::ZERO),
        (Quat::from_axis_angle(Vec3::X, std::f32::consts::PI), Vec3::new(0.0, 0.5, 0.0)),
        (Quat::from_axis_angle(Vec3::new(-0.3, 0.1, 1.0).normalize(), 3.0), Vec3::new(2.0, -1.0, 4.0)),
        (Quat::IDENTITY, Vec3::new(-1.5, 0.25, 0.75)),
    ];
    let mut worst = 0.0f32;
    for (base, levels) in [(ico_sphere(1), 2), (ico_sphere(1), 4), (quad_torus(9, 6), 2), (cube(), 4)] {
        for (turn, shift) in moves {
            let rigid = |p: Vec3| pivot + turn * (p - pivot) + shift;
            for level in 0..levels {
                let (mut stack, top) = sculpted(&base, levels, 31);
                let moved = edited_at(&mut stack, level, &top, |p| p.iter_mut().for_each(|p| *p = rigid(*p)));
                let expected: Vec<Vec3> = top.positions.iter().map(|&p| rigid(p)).collect();
                let off = furthest(&moved.positions, &expected);
                assert!(off < 2e-5, "{off} off after turning level {level} of {levels}");
                worst = worst.max(off);
            }
        }
    }
    println!("rigid moves of a coarse level: the sculpt is at most {worst:e} from where it should be");
}

/// The edit the whole thing is for: a limb bent at a coarse level. Well past
/// the bend the limb has only turned, so there the sculpt must be exactly the
/// old sculpt turned; well short of it nothing may have changed at all.
#[test]
fn bending_a_limb_carries_its_detail_round_the_bend() {
    let mut limb = ico_sphere(3);
    limb.positions.iter_mut().for_each(|p| p.x *= 3.0);
    for levels in [2, 3] {
        let (mut stack, top) = sculpted(&limb, levels, 41);
        let reach = 3.3 * longest_edge(&limb.positions, &limb.triangles);
        assert!(reach < 2.0, "the limb is long enough to have ends the bend does not reach");
        let bend = Quat::from_axis_angle(Vec3::new(0.0, 0.3, 1.0).normalize(), 1.1);
        let fold = |p: &mut Vec<Vec3>| p.iter_mut().filter(|p| p.x > 0.0).for_each(|p| *p = bend * *p);
        let bent = edited_at(&mut stack, 0, &top, fold);

        let (mut turned, mut still) = (0, 0);
        for v in 0..top.positions.len() {
            let was = top.positions[v];
            if was.x > reach {
                turned += 1;
                assert!(bent.positions[v].distance(bend * was) < 5e-6, "vertex {v} past the bend");
            } else if was.x < -reach {
                still += 1;
                assert!(same_bits(&bent.positions[v..=v], &top.positions[v..=v]), "vertex {v} short of the bend");
            }
        }
        assert!(
            turned > top.positions.len() / 20 && still > top.positions.len() / 20,
            "{turned} turned, {still} still"
        );
        // And at the bend itself the surface has not torn or spiked: the
        // finest level's edges are stretched there no more than the coarse
        // edges across the fold were.
        let mut coarse = top.positions[..limb.positions.len()].to_vec();
        let coarse_was = longest_edge(&coarse, &limb.triangles);
        fold(&mut coarse);
        let coarse_stretch = longest_edge(&coarse, &limb.triangles) / coarse_was;
        let stretch = longest_edge(&bent.positions, &bent.triangles) / longest_edge(&top.positions, &top.triangles);
        println!(
            "limb bent 1.1 rad, {levels} levels: {turned} vertices turned, {still} untouched; longest edge \
             {stretch:.2}x where the coarse level's is {coarse_stretch:.2}x"
        );
        assert!(stretch < coarse_stretch * 1.1, "{stretch} against {coarse_stretch}");
    }
}

/// The apex of the bump and how far the bump stands off the surface there:
/// what the bumped mesh has over the plain one, along the plain one's normal.
fn standing(bumped: &LevelMesh, plain: &LevelMesh, apex: usize) -> (f32, f32) {
    let normal = vertex_normals(&plain.positions, &plain.triangles)[apex];
    let over = bumped.positions[apex] - plain.positions[apex];
    (over.dot(normal), (over - normal * over.dot(normal)).length())
}

/// Two stacks over the same sphere, alike but that one has a bump sculpted on
/// its finest level: small against the coarsest level's faces and in the
/// middle of one of them, so that level knows nothing of it.
///
/// The sculpt under it is only faintly rough, so that the surface's own
/// normal at the bump is a fair measure of "straight out".
fn with_and_without_a_bump(levels: usize) -> ([Levels; 2], [LevelMesh; 2], usize, f32) {
    let base = ico_sphere(2);
    let (stack, plain) = sculpted_by(&base, levels, 77, 0.0005);
    let face = &base.triangles[30..33];
    let middle = face.iter().map(|&v| base.positions[v as usize]).sum::<Vec3>().normalize();
    let near = |v: &usize| plain.positions[*v].distance(middle);
    let apex = (0..plain.positions.len()).min_by(|a, b| near(a).total_cmp(&near(b))).unwrap();
    let centre = plain.positions[apex];
    let (radius, height) = (0.1, 0.03);
    let coarse = &plain.positions[..base.positions.len()];
    assert!(coarse.iter().all(|p| p.distance(centre) > radius * 1.1), "the bump is between the coarse vertices");
    let mut bumped = plain.clone();
    raise_bump(&mut bumped.positions, centre, radius, height, |p| p.normalize());
    assert_eq!(bumped.positions[..coarse.len()], plain.positions[..coarse.len()]);
    ([stack.clone(), stack], [bumped, plain], apex, height)
}

#[test]
fn scaling_a_coarse_level_keeps_a_bump_its_own_height_and_standing_straight() {
    for levels in [2, 3] {
        let ([mut with, mut without], [bumped, plain], apex, height) = with_and_without_a_bump(levels);
        let pivot = Vec3::new(0.2, 0.1, -0.3);
        let scale = |p: &mut Vec<Vec3>| p.iter_mut().for_each(|p| *p = pivot + (*p - pivot) * 2.0);
        let (big_bumped, big_plain) =
            (edited_at(&mut with, 0, &bumped, scale), edited_at(&mut without, 0, &plain, scale));

        // The form did double (all but the little each level adds back over its smooth base, which is detail too)...
        let size =
            |m: &LevelMesh| m.positions.iter().map(|p| p.distance(pivot)).sum::<f32>() / m.positions.len() as f32;
        let grew = size(&big_plain) / size(&plain);
        assert!((grew - 2.0).abs() < 0.03, "{grew}");
        // ...and the bump did not: it is the height it was, straight out of the surface.
        let (was, _) = standing(&bumped, &plain, apex);
        let (out, aside) = standing(&big_bumped, &big_plain, apex);
        assert!((was - height).abs() < height * 0.05, "{was}");
        println!(
            "scaled by 2, {levels} levels: bump of {was} stands {out} out and {aside} aside; the form grew {grew}"
        );
        assert!(
            (out - height).abs() < height * 0.05 && aside < height * 0.05,
            "{levels} levels: {out} out, {aside} aside"
        );
        // All over, what the bump adds is what it added before.
        let added = |a: &LevelMesh, b: &LevelMesh| -> Vec<Vec3> {
            a.positions.iter().zip(&b.positions).map(|(a, b)| *a - *b).collect()
        };
        let off = furthest(&added(&big_bumped, &big_plain), &added(&bumped, &plain));
        println!("  and nowhere differs from the bump it was by more than {off}");
        assert!(off < height * 0.06, "{levels} levels: {off}");
    }
}

#[test]
fn moving_one_coarse_vertex_changes_only_its_neighbourhood() {
    for (base, levels) in [(ico_sphere(2), 2), (ico_sphere(2), 3), (quad_torus(24, 12), 3)] {
        let (mut stack, top) = sculpted(&base, levels, 5);
        let reach = 3.3 * longest_edge(&base.positions, &base.triangles);
        for vertex in [0, 17, base.positions.len() - 1] {
            let was = base.positions[vertex];
            let moved = edited_at(&mut stack.clone(), 0, &top, |p| p[vertex] += was.normalize() * 0.1);
            let changed: Vec<usize> = (0..top.positions.len())
                .filter(|&v| !same_bits(&top.positions[v..=v], &moved.positions[v..=v]))
                .collect();
            assert!(
                !changed.is_empty() && changed.len() < top.positions.len() / 3,
                "{} of {}",
                changed.len(),
                top.positions.len()
            );
            let furthest = changed.iter().map(|&v| top.positions[v].distance(was)).fold(0.0, f32::max);
            assert!(furthest < reach, "a vertex {furthest} away moved; the edit reaches {reach}");
            assert_eq!(moved.mask, top.mask, "and the mask not at all");
            // The vertex itself came most of the way: about half, which is the smooth rule's share (see `levels`).
            let came = (moved.positions[vertex] - top.positions[vertex]).dot(was.normalize()) / 0.1;
            assert!((0.3..0.75).contains(&came), "{came}");
            println!(
                "one coarse vertex moved, {levels} levels: {} of {} vertices changed, the furthest {furthest:.2} away \
                 (bound {reach:.2}); the vertex itself came {came:.2} of the way",
                changed.len(),
                top.positions.len()
            );
        }
        // The stack was cloned for each: it still steps up to the sculpt untouched.
        let again = edited_at(&mut stack, 0, &top, |_| ());
        assert!(same_bits(&again.positions, &top.positions));
    }
}

#[test]
fn a_bump_beside_a_moved_coarse_vertex_keeps_its_shape() {
    for levels in [2, 3] {
        let ([mut with, mut without], [bumped, plain], apex, height) = with_and_without_a_bump(levels);
        // A corner of the coarse face the bump sits in, pulled well out: the surface under the bump tilts.
        let corner = ico_sphere(2).triangles[30] as usize;
        let pull = |p: &mut Vec<Vec3>| {
            let out = p[corner].normalize();
            p[corner] += out * 0.12;
        };
        let (pulled_bumped, pulled_plain) =
            (edited_at(&mut with, 0, &bumped, pull), edited_at(&mut without, 0, &plain, pull));
        let tilt = {
            let (was, now) = (
                vertex_normals(&plain.positions, &plain.triangles),
                vertex_normals(&pulled_plain.positions, &pulled_plain.triangles),
            );
            was[apex].angle_between(now[apex])
        };
        assert!(tilt > 0.1, "the edit did tilt the surface under the bump ({tilt})");
        let (out, aside) = standing(&pulled_bumped, &pulled_plain, apex);
        println!(
            "surface tilted {tilt:.2} rad under a bump of {height}, {levels} levels: it stands {out} out, {aside} aside"
        );
        assert!((out - height).abs() < height * 0.03, "{levels} levels: {out} out of the tilted surface");
        assert!(aside < height * 0.03, "{levels} levels: leaning {aside}");

        // And not only at its top: everywhere on the bump, as high off the tilted surface as it was off the old one.
        let on_bump = (0..plain.positions.len()).filter(|&v| bumped.positions[v] != plain.positions[v]);
        let worst = on_bump
            .map(|v| {
                let (was, _) = standing(&bumped, &plain, v);
                let (now, aside) = standing(&pulled_bumped, &pulled_plain, v);
                (now - was).abs().max(aside)
            })
            .fold(0.0, f32::max);
        println!("  and over the whole bump its height off the surface changed by at most {worst}");
        assert!(worst < height * 0.05, "{levels} levels: {worst}");
    }
}

#[test]
fn an_edit_is_carried_the_same_whichever_way_the_levels_are_walked() {
    for base in [ico_sphere(1), quad_torus(9, 6)] {
        let (stack, top) = sculpted(&base, 3, 13);
        let edit = |p: &mut Vec<Vec3>| p.iter_mut().step_by(4).for_each(|p| *p *= 1.1);
        // Edit level 1, then: straight up; down first; or settling where we are first.
        let straight = edited_at(&mut stack.clone(), 1, &top, edit);
        for by_way_of in [0, 1, 2] {
            let mut stack = stack.clone();
            let mut mesh = stack.step_to(1, &top.positions, &top.mask).unwrap().mesh;
            edit(&mut mesh.positions);
            let mesh = stack.step_to(by_way_of, &mesh.positions, &mesh.mask).unwrap().mesh;
            let up = stack.step_to(3, &mesh.positions, &mesh.mask).unwrap().mesh;
            assert!(same_bits(&up.positions, &straight.positions), "by way of level {by_way_of}");
        }
        assert!(!same_bits(&straight.positions, &top.positions));
    }
}

#[test]
fn a_level_split_flat_follows_its_coarse_vertices_all_the_way() {
    // A cube split flat twice is still a cube. Pull one corner at the bottom
    // level: the corner goes the whole way and the faces stay made of flat pieces.
    let (mut stack, top) = raised(&cube(), &[Split::Flat, Split::Flat]);
    let pull = Vec3::new(0.5, 0.25, 0.0);
    let moved = edited_at(&mut stack, 0, &top, |p| p[7] += pull);
    assert_eq!(moved.positions[7], top.positions[7] + pull);
    assert_eq!(moved.positions[0], top.positions[0]);
    // Every vertex of the finest level is where flat splitting the edited cube puts it.
    let mut coarse = cube();
    coarse.positions[7] += pull;
    let (_, expected) = raised(&coarse, &[Split::Flat, Split::Flat]);
    assert!(furthest(&moved.positions, &expected.positions) < 1e-6);
}

#[test]
fn a_whole_turn_of_the_surface_is_followed_through_the_awkward_angles() {
    // A sheet with a ridge sculpted on it, folded over by turning its coarse
    // level about an axis in its own plane: at half a turn the normals are
    // exactly opposite, where the least rotation between them is not defined.
    let (stack, mut top) = raised(&triangle_grid(4, 4), &[Split::Smooth, Split::Smooth]);
    for p in &mut top.positions {
        p.z += 0.05 * (p.x * 3.0).sin() * (p.y * 2.0).cos();
    }
    for angle in [3.0, std::f32::consts::PI - 1e-4, std::f32::consts::PI, std::f32::consts::PI + 1e-6, 4.0] {
        let turn = Mat3::from_axis_angle(Vec3::new(1.0, 1.0, 0.0).normalize(), angle);
        let moved = edited_at(&mut stack.clone(), 0, &top, |p| p.iter_mut().for_each(|p| *p = turn * *p));
        let expected: Vec<Vec3> = top.positions.iter().map(|&p| turn * p).collect();
        let off = furthest(&moved.positions, &expected);
        assert!(off < 2e-5, "{off} off at {angle}");
    }
}
