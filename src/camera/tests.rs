use glam::{Vec2, Vec3};

use super::*;

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

/// How far from the middle of the picture, in pixels, a point one unit above
/// the pivot lands.
fn height_on_screen(camera: &Camera) -> f32 {
    let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 2.0);
    let middle = view.project(camera.pivot).unwrap();
    let above = view.project(camera.pivot + camera.rotation() * Vec3::Y).unwrap();
    (above - middle).length()
}

#[test]
fn lenses_and_angles_of_view_turn_into_each_other() {
    assert!((lens_mm(DEFAULT_FOV) - 20.8).abs() < 0.1, "the starting view is about a 21 mm lens");
    assert!((fov_of_lens(50.0) - 27.0).abs() < 0.1, "a 50 mm lens sees 27 degrees from top to bottom");
    for mm in LENSES {
        assert!((lens_mm(fov_of_lens(mm)) - mm).abs() < 1e-3);
    }
}

#[test]
fn a_new_lens_keeps_what_is_at_the_pivot_the_same_size_on_screen() {
    let mut camera = Camera { pivot: Vec3::new(0.3, 1.0, -0.2), yaw: 30.0, pitch: 10.0, ..Camera::default() };
    let before = height_on_screen(&camera);
    let distance = camera.distance;
    camera.set_lens(fov_of_lens(85.0));
    assert!((height_on_screen(&camera) - before).abs() < before * 1e-3);
    assert!(camera.distance > distance * 3.0, "the camera stepped back to make up for the long lens");
    camera.set_lens(fov_of_lens(18.0));
    assert!((height_on_screen(&camera) - before).abs() < before * 1e-3);
}

#[test]
fn an_orthographic_view_does_not_change_with_the_lens() {
    let mut camera = Camera { ortho: true, ..Camera::default() };
    let before = height_on_screen(&camera);
    camera.set_lens(fov_of_lens(135.0));
    assert!((height_on_screen(&camera) - before).abs() < before * 1e-3);
}

#[test]
fn a_lens_past_the_range_or_not_a_number_is_brought_back() {
    let mut camera = Camera::default();
    camera.set_lens(1.0);
    assert_eq!(camera.fov_y, *FOV_RANGE.start());
    camera.set_lens(f32::NAN);
    assert_eq!(camera.fov_y, DEFAULT_FOV);
    assert!(camera.distance.is_finite());
}
