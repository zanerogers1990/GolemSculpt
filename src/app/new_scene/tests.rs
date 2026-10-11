//! What a new scene is, without a window: the document New Scene puts in
//! place, what becomes of the scene it replaces, and when it is refused.
//! What happens to the scene's file across it is in `file_undo/tests.rs`;
//! when the question is asked, in `unsaved/tests.rs`; its words, in
//! `ui/question.rs`.

use glam::{Quat, Vec3};

use super::*;
use crate::app::install;
use crate::camera::SavedView;
use crate::io::slot;
use crate::look::Look;
use crate::scene::{Scene, Symmetry, saved_mirror, saved_symmetry};

#[test]
fn the_document_of_a_new_scene_is_what_the_app_starts_with() {
    let document = fresh_document(2_000.0, 1.6);
    assert_eq!((document.meshes.len(), document.objects.len(), document.active), (1, 1, 0));
    let ball = &document.objects[0];
    assert_eq!((ball.name.as_str(), ball.visible, ball.mesh), ("Sphere", true, 0));
    assert_eq!((ball.position, ball.rotation, ball.scale), (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE));
    // The very mesh a start of the app builds, at the Starting Detail it was given.
    let wanted = starting_mesh(2_000.0);
    assert_eq!(document.meshes[0].positions, wanted.positions);
    assert_eq!(document.meshes[0].indices, wanted.indices);
    assert!(document.meshes[0].mask.iter().all(|&m| m == 0.0), "nothing is masked");
    let more = fresh_document(20_000.0, 1.6).meshes[0].triangle_count();
    assert!(more > 4 * wanted.triangle_count(), "the detail asked for is the detail made: {more}");
}

#[test]
fn it_is_seen_from_the_starting_view_and_names_no_look_of_its_own() {
    let document = fresh_document(2_000.0, 1.6);
    let view: SavedView = document.details.get(slot::CAMERA).expect("the document keeps a view");
    assert_eq!(view, starting_view(document.meshes[0].bounds(), 1.6).saved_view());
    assert_eq!((view.yaw, view.pitch, view.ortho), (25.0, 15.0, false));
    assert_eq!(view.pivot, [0.0; 3], "the ball is in the middle of the picture");
    // All of the ball is in the picture, in a wide window and in a tall one.
    for aspect in [1.6f32, 0.5] {
        let camera = starting_view(document.meshes[0].bounds(), aspect);
        let half = (camera.fov_y.to_radians() * 0.5).tan() * camera.distance;
        assert!(half * aspect.min(1.0) > 0.5, "aspect {aspect}: the ball does not fit");
    }
    // The look, the guides and the symmetry are left to what is there (see new_scene.rs).
    let details = &document.details;
    assert!(details.get_over(slot::SHADING, &io::Shading::default()).is_none(), "no material of its own");
    assert!(details.get_over(slot::LOOK, &Look::default()).is_none(), "no finish or lighting of its own");
    assert!(details.get_over(slot::MIRROR, &io::Mirror::default()).is_none());
    assert!(saved_symmetry(details, 0).is_none() && saved_mirror(details, 0).is_none());
}

/// The lowest detail there is to ask for still gives a ball (rule 9 of crates/README.md).
#[test]
fn the_lowest_detail_still_gives_a_ball() {
    for triangles in [0.0, 1.0, f32::NAN, -5.0] {
        let document = fresh_document(triangles, 1.0);
        let mesh = &document.meshes[0];
        assert!(mesh.triangle_count() >= 12, "{triangles}: {} triangles", mesh.triangle_count());
        assert!(mesh.positions.iter().all(|p| (p.length() - 0.5).abs() < 1e-4), "{triangles}: not a ball");
    }
}

/// Put in place of a scene that has been worked on, the document leaves one
/// object: new, selected, showing, with no symmetry, whatever the scene had.
#[test]
fn in_place_of_a_worked_scene_it_leaves_one_new_ball_selected_with_no_symmetry() {
    let mut mesh = starting_mesh(80.0);
    let mut scene = Scene::new("Body");
    let body = scene.active_id();
    let arm = scene.add("Arm", starting_mesh(80.0), &mut mesh);
    scene.edit_symmetry(body, |symmetry| symmetry.mirror = [true, false, true]);
    scene.edit_symmetry(arm, |symmetry| symmetry.radial.on = true);
    scene.select(body, &mut mesh);
    scene.hide_selected(&mut mesh);
    let before = scene.state();

    let details = install(&mut scene, &mut mesh, fresh_document(2_000.0, 1.6));
    scene.restore_symmetry(&details);
    scene.restore_mirrors(&details);
    let [ball] = &scene.objects[..] else { panic!("{} objects", scene.objects.len()) };
    assert!(ball.id != body && ball.id != arm, "an object of its own, not one of the old ones under a new name");
    assert_eq!((ball.name.as_str(), ball.visible, ball.mirror), ("Sphere", true, None));
    assert_eq!((scene.active_id(), scene.selected().to_vec()), (ball.id, vec![ball.id]));
    assert_eq!(scene.symmetry(ball.id), Symmetry::default());
    assert!(!scene.in_local_view());
    assert_eq!(mesh.positions, starting_mesh(2_000.0).positions, "the mesh in hand is the new ball's");

    // And the scene from before is all there for Undo, symmetry and all.
    scene.restore(before, &mut mesh);
    assert_eq!(scene.objects.iter().map(|o| o.id).collect::<Vec<_>>(), [body, arm]);
    assert_eq!(scene.symmetry(body).mirror, [true, false, true]);
    assert!(scene.symmetry(arm).radial.on);
}

#[test]
fn a_new_scene_is_refused_while_a_job_works_or_a_loop_records_and_says_which() {
    assert_eq!(refusal(None, false), None, "nothing in the way");
    let job = refusal(Some("Merge Tubes"), false).expect("a job is in the way");
    assert_eq!(job, "Merge Tubes is still working. Wait for it, or press its Cancel, then start a new scene.");
    let looping = refusal(None, true).expect("a recording is in the way");
    assert!(looping.starts_with("A 360 loop is being recorded.") && looping.contains("Esc"));
    assert_eq!(refusal(Some("Merge Tubes"), true), Some(looping), "the recording is said first: it holds the keys");
}
