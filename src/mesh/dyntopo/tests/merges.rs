//! Merges: the short edges under a coarser brush folded away. Everything the
//! splits are held to (a closed surface, adjacency that matches the
//! triangles, a sound spatial index, exact undo) is held for merges too, and
//! so are the things merges alone could break: a frozen mask, a face-group
//! border, hidden faces, a triangle turned over.

use super::*;
use crate::sculpt::dyntopo::Method;

/// A pass with merges, as the brush side asks for one: split above
/// `longest`, merge below the share of it Blender uses.
fn both_at(mesh: &mut Mesh, centre: Vec3, longest: f32) -> RefineStats {
    let sizes = EdgeSizes { longest, split: true, shortest: longest * dyntopo::MERGE_SHARE };
    mesh.retopologize(&[centre], REGION_RADIUS, sizes, 1_000_000, 10_000_000)
}

/// The triangles that are not points.
fn live(mesh: &Mesh) -> usize {
    mesh.indices.chunks_exact(3).filter(|c| !is_point(c)).count()
}

/// The vertices that are in a triangle.
fn used(mesh: &Mesh) -> usize {
    (0..mesh.vertex_count()).filter(|&v| !mesh.vt.row(v).is_empty()).count()
}

/// Every live triangle of the fixture (a lumpy ball round the origin) faces
/// away from its middle: none has been turned over.
fn assert_none_turned(mesh: &Mesh) {
    for (t, c) in mesh.indices.chunks_exact(3).enumerate().filter(|(_, c)| !is_point(c)) {
        let [a, b, c] = corners_of(mesh, c);
        assert!((b - a).cross(c - a).dot(a + b + c) > 0.0, "triangle {t} faces inward");
    }
}

/// The fixture refined finely round the region, as a small brush leaves it.
fn finely_refined() -> Mesh {
    let mut mesh = fixture();
    assert!(refine_at(&mut mesh, region_centre()).splits > 1000);
    mesh
}

#[test]
fn a_coarse_pass_over_fine_detail_merges_it_back_and_stays_closed() {
    let mut mesh = finely_refined();
    let centre = region_centre();
    let fine = mesh.live_triangle_count();
    // Four times the edge the refine left: what it made is under 0.4 of that.
    let longest = MAX_EDGE * 4.0;
    let stats = both_at(&mut mesh, centre, longest);
    assert!(stats.merges > 500, "{stats:?}");
    assert!(mesh.has_tombstones());
    assert_eq!(mesh.live_triangle_count(), live(&mesh));
    assert_eq!(mesh.live_triangle_count(), fine + 2 * stats.splits - 2 * stats.merges);
    assert!(mesh.live_triangle_count() < fine * 3 / 4, "{} of {fine}", mesh.live_triangle_count());
    // Still one closed surface with no handles, on the vertices still in use.
    assert_eq!(mesh.live_triangle_count(), 2 * used(&mesh) - 4);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    assert_none_turned(&mesh);

    // No merge made an edge the next split would undo, and few short edges
    // are left under the brush (a merge that would harm the surface is not made).
    let (mut short, mut under) = (0, 0);
    for c in mesh.indices.chunks_exact(3).filter(|c| !is_point(c)) {
        let p = corners_of(&mesh, c);
        for k in 0..3 {
            let (a, b) = (p[k], p[(k + 1) % 3]);
            if segment_distance(centre, a, b) <= REGION_RADIUS {
                assert!(a.distance(b) <= longest * 1.0001, "an edge longer than the detail under the brush");
                under += 1;
                short += usize::from(a.distance(b) < longest * dyntopo::MERGE_SHARE);
            }
        }
    }
    assert!(short * 20 < under, "{short} of {under} edges under the brush are still short");
    // And asking again finds little or nothing to do.
    let again = both_at(&mut mesh, centre, longest);
    assert!(again.merges * 20 < stats.merges && again.splits == 0, "{again:?}");
}

#[test]
fn merges_undo_and_redo_exactly() {
    let mut mesh = fixture();
    let mut history = History::default();
    let centre = region_centre();
    let original = kept(&mesh);

    mesh.begin_stroke();
    assert!(refine_at(&mut mesh, centre).splits > 1000);
    history.push_stroke(mesh.end_stroke(false).expect("the refine changed the mesh"));
    let (fine, fine_count) = (kept(&mesh), mesh.live_triangle_count());

    // Merges and a move in one stroke, as a big brush makes them.
    mesh.begin_stroke();
    assert!(both_at(&mut mesh, centre, MAX_EDGE * 4.0).merges > 500);
    push(&mut mesh, centre, 0.01);
    history.push_stroke(mesh.end_stroke(false).expect("the merges changed the mesh"));
    let (coarse, coarse_count) = (kept(&mesh), mesh.live_triangle_count());
    assert_sound(&mesh);

    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(kept(&mesh) == fine, "undo did not give back the finely refined mesh");
    assert_eq!(mesh.live_triangle_count(), fine_count);
    assert!(!mesh.has_tombstones());
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    assert_eq!(history.redo(&mut mesh), Some(false));
    assert!(kept(&mesh) == coarse, "redo did not give back the merged mesh");
    assert_eq!(mesh.live_triangle_count(), coarse_count);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    for _ in 0..2 {
        assert_eq!(history.undo(&mut mesh), Some(false));
    }
    assert!(kept(&mesh) == original, "two undos did not give back the mesh as it began");
    assert_sound(&mesh);
}

#[test]
fn remove_only_merges_and_never_splits() {
    let mut mesh = finely_refined();
    let centre = region_centre();
    let longest = MAX_EDGE * 4.0;
    let sizes = EdgeSizes { longest, split: false, shortest: longest * dyntopo::MERGE_SHARE };
    let stats = mesh.retopologize(&[centre], REGION_RADIUS, sizes, 1_000_000, 10_000_000);
    assert!(stats.merges > 500 && stats.splits == 0, "{stats:?}");
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    // On the fixture as it came, whose edges are all far over the merge
    // size, there is nothing to take away and nothing is added.
    let mut plain = fixture();
    let before = kept(&plain);
    let sizes = EdgeSizes { longest: 0.02, split: false, shortest: 0.02 * dyntopo::MERGE_SHARE };
    let stats = plain.retopologize(&[centre], REGION_RADIUS, sizes, 1_000_000, 10_000_000);
    assert_eq!((stats.merges, stats.splits), (0, 0));
    assert!(kept(&plain) == before);
}

#[test]
fn a_frozen_mask_keeps_its_vertices() {
    let mut mesh = finely_refined();
    let centre = region_centre();
    // Everything frozen: nothing is merged at all.
    let mut frozen = finely_refined();
    frozen.mask.iter_mut().for_each(|m| *m = 1.0);
    assert_eq!(both_at(&mut frozen, centre, MAX_EDGE * 4.0).merges, 0);

    // Half frozen: the other half is merged, and every frozen vertex stays
    // in the surface, where it was.
    let held: Vec<usize> = (0..mesh.vertex_count()).filter(|&v| mesh.positions[v].x < centre.x).collect();
    for &v in &held {
        mesh.mask[v] = 1.0;
    }
    let before = mesh.positions.clone();
    assert!(both_at(&mut mesh, centre, MAX_EDGE * 4.0).merges > 100);
    for &v in &held {
        assert!(!mesh.vt.row(v).is_empty(), "frozen vertex {v} was merged away");
        assert_eq!(mesh.positions[v], before[v]);
    }
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
}

#[test]
fn a_face_group_border_keeps_its_vertices_and_hidden_faces_are_left_alone() {
    let mut mesh = finely_refined();
    let centre = region_centre();
    // Two groups, split down the middle of the fine region.
    let ids: Vec<u16> = mesh
        .indices
        .chunks_exact(3)
        .map(|c| u16::from(corners_of(&mesh, c).iter().map(|p| p.x).sum::<f32>() / 3.0 < centre.x))
        .collect();
    mesh.set_group_ids(ids);
    let on_border: Vec<usize> = (0..mesh.vertex_count())
        .filter(|&v| {
            let fan = mesh.vt.row(v);
            fan.iter().any(|&t| mesh.group_of(t) != mesh.group_of(fan[0]))
        })
        .collect();
    assert!(on_border.len() > 20);
    // And the faces above the region's middle hidden.
    let hidden: Vec<u32> = (0..mesh.triangle_count() as u32)
        .filter(|&t| corners_of(&mesh, &mesh.indices[t as usize * 3..t as usize * 3 + 3]).iter().all(|p| p.y > centre.y))
        .collect();
    assert!(!hidden.is_empty());
    mesh.hide(&hidden).expect("faces were hidden");
    let corners_before: Vec<[u32; 3]> = hidden.iter().map(|&t| mesh.corners(t)).collect();

    assert!(both_at(&mut mesh, centre, MAX_EDGE * 4.0).merges > 100);
    for &v in &on_border {
        assert!(!mesh.vt.row(v).is_empty(), "border vertex {v} was merged away");
    }
    for (&t, before) in hidden.iter().zip(&corners_before) {
        assert_eq!(mesh.corners(t), *before, "hidden triangle {t} was changed");
    }
    assert_sound(&mesh);
}

#[test]
fn merges_keep_to_the_budget() {
    let mut mesh = finely_refined();
    let sizes = EdgeSizes { longest: MAX_EDGE * 4.0, split: true, shortest: MAX_EDGE * 4.0 * dyntopo::MERGE_SHARE };
    let stats = mesh.retopologize(&[region_centre()], REGION_RADIUS, sizes, 40, 10_000_000);
    assert!(stats.hit_budget && stats.merges + stats.splits <= 40, "{stats:?}");
    assert!(stats.merges > 0);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
}

#[test]
fn building_the_mesh_again_drops_what_merges_left() {
    let mut mesh = finely_refined();
    assert!(both_at(&mut mesh, region_centre(), MAX_EDGE * 4.0).merges > 500);
    let rebuilt = Mesh::new(mesh.positions.clone(), mesh.indices.clone(), None);
    assert_eq!(rebuilt.triangle_count(), mesh.live_triangle_count());
    assert_eq!(rebuilt.vertex_count(), used(&mesh));
    assert!(!rebuilt.has_tombstones());
    assert_closed_manifold(&rebuilt);
}

/// What the brush side does with the Method: a small brush adds detail, a
/// big one over the same place takes it away with Both, and leaves it with
/// Add. (Clay, at strength 0.5.)
#[test]
fn a_big_brush_takes_back_what_a_small_one_added() {
    for method in Method::ALL {
        let mut mesh = fixture();
        let view = view_of(&mesh);
        let path = across(-0.2, 0.2, 0.1, 30);
        let mut sculptor = refining_sculptor(Brush::Clay, 0.04);
        sculptor.dyntopo.method = method;
        run_stroke(&mut mesh, &mut sculptor, &view, &path);
        let fine = mesh.live_triangle_count();
        sculptor.settings.radius_world = 0.25;
        run_stroke(&mut mesh, &mut sculptor, &view, &path);
        let after = mesh.live_triangle_count();
        match method {
            // A Remove-only small brush adds nothing in the first place.
            Method::Remove => assert_eq!(fine, fixture().triangle_count()),
            Method::Add => assert!(after >= fine, "{method:?}: {fine} -> {after}"),
            Method::Both => assert!(after * 3 < fine * 2, "{method:?}: {fine} -> {after}"),
        }
        if method != Method::Remove {
            assert!(fine > fixture().triangle_count() * 11 / 10, "{method:?}: the small brush added little");
        }
        assert!(mesh.positions.iter().all(|p| p.is_finite()));
        assert_closed_manifold(&mesh);
        assert_sound(&mesh);
    }
}

/// Undo and redo of a stroke that merged give back every vertex's normal to
/// the bit, as for any other stroke (`--brush-hash` holds them to it), not
/// only the shape.
#[test]
fn undo_and_redo_of_merges_give_back_the_normals_to_the_bit() {
    let bits = |mesh: &Mesh| -> (Vec<[u32; 3]>, Vec<[u32; 3]>, Vec<u32>, Vec<u32>) {
        let v = |p: &Vec3| p.to_array().map(f32::to_bits);
        (
            mesh.positions.iter().map(v).collect(),
            mesh.normals.iter().map(v).collect(),
            mesh.mask.iter().map(|m| m.to_bits()).collect(),
            mesh.indices.clone(),
        )
    };
    let mut mesh = fixture();
    let view = view_of(&mesh);
    let path = across(-0.2, 0.2, 0.1, 30);
    let mut sculptor = refining_sculptor(Brush::Clay, 0.04);
    let fine = run_stroke(&mut mesh, &mut sculptor, &view, &path).expect("the stroke did something");
    let mut history = History::default();
    history.push_stroke(fine);
    mesh.flush_normals();
    let before = bits(&mesh);
    sculptor.settings.radius_world = 0.25;
    let coarse = run_stroke(&mut mesh, &mut sculptor, &view, &path).expect("the stroke did something");
    assert!(mesh.has_tombstones(), "the big brush merged nothing");
    history.push_stroke(coarse);
    mesh.flush_normals();
    let after = bits(&mesh);

    assert_eq!(history.undo(&mut mesh), Some(false));
    mesh.flush_normals();
    let undone = bits(&mesh);
    let parts = ["positions", "normals", "mask", "triangles"];
    let differ = |x: &(Vec<[u32; 3]>, Vec<[u32; 3]>, Vec<u32>, Vec<u32>), y: &(Vec<[u32; 3]>, Vec<[u32; 3]>, Vec<u32>, Vec<u32>)| {
        [x.0 != y.0, x.1 != y.1, x.2 != y.2, x.3 != y.3].iter().zip(parts).filter(|(d, _)| **d).map(|(_, p)| p).collect::<Vec<_>>()
    };
    assert!(differ(&undone, &before).is_empty(), "undo: {:?} differ", differ(&undone, &before));
    assert_eq!(history.redo(&mut mesh), Some(false));
    mesh.flush_normals();
    let redone = bits(&mesh);
    assert!(differ(&redone, &after).is_empty(), "redo: {:?} differ", differ(&redone, &after));
}

// ---------------------------------------------------------------- taking places back

/// Everything about a mesh undo answers for, to the bit, its face groups
/// included.
fn every_bit(mesh: &Mesh) -> (Vec<[u32; 3]>, Vec<[u32; 3]>, Vec<u32>, Vec<u32>, Vec<u16>) {
    let v = |p: &Vec3| p.to_array().map(f32::to_bits);
    (
        mesh.positions.iter().map(v).collect(),
        mesh.normals.iter().map(v).collect(),
        mesh.mask.iter().map(|m| m.to_bits()).collect(),
        mesh.indices.clone(),
        mesh.group_ids().to_vec(),
    )
}

/// Fine, then coarse over the same place, then fine again, each a stroke:
/// the third takes back the places the second left.
fn fine_coarse_fine(mesh: &mut Mesh, history: &mut History) -> [(usize, usize); 3] {
    let centre = region_centre();
    let mut counts = [(0, 0); 3];
    for (k, coarse) in [false, true, false].into_iter().enumerate() {
        mesh.begin_stroke();
        let stats = if coarse { both_at(mesh, centre, MAX_EDGE * 4.0) } else { both_at(mesh, centre, MAX_EDGE) };
        assert!(stats.merges + stats.splits > 100, "stroke {k} did little: {stats:?}");
        push(mesh, centre, 0.002);
        history.push_stroke(mesh.end_stroke(false).expect("the stroke changed the mesh"));
        mesh.flush_normals();
        counts[k] = (mesh.triangle_count(), mesh.vertex_count());
    }
    counts
}

#[test]
fn a_fine_pass_after_a_coarse_one_takes_back_the_places_merges_left() {
    let mut mesh = fixture();
    let mut history = History::default();
    let [fine, coarse, again] = fine_coarse_fine(&mut mesh, &mut history);
    // The coarse pass left places behind; the fine one after it took them
    // back instead of growing the lists by as much again.
    assert_eq!(coarse, fine, "merges change no list's length");
    let grown = (again.0 - coarse.0, again.1 - coarse.1);
    let first = (fine.0 - fixture().triangle_count(), fine.1 - fixture().vertex_count());
    assert!(grown.0 * 4 < first.0 && grown.1 * 4 < first.1, "the lists grew by {grown:?} against {first:?}");
    assert!(!mesh.has_tombstones() || mesh.live_triangle_count() * 10 > mesh.triangle_count() * 9);
    assert_eq!(mesh.live_triangle_count(), live(&mesh));
    assert_eq!(mesh.live_triangle_count(), 2 * used(&mesh) - 4);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    assert_none_turned(&mesh);
}

#[test]
fn undo_and_redo_through_places_taken_back_are_exact() {
    let mut mesh = fixture();
    // Two face groups across the region, so the places taken back change group.
    let centre = region_centre();
    let ids: Vec<u16> = mesh
        .indices
        .chunks_exact(3)
        .map(|c| 1 + u16::from(corners_of(&mesh, c).iter().map(|p| p.y).sum::<f32>() / 3.0 < centre.y))
        .collect();
    mesh.set_group_ids(ids);
    let rest = mesh.positions.clone();
    let mut history = History::default();
    let mut states = vec![every_bit(&mesh)];
    let centre_stroke = |mesh: &mut Mesh, history: &mut History, coarse: bool| {
        mesh.begin_stroke();
        if coarse { both_at(mesh, centre, MAX_EDGE * 4.0) } else { both_at(mesh, centre, MAX_EDGE) };
        push(mesh, centre, 0.002);
        history.push_stroke(mesh.end_stroke(false).expect("the stroke changed the mesh"));
        mesh.flush_normals();
    };
    for coarse in [false, true, false, true, false] {
        centre_stroke(&mut mesh, &mut history, coarse);
        states.push(every_bit(&mesh));
        assert_sound(&mesh);
    }
    // Reset Shape's resting places still come out, every one a number.
    let resting = mesh.rest_shape(&rest).expect("the resting shape is this mesh's");
    assert!(resting.iter().all(|p| p.is_finite()));
    for back in (0..5).rev() {
        assert_eq!(history.undo(&mut mesh), Some(false));
        mesh.flush_normals();
        assert!(every_bit(&mesh) == states[back], "undo to step {back} did not give it back to the bit");
        assert_sound(&mesh);
    }
    for forward in 1..=5 {
        assert_eq!(history.redo(&mut mesh), Some(false));
        mesh.flush_normals();
        assert!(every_bit(&mesh) == states[forward], "redo to step {forward} did not give it back to the bit");
    }
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
}

/// With sculpt layers on the mesh, its vertices keep offsets that only a
/// record of the layers' own carries, so no place is taken back: the lists
/// grow as they did before, and the layers stay one entry per vertex.
#[test]
fn with_sculpt_layers_no_place_is_taken_back() {
    let mut mesh = fixture();
    let (layer, _) = mesh.add_layer(Some("Detail".into()));
    let mut history = History::default();
    let [fine, coarse, again] = fine_coarse_fine(&mut mesh, &mut history);
    let first = (fine.0 - fixture().triangle_count(), fine.1 - fixture().vertex_count());
    let grown = (again.0 - coarse.0, again.1 - coarse.1);
    assert!(grown.0 * 2 > first.0 && grown.1 * 2 > first.1, "places were taken back: {grown:?} against {first:?}");
    assert_eq!(mesh.layers.as_ref().and_then(|layers| layers.offsets(layer)).map(<[Vec3]>::len), Some(mesh.vertex_count()));
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
}

/// How the lists grow over a session of fine and coarse passes over one
/// place, taking places back and not (a sculpt layer on the mesh stops it,
/// and changes nothing else about the strokes):
/// `cargo test --bin sculpt -- growth_over_a_session --ignored --nocapture`.
#[test]
#[ignore]
fn growth_over_a_session() {
    for taking_back in [false, true] {
        let mut mesh = fixture();
        if !taking_back {
            let _ = mesh.add_layer(Some("stops taking back".into()));
        }
        let mut history = History::default();
        let mut line = format!("taking back {taking_back:>5}:");
        for round in 0..6 {
            let [_, _, again] = fine_coarse_fine(&mut mesh, &mut history);
            line += &format!("  {round}: {} live of {} ({} vertices)", mesh.live_triangle_count(), again.0, again.1);
        }
        println!("{line}");
    }
}
