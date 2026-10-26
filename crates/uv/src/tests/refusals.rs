//! What is refused, with a message, and the broken meshes that are not
//! refused but cut into charts where they are broken.

use glam::Vec3;

use super::checks::measure;
use super::meshes::{Model, cube, holed_grid, loose_squares};
use crate::{MAX_TRIANGLES, Options, unwrap};

fn refusal(positions: &[Vec3], triangles: &[u32], polys: Option<(&[u32], &[u32])>, groups: Option<&[u16]>) -> String {
    match unwrap(positions, triangles, polys, groups, &Options::default()) {
        Ok(_) => panic!("unwrapped when it should have been refused"),
        Err(e) => {
            println!("refused: {e}");
            e
        }
    }
}

#[test]
fn an_empty_model_is_refused() {
    assert!(refusal(&[], &[], None, None).contains("nothing to unwrap"));
    assert!(refusal(&[Vec3::ZERO; 3], &[], None, None).contains("nothing to unwrap"));
}

#[test]
fn a_model_over_the_limit_is_refused_before_any_work() {
    let triangles = [0, 1, 2].repeat(MAX_TRIANGLES + 1);
    let points = [Vec3::ZERO, Vec3::X, Vec3::Y];
    assert!(refusal(&points, &triangles, None, None).contains("at most"));
}

#[test]
fn broken_triangle_lists_and_points_are_refused() {
    let points = [Vec3::ZERO, Vec3::X, Vec3::Y];
    assert!(refusal(&points, &[0, 1], None, None).contains("not a whole number"));
    assert!(refusal(&points, &[0, 1, 3], None, None).contains("only 3 points"));
    let bad = [Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 0.0), Vec3::Y];
    assert!(refusal(&bad, &[0, 1, 2], None, None).contains("not a number"));
}

#[test]
fn a_model_with_no_surface_is_refused() {
    // Every triangle squashed onto a line.
    let points = [Vec3::ZERO, Vec3::X, Vec3::X * 2.0, Vec3::X * 3.0];
    assert!(refusal(&points, &[0, 1, 2, 1, 2, 3], None, None).contains("no surface"));
}

#[test]
fn more_charts_than_fit_the_square_at_this_padding_are_refused() {
    // A tenth of the square between charts leaves room for about a hundred.
    let model = loose_squares(300);
    let options = Options { padding: 0.1, ..Options::default() };
    let e = unwrap(&model.positions, &model.triangles, None, None, &options).unwrap_err();
    println!("refused: {e}");
    assert!(e.contains("too many pieces"));
}

#[test]
fn groups_and_polygons_that_do_not_fit_the_triangles_are_refused() {
    let model = cube(2, true);
    let (start, corners) = model.polys().unwrap();
    let groups = vec![0u16; model.triangle_count() - 1];
    assert!(refusal(&model.positions, &model.triangles, None, Some(&groups)).contains("face groups"));
    // One face too few.
    let short = &start[..start.len() - 1];
    let short_corners = &corners[..*short.last().unwrap() as usize];
    assert!(refusal(&model.positions, &model.triangles, Some((short, short_corners)), None).contains("polygon"));
    // Triangles that are not pieces of their faces.
    let mut shuffled = model.triangles.clone();
    shuffled.rotate_left(6);
    assert!(refusal(&model.positions, &shuffled, Some((start, corners)), None).contains("not a piece"));
}

#[test]
fn polygon_starts_are_taken_with_or_without_the_closing_count() {
    let model = cube(3, true);
    let (start, corners) = model.polys().unwrap();
    let options = Options::default();
    let with = unwrap(&model.positions, &model.triangles, Some((start, corners)), None, &options).unwrap();
    let without =
        unwrap(&model.positions, &model.triangles, Some((&start[..start.len() - 1], corners)), None, &options).unwrap();
    assert_eq!(with, without);
}

#[test]
fn odd_options_are_held_to_sensible_values() {
    let model = cube(2, false);
    for options in [
        Options { padding: -1.0, max_angle_degrees: 500.0 },
        Options { padding: f32::NAN, max_angle_degrees: f32::NAN },
        Options { padding: 0.0, max_angle_degrees: 0.0 },
        Options { padding: 3.0, max_angle_degrees: 89.0 },
    ] {
        let out = unwrap(&model.positions, &model.triangles, None, None, &options)
            .unwrap_or_else(|e| panic!("{options:?}: {e}"));
        measure(&model, &out).assert_valid(&format!("{options:?}"));
    }
}

#[test]
fn broken_meshes_are_cut_where_they_are_broken_not_refused() {
    let grid = holed_grid(12);
    let mut positions = grid.positions.clone();
    let mut triangles = grid.triangles.clone();
    // A fin: a third triangle on an edge two triangles already share.
    let fin_tip = positions.len() as u32;
    positions.push(Vec3::new(0.5, 0.05, 0.3));
    triangles.extend([triangles[0], triangles[1], fin_tip]);
    // A triangle wound the wrong way.
    triangles.swap(10 * 3 + 1, 10 * 3 + 2);
    // A triangle squashed onto a line (three points along the bottom edge),
    // and one onto a point.
    triangles.extend([0, 1, 2, 5, 5, 5]);
    // The same triangle twice.
    triangles.extend_from_slice(&grid.triangles[30..33]);
    // A point no triangle uses.
    positions.push(Vec3::splat(9.0));
    let model = Model { positions, triangles, polys: None };
    let out = unwrap(&model.positions, &model.triangles, None, None, &Options::default()).unwrap();
    let q = measure(&model, &out);
    q.print("broken grid");
    assert_eq!(q.outside, 0);
    assert_eq!(q.overlaps, 0);
    assert_eq!(q.flipped, 0);
}
