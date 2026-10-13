//! The mask through a change of level, and the helpers for whatever else
//! the app keeps per vertex or per face.

use glam::Vec3;

use super::fixtures::*;
use crate::{LevelMesh, Levels, Split, face_values_down, face_values_up};

const SMOOTH2: [Split; 2] = [Split::Smooth; 2];

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// The new vertices of `level` with the kept vertices each lies between,
/// found the slow way: flat splitting puts a new vertex at their mean, so
/// carrying each kept vertex's own number up as a channel names them.
fn between(levels: &Levels, level: usize) -> Vec<Vec<usize>> {
    let kept = levels.vertex_count(level - 1);
    let mut between = vec![Vec::new(); levels.vertex_count(level)];
    for v in 0..kept {
        let mut one = vec![0.0f32; kept];
        one[v] = 1.0;
        let up = levels.values_up(level - 1, level, &one).unwrap();
        (kept..up.len()).filter(|&i| up[i] > 0.0).for_each(|i| between[i].push(v));
    }
    between
}

#[test]
fn an_edited_coarse_mask_is_carried_up_over_what_was_painted_finer() {
    for base in [ico_sphere(1), quad_torus(9, 6)] {
        let (mut levels, mut top) = raised(&base, &SMOOTH2);
        roughen(&mut top, 0.0, &mut Rng(8));
        let mut coarse = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
        assert_eq!(bits(&coarse.mask), bits(&top.mask[..coarse.mask.len()]), "down, the mask is its first entries");

        // Mask a third of the coarse vertices fully and clear another third.
        let edited: Vec<usize> = (0..coarse.mask.len()).filter(|v| v % 3 != 2).collect();
        for &v in &edited {
            coarse.mask[v] = (v % 3) as f32;
        }
        let up = levels.step_to(2, &coarse.positions, &coarse.mask).unwrap().mesh;
        assert!(same_bits(&up.positions, &top.positions), "painting the mask moves nothing");
        assert!(up.mask.iter().all(|m| (0.0..=1.0).contains(m)));
        assert_eq!(
            bits(&up.mask[..coarse.mask.len()]),
            bits(&coarse.mask),
            "kept vertices have the mask they were given"
        );

        // Level by level: a new vertex none of whose neighbours changed keeps
        // its value to the bit, and one that was reached has what it had plus
        // the change in the mean of the vertices it lies between, kept in range.
        let mut changed: Vec<bool> =
            (0..coarse.mask.len()).map(|v| coarse.mask[v].to_bits() != top.mask[v].to_bits()).collect();
        for level in 1..=2 {
            let between = between(&levels, level);
            changed.resize(between.len(), false);
            for v in levels.vertex_count(level - 1)..between.len() {
                let mean = |mask: &[f32]| between[v].iter().map(|&k| mask[k]).sum::<f32>() / between[v].len() as f32;
                if between[v].iter().any(|&k| changed[k]) {
                    let expected = (top.mask[v] + mean(&up.mask) - mean(&top.mask)).clamp(0.0, 1.0);
                    assert!((up.mask[v] - expected).abs() < 1e-6, "vertex {v}: {} for {expected}", up.mask[v]);
                } else {
                    assert_eq!(up.mask[v].to_bits(), top.mask[v].to_bits(), "vertex {v} was not reached");
                }
                changed[v] = up.mask[v].to_bits() != top.mask[v].to_bits();
            }
        }
        assert!(changed.iter().any(|&c| c) && changed.iter().any(|&c| !c));
    }
}

#[test]
fn a_channel_goes_down_as_its_first_entries_and_up_as_plain_means() {
    for base in [ico_sphere(1), cube()] {
        let (levels, top) = raised(&base, &[Split::Flat, Split::Flat]);
        // A flat split is itself a channel carried up: the positions.
        let up = levels.values_up(0, 2, &base.positions).unwrap();
        assert!(same_bits(&up, &top.positions));
        let x: Vec<f32> = base.positions.iter().map(|p| p.x).collect();
        let up_x = levels.values_up(0, 2, &x).unwrap();
        assert!(up_x.iter().zip(&top.positions).all(|(x, p)| (x - p.x).abs() < 1e-6));
        // Two levels at once is one level twice.
        let twice = levels.values_up(1, 2, &levels.values_up(0, 1, &x).unwrap()).unwrap();
        assert_eq!(bits(&up_x), bits(&twice));
        assert_eq!(levels.values_up(1, 1, &twice[..levels.vertex_count(1)]).unwrap().len(), levels.vertex_count(1));

        let down = levels.values_down(2, 0, &top.positions).unwrap();
        assert!(same_bits(&down, &base.positions));
        assert_eq!(levels.values_down(2, 1, &up_x).unwrap(), up_x[..levels.vertex_count(1)]);

        assert!(levels.values_up(0, 2, &x[1..]).is_err(), "a value short");
        assert!(levels.values_up(2, 0, &up_x).is_err(), "up is up");
        assert!(levels.values_down(0, 2, &x).is_err(), "down is down");
        assert!(levels.values_up(0, 3, &x).is_err(), "no such level");
    }
}

#[test]
fn a_channel_edited_low_is_carried_up_over_the_finer_values() {
    for base in [ico_sphere(1), quad_torus(9, 6)] {
        let (levels, _) = raised(&base, &SMOOTH2);
        let mut rng = Rng(4);
        // A layer's offsets on the finest level, and its coarsest entries edited.
        let fine: Vec<Vec3> = (0..levels.vertex_count(2)).map(|_| rng.vector() * 0.1).collect();
        let mut edited = levels.values_down(2, 0, &fine).unwrap();
        assert!(
            same_bits(&levels.values_up_over(0, 2, &edited, &fine).unwrap(), &fine),
            "nothing edited, nothing changed"
        );
        let shift = Vec3::new(0.5, -0.25, 1.0);
        edited.iter_mut().for_each(|v| *v += shift);
        // The same change everywhere is the same change at every finer vertex.
        let up = levels.values_up_over(0, 2, &edited, &fine).unwrap();
        assert!(same_bits(&up[..edited.len()], &edited));
        assert!(up.iter().zip(&fine).all(|(up, fine)| (*up - *fine - shift).length() < 1e-6));

        // One entry changed reaches only the vertices next to it.
        let mut edited = levels.values_down(2, 0, &fine).unwrap();
        edited[5] += shift;
        let up = levels.values_up_over(0, 2, &edited, &fine).unwrap();
        let reached = (0..fine.len()).filter(|&v| !same_bits(&up[v..=v], &fine[v..=v])).count();
        assert!(reached > 5 && reached < fine.len() / 4, "{reached} of {}", fine.len());
        assert!(levels.values_up_over(0, 2, &edited[1..], &fine).is_err());
        assert!(levels.values_up_over(0, 2, &edited, &fine[1..]).is_err());
    }
}

#[test]
fn a_value_per_face_follows_its_face_up_and_comes_back_down() {
    let mut rng = Rng(2);
    for base in [ico_sphere(1), cube(), quad_torus(5, 4)] {
        let (levels, _) = raised(&base, &SMOOTH2);
        // A face group on every coarse triangle.
        let groups: Vec<u32> = (0..levels.triangle_count(0)).map(|_| (rng.next() * 5.0) as u32).collect();
        let up = levels.face_values(0, 2, &groups, u32::MAX).unwrap();
        assert_eq!(up.len(), levels.triangle_count(2));
        let (first, second) = (levels.parent_triangles(1).unwrap(), levels.parent_triangles(2).unwrap());
        for (t, &group) in up.iter().enumerate() {
            assert_eq!(group, groups[first[second[t] as usize] as usize]);
        }
        // Every coarse triangle of these meshes has children, so the way down is the way back.
        assert_eq!(levels.face_values(2, 0, &up, u32::MAX).unwrap(), groups);
        assert_eq!(
            levels.face_values(1, 1, &up[..levels.triangle_count(1)], 0).unwrap(),
            up[..levels.triangle_count(1)]
        );
        assert!(levels.face_values(0, 2, &groups[1..], 0).is_err());
        assert!(levels.face_values(0, 3, &groups, 0).is_err());
        assert!(levels.parent_triangles(0).is_err() && levels.parent_triangles(3).is_err());
    }
}

#[test]
fn a_face_with_no_children_takes_the_missing_value_and_the_first_child_wins() {
    // Children 0 and 2 of parent 1, child 1 of parent 0, none of parent 2.
    let parents = [1u32, 0, 1];
    assert_eq!(face_values_up(&parents, &['a', 'b', 'c']), ['b', 'a', 'b']);
    assert_eq!(face_values_down(&parents, &['x', 'y', 'z'], 3, '-'), ['y', 'x', '-']);

    // A hexagon cut into four triangles has one in the middle that touches
    // no side of the face, so no finer triangle lies along it.
    let corners: Vec<Vec3> = (0..6).map(|k| Vec3::new((k as f32).cos(), (k as f32).sin(), 0.0)).collect();
    let hexagon = LevelMesh {
        mask: vec![0.0; 6],
        positions: corners,
        triangles: vec![0, 1, 2, 2, 3, 4, 4, 5, 0, 0, 2, 4],
        polys: Some(crate::Polys { start: vec![0, 6], indices: vec![0, 1, 2, 3, 4, 5] }),
    };
    let (levels, _) = raised(&hexagon, &[Split::Flat]);
    let parents = levels.parent_triangles(1).unwrap();
    assert_eq!(parents.len(), 12);
    assert!(parents.iter().all(|&p| p < 3), "{parents:?}");
    assert_eq!(levels.face_values(1, 0, &[7u8; 12], 0).unwrap(), [7, 7, 7, 0]);
}
