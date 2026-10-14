//! Face groups on a mesh: what each command makes, that undo puts back
//! exactly what was there, and that the groups follow the mesh through
//! everything that changes its triangles. The commands as the card runs
//! them, status lines included, are the scripted run `--demo-groups play`.

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};
use sculpt_multires::Split;

use super::*;
use crate::camera::{Camera, View};
use crate::history::History;
use crate::mesh::levels::{stepped, subdivided};
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Polys, StrokeDelta};
use crate::sculpt::{Brush, PointerSample, Sculptor};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn sphere(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

fn centre(mesh: &Mesh, t: usize) -> Vec3 {
    let c = &mesh.indices[t * 3..t * 3 + 3];
    (mesh.positions[c[0] as usize] + mesh.positions[c[1] as usize] + mesh.positions[c[2] as usize]) / 3.0
}

/// Every triangle put in group `of(its centre)`.
fn grouped_by(mesh: &mut Mesh, of: impl Fn(Vec3) -> u16) {
    let ids = (0..mesh.triangle_count()).map(|t| of(centre(mesh, t))).collect();
    mesh.set_group_ids(ids);
}

/// The ball in two groups: 1 where x is positive, 2 where it is not.
fn two_halves(tris: usize) -> Mesh {
    let mut mesh = sphere(tris);
    grouped_by(&mut mesh, |p| if p.x > 0.0 { 1 } else { 2 });
    mesh
}

/// A flat sheet of `n` by `n` unit quads, each cut in two as a quad remesh
/// leaves its faces.
fn quad_sheet(n: usize) -> Mesh {
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    let positions: Vec<Vec3> = (0..=n).flat_map(|j| (0..=n).map(move |i| Vec3::new(i as f32, j as f32, 0.0))).collect();
    let (mut triangles, mut quads) = (Vec::new(), Vec::new());
    for j in 0..n {
        for i in 0..n {
            let q = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            triangles.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
            quads.push(q);
        }
    }
    let (mut mesh, to_new) = Mesh::new_mapped(positions, triangles, None);
    let corners: Vec<u32> = quads.iter().flatten().map(|&v| to_new[v as usize]).collect();
    mesh.polys = Some(Polys { start: (0..=quads.len() as u32).map(|f| f * 4).collect(), indices: corners });
    mesh
}

/// Whether every quad of `mesh` has both its triangles in one group.
fn quads_are_whole(mesh: &Mesh) -> bool {
    let face_of = faces_of_triangles(mesh).expect("a quad mesh");
    let mut of_face: HashMap<u32, u16> = HashMap::new();
    (0..mesh.triangle_count())
        .all(|t| *of_face.entry(face_of[t]).or_insert(mesh.group_of(t as u32)) == mesh.group_of(t as u32))
}

/// The pairs of groups that share an edge.
fn touching_pairs(mesh: &Mesh) -> HashSet<(u16, u16)> {
    let mut by_edge: HashMap<(u32, u32), Vec<u16>> = HashMap::new();
    for (t, c) in mesh.indices.chunks_exact(3).enumerate() {
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            by_edge.entry((a.min(b), a.max(b))).or_default().push(mesh.group_of(t as u32));
        }
    }
    by_edge
        .values()
        .filter(|on| on.len() == 2 && on[0] != on[1])
        .map(|on| (on[0].min(on[1]), on[0].max(on[1])))
        .collect()
}

fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// Drags the pointer along `points` (pixels), a few samples per frame.
fn run_stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec2]) -> Option<StrokeDelta> {
    let sample = |i: usize| PointerSample { pos: points[i], pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, view, sample(0), false, false), "stroke missed the mesh");
    for i in 1..points.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    delta
}

/// The vertices a stroke changed, whether or not a sculpt layer recorded it.
fn changed(delta: &Option<StrokeDelta>) -> &[u32] {
    fn of(delta: &StrokeDelta) -> &[u32] {
        match delta {
            StrokeDelta::Positions { verts, .. } | StrokeDelta::Mask { verts, .. } => verts,
            StrokeDelta::Layered { stroke, .. } | StrokeDelta::Painted { shape: stroke, .. } => of(stroke),
            StrokeDelta::Paint(_) => &[],
        }
    }
    delta.as_ref().map_or(&[], of)
}

/// Gives `mesh` a sculpt layer that is recording, with ripples sculpted on
/// it all over, and returns the layer's number.
fn with_a_recording_layer(mesh: &mut Mesh) -> u32 {
    let (layer, _) = mesh.add_layer(Some("Ripples".into()));
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, v| Some(v.pos * (1.0 + 0.02 * (v.pos.y * 50.0).sin() * (v.pos.x * 37.0).cos())));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false).expect("the ripples are a stroke");
    let _ = mesh.take_dirty();
    layer
}

// ------------------------------------------------------------ the commands

#[test]
fn a_mesh_starts_without_groups_and_they_cost_it_nothing() {
    let mut mesh = sphere(4_000);
    assert!(!mesh.has_groups() && mesh.group_ids().is_empty() && mesh.group_count() == 0);
    assert_eq!(mesh.group_of(17), 0);
    let before = mesh.memory_bytes();
    // A list of nothing but zeros is no groups.
    assert!(mesh.set_group_ids(vec![0; mesh.triangle_count()]).is_empty());
    assert!(!mesh.has_groups());
    assert_eq!(mesh.memory_bytes(), before);
    grouped_by(&mut mesh, |p| if p.y > 0.0 { 3 } else { 0 });
    assert!(mesh.has_groups() && mesh.group_count() == 1);
    assert_eq!(mesh.memory_bytes(), before + mesh.triangle_count() * 2);
    assert_eq!(mesh.group_triangles(3, true).len() + mesh.group_triangles(3, false).len(), mesh.triangle_count());
}

#[test]
fn group_masked_takes_the_masked_surface_that_shows() {
    let mut mesh = sphere(20_000);
    assert!(masked(&mesh).unwrap_err().contains("masked"), "nothing masked is said, not done");
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = if mesh.positions[v].y > 0.2 { 1.0 } else { 0.0 };
    }
    // What is hidden is left alone, masked or not.
    let hidden: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|&t| centre(&mesh, t as usize).x > 0.3).collect();
    mesh.hide(&hidden);
    let made = masked(&mesh).unwrap();
    assert_eq!((made.group, made.groups), (1, 1));
    let before = mesh.set_group_ids(made.ids);
    assert!(before.is_empty());
    let mut inside = 0;
    for t in 0..mesh.triangle_count() {
        let c = &mesh.indices[t * 3..t * 3 + 3];
        let sum: f32 = c.iter().map(|&v| mesh.mask[v as usize]).sum();
        let want = sum >= 1.5 && !mesh.is_triangle_hidden(t as u32);
        assert_eq!(mesh.group_of(t as u32) == 1, want, "triangle {t}");
        inside += usize::from(want);
    }
    assert_eq!(inside, made.triangles);
    assert!(inside > 1000 && mesh.group_count() == 1);

    // A second group beside the first gets the next number, and the first keeps its faces.
    mesh.show_all();
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = if mesh.positions[v].y < -0.2 { 1.0 } else { 0.0 };
    }
    let second = masked(&mesh).unwrap();
    assert_eq!(second.group, 2);
    mesh.set_group_ids(second.ids);
    assert_eq!(mesh.group_count(), 2);
    assert_eq!(mesh.group_triangles(1, true).len(), inside);
}

#[test]
fn group_visible_takes_what_shows_and_the_whole_object_when_nothing_is_hidden() {
    let mut mesh = sphere(8_000);
    let all = visible(&mesh).unwrap();
    assert_eq!(all.triangles, mesh.triangle_count());
    mesh.set_group_ids(all.ids);
    assert!(mesh.group_ids().iter().all(|&id| id == 1));

    let low: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|&t| centre(&mesh, t as usize).y < 0.0).collect();
    mesh.hide(&low);
    let top = visible(&mesh).unwrap();
    assert_eq!(top.group, 2, "the hidden half still has group 1, so the top is a new one");
    assert_eq!(top.triangles, mesh.triangle_count() - low.len());
    mesh.set_group_ids(top.ids);
    assert!(low.iter().all(|&t| mesh.group_of(t) == 1));
    assert_eq!(mesh.group_triangles(2, true).len(), top.triangles);

    // Everything hidden: nothing to group.
    let rest: Vec<u32> = (0..mesh.triangle_count() as u32).collect();
    mesh.hide(&rest);
    assert!(visible(&mesh).is_err() && masked(&mesh).is_err() && parts(&mesh).is_err());
}

/// Two balls and a cube in one mesh, not touching.
fn three_parts() -> Mesh {
    let (a, b, c) = (sphere(3_000), sphere(1_200), primitives::build(Primitive::Cube, 600, 0.3));
    let (mut positions, mut indices) = (Vec::new(), Vec::new());
    for (part, shift) in [(&a, Vec3::ZERO), (&b, Vec3::new(1.5, 0.0, 0.0)), (&c, Vec3::new(0.0, 1.5, 0.0))] {
        let base = positions.len() as u32;
        positions.extend(part.positions.iter().map(|&p| p + shift));
        indices.extend(part.indices.iter().map(|&i| i + base));
    }
    Mesh::new(positions, indices, None)
}

#[test]
fn auto_groups_gives_each_connected_part_its_own_group() {
    let mut mesh = three_parts();
    let made = parts(&mesh).unwrap();
    assert_eq!(made.groups, 3);
    assert_eq!(made.triangles, mesh.triangle_count());
    mesh.set_group_ids(made.ids);
    assert_eq!(mesh.group_count(), 3);
    // Each part is one group, whole: the group is settled by where the triangle is.
    let which = |p: Vec3| {
        if p.x > 0.9 {
            1
        } else if p.y > 0.9 {
            2
        } else {
            0
        }
    };
    let mut of_part: [Option<u16>; 3] = [None; 3];
    for t in 0..mesh.triangle_count() {
        let slot = &mut of_part[which(centre(&mesh, t))];
        assert_eq!(*slot.get_or_insert(mesh.group_of(t as u32)), mesh.group_of(t as u32));
    }
    let ids: HashSet<u16> = of_part.iter().map(|id| id.unwrap()).collect();
    assert_eq!(ids.len(), 3);
    assert!(touching_pairs(&mesh).is_empty(), "no two parts share an edge");

    // One piece is one group, and the same answer every time.
    let ball = sphere(2_000);
    let (once, again) = (parts(&ball).unwrap(), parts(&ball).unwrap());
    assert_eq!((once.groups, &once.ids), (1, &again.ids));
}

#[test]
fn groups_by_angle_splits_a_cube_into_its_six_sides() {
    let mut cube = primitives::build(Primitive::Cube, 6_000, 0.5);
    let made = by_angle(&cube, 30.0).unwrap();
    cube.set_group_ids(made.ids);
    // Every triangle of a side is in that side's group, and no other side shares it.
    let side = |n: Vec3| {
        let a = n.abs();
        let axis = if a.x > a.y && a.x > a.z {
            0
        } else if a.y > a.z {
            1
        } else {
            2
        };
        axis * 2 + usize::from(n[axis] > 0.0)
    };
    let normals = super::make::face_normals(&cube);
    let flat = |t: usize| normals[t].abs().max_element() > 0.999;
    let mut of_side: [Option<u16>; 6] = [None; 6];
    for t in (0..cube.triangle_count()).filter(|&t| flat(t)) {
        let slot = &mut of_side[side(normals[t])];
        assert_eq!(*slot.get_or_insert(cube.group_of(t as u32)), cube.group_of(t as u32), "triangle {t}");
    }
    let ids: HashSet<u16> = of_side.iter().map(|id| id.expect("every side has flat triangles")).collect();
    assert_eq!(ids.len(), 6, "six sides, six groups");

    // Groups that touch are not near each other in colour.
    let pairs = touching_pairs(&cube);
    assert!(pairs.len() >= 12, "{} pairs of sides touch", pairs.len());
    for &(a, b) in pairs.iter().filter(|(a, b)| ids.contains(a) && ids.contains(b)) {
        assert!(hue_gap(a, b) >= 0.1, "groups {a} and {b} touch and are {} apart", hue_gap(a, b));
    }
    // An angle no edge of the cube reaches leaves it one group; the ball is one at any sensible angle.
    assert_eq!(by_angle(&cube, 120.0).unwrap().groups, 1);
    assert_eq!(by_angle(&sphere(4_000), 45.0).unwrap().groups, 1);
}

#[test]
fn merge_moves_a_whole_group_and_clear_takes_them_all_away() {
    let mut mesh = sphere(6_000);
    grouped_by(&mut mesh, |p| {
        if p.y > 0.2 {
            5
        } else if p.y < -0.2 {
            9
        } else {
            0
        }
    });
    let (five, nine) = (mesh.group_triangles(5, true).len(), mesh.group_triangles(9, true).len());
    assert!(merged(&mesh, 5, 5).is_err() && merged(&mesh, 0, 5).is_err() && merged(&mesh, 5, 0).is_err());
    assert!(merged(&mesh, 7, 5).is_err(), "a group that does not exist");
    let made = merged(&mesh, 5, 9).unwrap();
    assert_eq!((made.group, made.triangles), (9, five));
    mesh.set_group_ids(made.ids);
    assert_eq!(mesh.group_count(), 1);
    assert_eq!(mesh.group_triangles(9, true).len(), five + nine);
    // Clear Groups is an empty list.
    let before = mesh.set_group_ids(Vec::new());
    assert!(!mesh.has_groups() && before.len() == mesh.triangle_count());
}

#[test]
fn a_new_group_takes_the_lowest_free_number_clear_of_its_neighbours_colours() {
    // Band after band up a ball, each made by Group Masked: every band touches the one before.
    let mut mesh = sphere(30_000);
    for band in 0..12 {
        let (low, high) = (-0.5 + band as f32 / 12.0, -0.5 + (band + 1) as f32 / 12.0);
        for v in 0..mesh.vertex_count() {
            let y = mesh.positions[v].y;
            mesh.mask[v] = if y >= low && y < high + 0.02 { 1.0 } else { 0.0 };
        }
        // Only what no band has yet: the masked faces still in no group.
        let taken: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|&t| mesh.group_of(t) != 0).collect();
        mesh.hide(&taken);
        let made = masked(&mesh).unwrap();
        mesh.show_all();
        mesh.set_group_ids(made.ids);
    }
    assert_eq!(mesh.group_count(), 12);
    for (a, b) in touching_pairs(&mesh) {
        assert!(a == 0 || hue_gap(a, b) >= 0.1, "bands {a} and {b} touch and are {} apart", hue_gap(a, b));
    }
    // The numbers stay small: no more than a few are passed over for colour.
    assert!(mesh.group_ids().iter().all(|&id| id <= 24), "{:?}", mesh.group_ids().iter().max());
    // A colour is a matter of the number alone, and is a colour.
    for id in 1..200u16 {
        let c = colour(id);
        assert_eq!(c, colour(id));
        assert!(c.iter().all(|&x| (0.0..=1.0).contains(&x)) && c.iter().cloned().fold(0.0, f32::max) > 0.8, "{c:?}");
    }
    assert!((1..60u16).all(|id| hue_gap(id, id + 1) > 0.3), "consecutive numbers are far apart in hue");
}

#[test]
fn a_quads_two_triangles_are_always_in_one_group() {
    let mut sheet = quad_sheet(24);
    // A mask whose edge runs through the quads on a slant.
    for v in 0..sheet.vertex_count() {
        let p = sheet.positions[v];
        sheet.mask[v] = if p.x + 0.37 * p.y < 14.2 { 1.0 } else { 0.0 };
    }
    let made = masked(&sheet).unwrap();
    assert!(made.triangles % 2 == 0);
    sheet.set_group_ids(made.ids);
    assert!(quads_are_whole(&sheet) && sheet.group_count() == 1);

    // Any list put on the mesh is settled the same way.
    grouped_by(&mut sheet, |p| if p.x + 0.61 * p.y < 17.3 { 4 } else { 6 });
    assert!(quads_are_whole(&sheet) && sheet.group_count() == 2);

    // Groups by Angle on a folded sheet: the fold splits it, a quad's own diagonal never does.
    let mut folded = quad_sheet(12);
    for v in 0..folded.vertex_count() {
        let p = folded.positions[v];
        // Each quad is bent a little along its diagonal as well.
        folded.positions[v] =
            Vec3::new(p.x, p.y, if p.x > 6.0 { (p.x - 6.0) * 2.0 } else { 0.0 } + 0.2 * ((p.x + p.y) % 2.0));
    }
    let folded_ids = by_angle(&folded, 40.0).unwrap();
    folded.set_group_ids(folded_ids.ids);
    assert!(quads_are_whole(&folded));
    assert!(folded.group_count() >= 2);
}

// ------------------------------------------------------------------- undo

#[test]
fn every_group_command_is_one_undo_step_that_puts_back_exactly_what_was_there() {
    let mut mesh = sphere(12_000);
    let mut history = History::default();
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = if mesh.positions[v].z > 0.1 { 1.0 } else { 0.0 };
    }
    let state =
        |mesh: &Mesh| (mesh.group_ids().to_vec(), mesh.mask.clone(), mesh.hidden_triangles().map(<[bool]>::to_vec));
    let mut states = vec![state(&mesh)];

    // Group Masked, clearing the mask in the same step.
    let made = masked(&mesh).unwrap();
    let before = mesh.set_group_ids(made.ids);
    let cleared = mesh.remap_mask(|_, _, _| 0.0);
    assert!(cleared.is_some());
    history.push_groups(GroupsStep::groups(before, cleared));
    states.push(state(&mesh));
    assert!(mesh.mask.iter().all(|&m| m == 0.0) && mesh.group_count() == 1);

    // Groups by Angle over it, at an angle so tight that the ball comes apart.
    let made = by_angle(&mesh, 0.5).unwrap();
    assert!(made.groups > 50, "a tight angle cuts a ball into many: {}", made.groups);
    let before = mesh.set_group_ids(made.ids);
    history.push_groups(GroupsStep::groups(before, None));
    states.push(state(&mesh));

    // Isolate: everything but one group hidden, as one step.
    let group = mesh.group_of(0);
    let shown = mesh.hidden_triangles().map(<[bool]>::to_vec);
    let flags: Vec<bool> = mesh.group_ids().iter().map(|&id| id != group).collect();
    assert!(mesh.restore_hidden_triangles(&flags));
    history.push_groups(GroupsStep::hidden(shown.as_deref()));
    states.push(state(&mesh));
    assert!(mesh.any_hidden());

    // Clear Groups.
    let before = mesh.set_group_ids(Vec::new());
    history.push_groups(GroupsStep::groups(before, None));
    states.push(state(&mesh));

    assert_eq!(history.steps(), (4, 0));
    for back in (0..4).rev() {
        assert_eq!(history.undo(&mut mesh), Some(false), "the mesh itself is not swapped");
        assert!(state(&mesh) == states[back], "undo to state {back}");
    }
    assert_eq!(history.undo(&mut mesh), None);
    for forward in 1..=4 {
        assert_eq!(history.redo(&mut mesh), Some(false));
        assert!(state(&mesh) == states[forward], "redo to state {forward}");
    }
    // And back once more, to be sure a step can be crossed any number of times.
    for back in (0..4).rev() {
        history.undo(&mut mesh);
        assert!(state(&mesh) == states[back]);
    }
}

// ------------------------------------------------------ following the mesh

#[test]
fn dynamic_topology_keeps_new_triangles_in_the_group_they_were_split_from() {
    let mut mesh = two_halves(6_000);
    let (before_ids, before_count) = (mesh.group_ids().to_vec(), mesh.triangle_count());
    let mut history = History::default();
    // Refined right on the border, at the front of the ball.
    mesh.begin_stroke();
    let stats = mesh.refine(&[Vec3::new(0.0, 0.0, 0.5)], 0.2, 0.008, usize::MAX, 10_000_000);
    let delta = mesh.end_stroke(false).expect("the refine is a stroke");
    assert!(mesh.triangle_count() > before_count + 500, "{stats:?}");
    assert_eq!(mesh.group_ids().len(), mesh.triangle_count());
    // The border is where it was: every triangle is on its own side of the plane it was cut along.
    // (The seam's own triangles straddled x = 0 from the start, so they are judged by which group they began in.)
    let edge = 0.5 * 6.3 / (6_000f32).sqrt();
    for t in 0..mesh.triangle_count() {
        let (x, id) = (centre(&mesh, t).x, mesh.group_of(t as u32));
        assert!(id == 1 || id == 2, "triangle {t} is in group {id}");
        assert!(x.abs() < edge || (id == 1) == (x > 0.0), "triangle {t} at x = {x} is in group {id}");
    }
    assert_eq!(mesh.group_count(), 2);
    let after_ids = mesh.group_ids().to_vec();
    assert_eq!(&after_ids[..before_count], &before_ids[..], "no triangle from before changed group");

    // Undo takes the new triangles' groups away with them; redo brings back the same.
    history.push_stroke(delta);
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert_eq!(mesh.group_ids(), &before_ids[..]);
    assert_eq!(history.redo(&mut mesh), Some(false));
    assert_eq!(mesh.group_ids(), &after_ids[..]);

    // A mesh without groups grows none.
    let mut plain = sphere(6_000);
    plain.begin_stroke();
    plain.refine(&[Vec3::new(0.0, 0.0, 0.5)], 0.2, 0.008, usize::MAX, 10_000_000);
    plain.end_stroke(false);
    assert!(!plain.has_groups() && plain.group_ids().is_empty());
}

#[test]
fn groups_follow_a_change_of_subdivision_level_and_a_round_trip_changes_nothing() {
    for quads in [false, true] {
        let base = if quads { quad_sheet(10) } else { sphere(1_500) };
        let top = subdivided(&subdivided(&base, Split::Smooth).unwrap().0, Split::Smooth).unwrap().0;
        let mut top = top;
        // A border that runs across the coarse faces, not along their edges.
        let cut = if quads { 4.3 } else { 0.07 };
        grouped_by(&mut top, |p| if p.x + 0.3 * p.y > cut { 3 } else { 8 });
        let print = top.levels.as_ref().unwrap().print(&top);
        let ordered = |mesh: &Mesh| mesh.levels.as_ref().unwrap().ordered_groups(mesh);
        let fine = ordered(&top);

        // Down two levels: every coarse face is in the group most of its part of the fine level is.
        let (low, _) = stepped(&top, 0).unwrap();
        assert_eq!(low.group_count(), 2);
        let coarse = ordered(&low);
        assert_eq!(coarse.len() * 16, fine.len());
        if !quads {
            // Loop levels number a face's four children together.
            for (face, &id) in coarse.iter().enumerate() {
                let children = &fine[face * 16..face * 16 + 16];
                // Two steps of four children each: thirteen of the sixteen always carry it.
                let threes = children.iter().filter(|&&c| c == 3).count();
                assert!(id == 3 || id == 8);
                assert!(threes < 13 || id == 3, "face {face}: {threes} of 16 are group 3, and it is in {id}");
                assert!(threes > 3 || id == 8, "face {face}: {threes} of 16 are group 3, and it is in {id}");
            }
        } else {
            assert!(quads_are_whole(&low));
        }

        // And back up: exactly the groups it left with, not the coarse border's stair steps.
        let (back, _) = stepped(&low, 2).unwrap();
        assert_eq!(ordered(&back), fine);
        assert_eq!(back.levels.as_ref().unwrap().print(&back), print);
        // By way of the middle level too, and down and up again.
        let (mid, _) = stepped(&back, 1).unwrap();
        let (low2, _) = stepped(&mid, 0).unwrap();
        let (mid2, _) = stepped(&low2, 1).unwrap();
        assert_eq!(ordered(&mid2), ordered(&mid));
        let (back2, _) = stepped(&mid2, 2).unwrap();
        assert_eq!(ordered(&back2), fine);

        // Groups changed on the coarse level are what the finer levels get: each face its parent's.
        let mut changed = stepped(&back2, 0).unwrap().0;
        grouped_by(&mut changed, |p| if p.y > cut { 11 } else { 12 });
        let coarse = ordered(&changed);
        let (up, _) = stepped(&changed, 2).unwrap();
        let fine_now = ordered(&up);
        if !quads {
            assert!(fine_now.chunks_exact(16).zip(&coarse).all(|(children, id)| children.iter().all(|c| c == id)));
        }
        assert_eq!(up.group_count(), 2);
        assert!(fine_now.iter().all(|&id| id == 11 || id == 12));

        // Subdividing gives the new level its parents' groups, and a mesh without groups stays without.
        let (finer, _) = subdivided(&up, Split::Smooth).unwrap();
        assert_eq!(finer.group_triangles(11, true).len(), up.group_triangles(11, true).len() * 4);
        let plain = subdivided(&base, Split::Smooth).unwrap().0;
        assert!(!plain.has_groups() && !stepped(&plain, 0).unwrap().0.has_groups());
    }
}

/// How far from the plane x = 0 the border between groups 1 and 2 runs on
/// `mesh`, at most, and the longest edge among the triangles along it.
fn border_offset(mesh: &Mesh) -> (f32, f32) {
    let mut by_edge: HashMap<(u32, u32), Vec<u16>> = HashMap::new();
    for (t, c) in mesh.indices.chunks_exact(3).enumerate() {
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            by_edge.entry((a.min(b), a.max(b))).or_default().push(mesh.group_of(t as u32));
        }
    }
    let (mut worst, mut longest, mut edges) = (0f32, 0f32, 0);
    for (&(a, b), on) in &by_edge {
        if on.len() == 2 && on[0] != on[1] {
            let (p, q) = (mesh.positions[a as usize], mesh.positions[b as usize]);
            worst = worst.max(p.x.abs()).max(q.x.abs());
            longest = longest.max(p.distance(q));
            edges += 1;
        }
    }
    assert!(edges > 20, "the two groups share a border ({edges} edges)");
    (worst, longest)
}

#[test]
fn a_rebuilt_mesh_takes_its_groups_from_the_nearest_old_face() {
    let old = two_halves(20_000);
    // Other triangles on the same shape: the same ball, built finer and coarser, and turned
    // so that no triangle of the new lies along one of the old.
    for tris in [9_000, 50_000] {
        let turn = glam::Quat::from_rotation_y(0.4) * glam::Quat::from_rotation_x(0.3);
        let other = sphere(tris);
        let mut new = Mesh::new(other.positions.iter().map(|&p| turn * p).collect(), other.indices.clone(), None);
        assert_eq!(carry(&mut new, &[Source::of(&old)], Fit::Whole), Carried::Kept(2));
        let (worst, longest) = border_offset(&new);
        assert!(worst <= longest, "the border is {worst} from where it was; a triangle there is {longest} across");
        // Each side is its own group, with no speck of the other in it.
        for t in 0..new.triangle_count() {
            let x = centre(&new, t).x;
            assert!(x.abs() <= longest || (new.group_of(t as u32) == 1) == (x > 0.0), "triangle {t} at x = {x}");
        }
        let ones = new.group_triangles(1, true).len() as f32 / new.triangle_count() as f32;
        assert!((ones - 0.5).abs() < 0.03, "{ones} of the ball is group 1");
    }

    // A mesh without groups hands none on, and one that is somewhere else altogether keeps none.
    let mut new = sphere(5_000);
    assert_eq!(carry(&mut new, &[Source::of(&sphere(3_000))], Fit::Whole), Carried::Nothing);
    assert!(!new.has_groups());
    let far = sphere(5_000);
    let mut elsewhere = Mesh::new(
        far.positions.iter().map(|&p| p * 0.2 + Vec3::new(3.0, 0.0, 0.0)).collect(),
        far.indices.clone(),
        None,
    );
    assert_eq!(carry(&mut elsewhere, &[Source::of(&old)], Fit::Whole), Carried::Dropped);
    assert!(!elsewhere.has_groups());
    // The same ball with a horn pulled out of it is still the same model: the horn is in the group it grew from.
    let ball = sphere(8_000);
    let horned: Vec<Vec3> =
        ball.positions.iter().map(|&p| if p.x > 0.4 { p * (1.0 + (p.x - 0.4) * 12.0) } else { p }).collect();
    let mut horn = Mesh::new(horned, ball.indices.clone(), None);
    assert_eq!(carry(&mut horn, &[Source::of(&old)], Fit::Whole), Carried::Kept(2));
    assert!((0..horn.triangle_count()).filter(|&t| centre(&horn, t).x > 0.45).all(|t| horn.group_of(t as u32) == 1));
}

#[test]
fn a_piece_keeps_the_groups_of_the_old_surface_it_is_and_new_surface_is_in_none() {
    let old = two_halves(8_000);
    // The top of the ball cut off and closed with a flat cap, and a copy of the ball standing beside it.
    let top: Vec<u32> = (0..old.triangle_count())
        .filter(|&t| centre(&old, t).y > 0.1)
        .flat_map(|t| old.indices[t * 3..t * 3 + 3].to_vec())
        .collect();
    let mut positions = old.positions.clone();
    let hub = positions.len() as u32;
    positions.push(Vec3::new(0.0, 0.1, 0.0));
    let mut rim: HashMap<(u32, u32), i32> = HashMap::new();
    for t in top.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *rim.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
        }
    }
    let mut indices = top.clone();
    let cap_from = indices.len() / 3;
    for (&(a, b), &way) in rim.iter().filter(|(_, way)| **way != 0) {
        indices.extend_from_slice(&if way > 0 { [b, a, hub] } else { [a, b, hub] });
    }
    let caps = indices.len() / 3 - cap_from;
    assert!(caps > 20);
    let base = positions.len() as u32;
    positions.extend(old.positions.iter().map(|&p| p + Vec3::new(2.0, 0.0, 0.0)));
    indices.extend(old.indices.iter().map(|&i| i + base));
    let mut piece = Mesh::new(positions, indices, None);
    assert_eq!(carry(&mut piece, &[Source::of(&old)], Fit::Part), Carried::Kept(2));
    let (mut skin, mut cap, mut copy) = (0, 0, 0);
    for t in 0..piece.triangle_count() {
        let (p, id) = (centre(&piece, t), piece.group_of(t as u32));
        if p.x > 1.0 {
            assert_eq!(id, 0, "the copy beside it is not the old surface");
            copy += 1;
        } else if p.length() < 0.45 {
            assert_eq!(id, 0, "the cap over the cut is new surface");
            cap += 1;
        } else {
            assert_eq!(id, if p.x > 0.0 { 1 } else { 2 }, "triangle {t} at {p}");
            skin += 1;
        }
    }
    assert!(skin > 1000 && cap > 10 && copy == old.triangle_count(), "{skin} {cap} {copy}");
    // As a rebuild of the whole shape the same mesh would not be the old shape at all.
    let mut whole = Mesh::new(piece.positions.clone(), piece.indices.clone(), None);
    assert_eq!(carry(&mut whole, &[Source::of(&old)], Fit::Whole), Carried::Dropped);
}

#[test]
fn a_lone_triangle_in_the_wrong_group_joins_its_neighbours() {
    let mut mesh = two_halves(4_000);
    let mut ids = mesh.group_ids().to_vec();
    // A speck well inside each half.
    let inside = |want: u16| (0..ids.len()).find(|&t| ids[t] == want && centre(&mesh, t).x.abs() > 0.3).unwrap();
    let (a, b) = (inside(1), inside(2));
    (ids[a], ids[b]) = (2, 1);
    let wanted = mesh.group_ids().to_vec();
    super::transfer::tidy_specks(&mesh, &mut ids);
    assert_eq!(ids, wanted);
    // The border itself is not a speck and stays.
    mesh.set_group_ids(ids.clone());
    super::transfer::tidy_specks(&mesh, &mut ids);
    assert_eq!(ids, wanted);
}

#[test]
fn a_cut_and_mirrored_side_takes_the_groups_of_the_side_it_is_the_image_of() {
    // Groups that are not the same on the two sides: bands across +X, one group on -X.
    let mut old = sphere(12_000);
    grouped_by(&mut old, |p| {
        if p.x <= 0.0 {
            9
        } else if p.y > 0.0 {
            1
        } else {
            2
        }
    });
    // The +X side kept and mirrored: here simply the same ball again, which is its own mirror image.
    let mut new = sphere(12_000);
    let source = Source { fold: Some((0, true)), ..Source::of(&old) };
    assert_eq!(carry(&mut new, &[source], Fit::Whole), Carried::Kept(2));
    let edge = 0.5 * 6.3 / (12_000f32).sqrt();
    for t in 0..new.triangle_count() {
        let p = centre(&new, t);
        if p.y.abs() > edge && p.x.abs() > edge {
            assert_eq!(new.group_of(t as u32), if p.y > 0.0 { 1 } else { 2 }, "triangle {t} at {p}");
        }
    }
    // Without the fold the far side would have kept the group that was thrown away.
    let mut plain = sphere(12_000);
    carry(&mut plain, &[Source::of(&old)], Fit::Whole);
    assert_eq!(plain.group_count(), 3);
}

#[test]
fn joined_meshes_keep_their_groups_apart() {
    let (mut a, mut b, c) = (sphere(2_000), sphere(2_000), sphere(2_000));
    grouped_by(&mut a, |p| if p.y > 0.0 { 1 } else { 2 });
    grouped_by(&mut b, |p| if p.y > 0.0 { 2 } else { 7 });
    let apart = kept_apart(&[&a, &b, &c]);
    assert_eq!(apart[0], a.group_ids(), "the first keeps its numbers");
    assert!(apart[2].is_empty(), "a mesh without groups gets none");
    let of_b: HashSet<u16> = apart[1].iter().copied().collect();
    assert_eq!(of_b, HashSet::from([3, 7]), "2 was taken, so it became the lowest free number; 7 was free and stays");
    // The same faces are still together.
    assert!(b.group_ids().iter().zip(&apart[1]).all(|(&was, &now)| (was == 2) == (now == 3)));
}

#[test]
fn every_copy_of_an_array_made_real_is_in_the_groups_of_the_mesh_it_copies() {
    let old = two_halves(4_000);
    // As the array tool lays them out: the mesh itself, a copy moved and turned, and a
    // mirrored one with its faces wound the other way.
    let turn = glam::Quat::from_rotation_z(0.7);
    let places: [(&dyn Fn(Vec3) -> Vec3, bool); 3] = [
        (&|p| p, false),
        (&|p| turn * p * 1.5 + Vec3::new(3.0, 0.0, 0.0), false),
        (&|p| Vec3::new(-p.x, p.y, p.z) + Vec3::new(0.0, 4.0, 0.0), true),
    ];
    let (mut positions, mut indices) = (Vec::new(), Vec::new());
    for (place, flipped) in places {
        let base = positions.len() as u32;
        positions.extend(old.positions.iter().map(|&p| place(p)));
        for t in old.indices.chunks_exact(3) {
            let (b, c) = if flipped { (t[2], t[1]) } else { (t[1], t[2]) };
            indices.extend_from_slice(&[t[0] + base, b + base, c + base]);
        }
    }
    let (mut made, numbering) = Mesh::new_numbered(positions.clone(), indices.clone(), None, &[]);
    let ids = copies(&old, &made, &numbering.triangle).expect("three copies of the mesh");
    made.set_group_ids(ids);
    assert_eq!(made.group_count(), 2);
    assert_eq!(made.group_triangles(1, true).len(), old.group_triangles(1, true).len() * 3);
    for t in 0..made.triangle_count() {
        let p = centre(&made, t);
        // Where the face is on the mesh it is a copy of.
        let own = if p.y > 2.0 {
            Vec3::new(-p.x, p.y - 4.0, p.z)
        } else if p.x > 1.5 {
            turn.inverse() * ((p - Vec3::new(3.0, 0.0, 0.0)) / 1.5)
        } else {
            p
        };
        assert_eq!(made.group_of(t as u32), if own.x > 0.0 { 1 } else { 2 }, "the face at {p}");
    }

    // Not copies: another mesh that has twice the triangles and vertices, the first of them elsewhere.
    let moved: Vec<Vec3> = positions.iter().map(|&p| p + Vec3::splat(0.001)).collect();
    let two = old.indices.len() * 2;
    let (other, numbering) =
        Mesh::new_numbered(moved[..old.vertex_count() * 2].to_vec(), indices[..two].to_vec(), None, &[]);
    assert!(copies(&old, &other, &numbering.triangle).is_none());
    // Nor the mesh by itself, nor a part of a copy.
    let (alone, numbering) = Mesh::new_numbered(old.positions.clone(), old.indices.clone(), None, &[]);
    assert!(copies(&old, &alone, &numbering.triangle).is_none());
    let most = indices.len() - 3;
    let (short, numbering) = Mesh::new_numbered(positions.clone(), indices[..most].to_vec(), None, &[]);
    assert!(copies(&old, &short, &numbering.triangle).is_none());
}

#[test]
fn a_copy_of_a_mesh_takes_its_groups_through_the_renumbering() {
    let mesh = two_halves(5_000);
    // As a file or a copy hands it over: the triangles turned back to front.
    let indices: Vec<u32> = mesh.indices.chunks_exact(3).rev().flatten().copied().collect();
    let ids: Vec<u16> = mesh.group_ids().iter().rev().copied().collect();
    let (mut copy, numbering) = Mesh::new_numbered(mesh.positions.clone(), indices, None, &[]);
    copy.set_group_ids(renumbered(&ids, &numbering.triangle));
    assert_eq!(copy.group_count(), 2);
    assert!((0..copy.triangle_count()).all(|t| (copy.group_of(t as u32) == 1) == (centre(&copy, t).x > 0.0)));
    assert!(renumbered(&[], &numbering.triangle).is_empty());
    assert!(renumbered(&ids[1..], &numbering.triangle).is_empty(), "a list for another mesh is not guessed at");
}

// ----------------------------------------------------------- Stay in Group

/// The vertices with no face in group 1: what a stroke kept to group 1 must not touch.
fn outside_one(mesh: &Mesh) -> Vec<bool> {
    mesh.group_vertices(1).iter().map(|&on| !on).collect()
}

#[test]
fn the_fence_is_the_groups_own_faces_and_the_border_is_on_both_sides() {
    let mut mesh = two_halves(8_000);
    assert!(mesh.hidden_vertices().is_none() && !mesh.stays_in_groups());
    mesh.stay_in_groups(&[1]);
    assert!(mesh.stays_in_groups());
    let outside = outside_one(&mesh);
    assert_eq!(mesh.hidden_vertices().unwrap(), &outside[..]);
    // The border's vertices have faces on both sides, so they are inside for either group.
    let in_two: Vec<bool> = mesh.group_vertices(2);
    let border = outside.iter().zip(&in_two).filter(|(out, two)| !**out && **two).count();
    assert!(border > 40, "{border} vertices are on the border");
    // Nothing is hidden by it: what is drawn and what the pointer hits is as before.
    assert!(!mesh.any_hidden() && mesh.hidden_triangles().is_none());
    assert_eq!(mesh.visible_indices().len(), mesh.indices.len());
    assert!(mesh.raycast(Vec3::new(-3.0, 0.0, 0.0), Vec3::X).is_some(), "the far group still stops the pointer");

    // The stroke loops pass over what is outside, and the end of the stroke takes the fence down.
    let leaves: Vec<u32> = (0..mesh.bvh.leaves.len() as u32).collect();
    let rest = mesh.positions.clone();
    mesh.begin_stroke();
    mesh.move_vertices(&leaves, |_, v| Some(v.pos + Vec3::Y));
    for v in 0..mesh.vertex_count() {
        assert_eq!(mesh.positions[v] == rest[v], outside[v], "vertex {v}");
    }
    let delta = mesh.end_stroke(false).unwrap();
    assert!(!mesh.stays_in_groups() && mesh.hidden_vertices().is_none());
    mesh.apply_delta(&delta, true);
    assert_eq!(mesh.positions, rest);

    // With part of the model hidden as well, both are off limits; and a stroke that never began lets go.
    let low: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|&t| centre(&mesh, t as usize).y < -0.1).collect();
    mesh.hide(&low);
    let hidden = mesh.hidden_vertices().unwrap().to_vec();
    mesh.stay_in_groups(&[1]);
    let off = mesh.hidden_vertices().unwrap().to_vec();
    assert!(hidden.iter().zip(&off).all(|(h, o)| !*h || *o), "what is hidden stays off limits");
    assert!(outside.iter().zip(&off).all(|(out, o)| !*out || *o), "and so does what is outside");
    assert!(off.iter().filter(|o| **o).count() > hidden.iter().filter(|h| **h).count());
    mesh.leave_groups();
    assert_eq!(mesh.hidden_vertices().unwrap(), &hidden[..]);
    // Group 0, the surface in no group, can be stayed in like any other.
    let mut partly = sphere(4_000);
    grouped_by(&mut partly, |p| if p.x > 0.0 { 1 } else { 0 });
    partly.stay_in_groups(&[0]);
    let off = partly.hidden_vertices().unwrap().to_vec();
    assert!((0..partly.vertex_count()).all(|v| !off[v] || partly.positions[v].x > 0.0));
    partly.leave_groups();
}

#[test]
fn every_brush_kept_to_a_group_leaves_the_rest_of_the_model_alone() {
    // The stroke runs down the front just right of the border, with a brush wide enough to
    // reach well over it; Move and Snake Hook drag across it instead.
    let middle = SIZE * 0.5;
    let down: Vec<Vec2> = (0..=40).map(|i| middle + Vec2::new(14.0, -90.0 + 180.0 * i as f32 / 40.0)).collect();
    let across: Vec<Vec2> = (0..=40).map(|i| middle + Vec2::new(30.0 - 160.0 * i as f32 / 40.0, 10.0)).collect();
    let mut checked = 0;
    let brushes = || Brush::ALL.into_iter().map(|b| (b, false)).chain([(Brush::Standard, true)]);
    // Every brush twice: on a plain mesh, and on one with a sculpt layer recording, which is
    // where a stroke is written down by another road and where Erase Layer has something to rub out.
    for ((brush, mask_mode), layered) in brushes().map(|b| (b, false)).chain(brushes().map(|b| (b, true))) {
        // Paint moves nothing: it is held to its group in `sculpt/paint/tests.rs`.
        if brush == Brush::Paint && !mask_mode {
            continue;
        }
        let kind = if mask_mode { "Mask" } else { brush.name() };
        let what = format!("{kind}{}", if layered { " on a layer" } else { "" });
        let dragged = matches!(kind, "Move" | "Snake Hook" | "Nudge" | "Pose");
        let points = if dragged { &across } else { &down };
        let mut mesh = two_halves(80_000);
        let layer = layered.then(|| with_a_recording_layer(&mut mesh));
        let offsets = |mesh: &Mesh| layer.map(|id| mesh.layers.as_ref().unwrap().offsets(id).unwrap().to_vec());
        let view = view_of(&mesh);
        let outside = outside_one(&mesh);
        let mut sculptor = Sculptor::default();
        sculptor.settings.brush = brush;
        sculptor.settings.mask_mode = mask_mode;
        sculptor.settings.symmetry = [false; 3];
        sculptor.settings.radius_px = 70.0;
        sculptor.settings.per_brush[brush.index()].strength = 0.8;

        // Free, the stroke does reach the other group: otherwise the rest of the test shows nothing.
        let free = run_stroke(&mut mesh, &mut sculptor, &view, points);
        // Erase Layer takes a sculpt layer away, and a mesh without one gives it nothing to do
        // and so nothing to be held back from. It is the one brush that may sit a round out.
        if brush == Brush::EraseLayer && !layered && !mask_mode {
            assert!(changed(&free).is_empty(), "Erase Layer moved a mesh that has no layer");
            continue;
        }
        checked += 1;
        let reached = changed(&free).iter().filter(|&&v| outside[v as usize]).count();
        assert!(reached > 20, "{what}: the free stroke only reached {reached} vertices of the other group");
        mesh.apply_delta(free.as_ref().unwrap(), true);
        let _ = mesh.take_dirty();

        let (rest, rest_mask, rest_offsets) = (mesh.positions.clone(), mesh.mask.clone(), offsets(&mesh));
        mesh.stay_in_groups(&[1]);
        let delta = run_stroke(&mut mesh, &mut sculptor, &view, points);
        assert!(!mesh.stays_in_groups(), "{what}: the fence comes down with the stroke");
        let verts = changed(&delta);
        assert!(verts.len() > 50, "{what}: the stroke still works inside the group ({})", verts.len());
        assert!(
            verts.iter().all(|&v| !outside[v as usize]),
            "{what}: a vertex outside the group is in the undo record"
        );
        for v in 0..mesh.vertex_count() {
            if outside[v] {
                assert_eq!(mesh.positions[v], rest[v], "{what}: vertex {v} outside the group moved");
                assert_eq!(mesh.mask[v], rest_mask[v], "{what}: vertex {v} outside the group was masked");
            }
        }
        // And what the layer keeps of the surface outside is what it kept before.
        if let (Some(was), Some(now)) = (rest_offsets, offsets(&mesh)) {
            assert!(
                (0..was.len()).all(|v| !outside[v] || was[v] == now[v]),
                "{what}: the layer changed outside the group"
            );
        }
    }
    // Every brush and the mask on both meshes, less Erase Layer's one round out and Paint's two.
    assert_eq!(checked, (Brush::ALL.len() + 1) * 2 - 3);
}

#[test]
fn a_stroke_kept_to_its_group_refines_only_inside_it() {
    let mut mesh = two_halves(6_000);
    let (before, ids) = (mesh.triangle_count(), mesh.group_ids().to_vec());
    let old: Vec<[u32; 3]> = mesh.indices.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
    mesh.stay_in_groups(&[1]);
    mesh.begin_stroke();
    mesh.refine(&[Vec3::new(0.0, 0.0, 0.5)], 0.2, 0.008, usize::MAX, 10_000_000);
    mesh.end_stroke(false);
    assert!(mesh.triangle_count() > before + 200);
    // Every triangle of group 2 is as it was, corner for corner; everything new is group 1.
    for t in 0..before {
        if ids[t] == 2 {
            assert_eq!(&mesh.indices[t * 3..t * 3 + 3], &old[t][..], "triangle {t} of the other group was rewritten");
        }
    }
    assert!(mesh.group_ids()[before..].iter().all(|&id| id == 1));
    // The fence is built again for the grown mesh, not read from the smaller one.
    mesh.stay_in_groups(&[1]);
    assert_eq!(mesh.hidden_vertices().unwrap().len(), mesh.vertex_count());
    assert_eq!(mesh.hidden_vertices().unwrap(), &outside_one(&mesh)[..]);
    mesh.leave_groups();
}
