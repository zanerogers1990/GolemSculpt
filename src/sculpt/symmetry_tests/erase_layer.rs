//! Erase Layer under symmetry round an axis: the copies of its dab rub the
//! layer out by the same amount in every sector, never past the model
//! underneath, and no harder at the axis, where they all land on one spot,
//! than anywhere else. (Across a mirror plane it is in `erase/tests.rs`,
//! where the mesh is a mirror image of itself to the bit.)

use super::*;

/// Ridges round the ball, the same at every angle round Y: a layer every
/// symmetry tested here maps onto itself.
fn ridges(p: Vec3) -> Vec3 {
    p + p.normalize() * (0.003 * (1.5 + (p.y * 50.0).cos()))
}

/// The ball with that layer on it, recording, and where every vertex stood
/// before the layer was sculpted. Erasing is measured from the ball as it
/// is now (`Ball::start`).
fn layered() -> (Ball, Vec<Vec3>) {
    let mut ball = Ball::new();
    let bare = ball.mesh.positions.clone();
    ball.mesh.add_layer(None);
    let leaves: Vec<u32> = (0..ball.mesh.bvh.leaves.len() as u32).collect();
    ball.mesh.begin_stroke();
    ball.mesh.move_vertices(&leaves, |_, vertex| Some(ridges(vertex.pos)));
    ball.mesh.positions_changed(&leaves);
    ball.mesh.end_stroke(false).expect("the layer took the ridges");
    ball.mesh.flush_normals();
    let _ = ball.mesh.take_dirty();
    ball.start = ball.mesh.positions.clone();
    (ball, bare)
}

fn eraser(strength: f32) -> Sculptor {
    let mut sculptor = sculptor_with(Brush::EraseLayer);
    sculptor.settings.current_mut().strength = strength;
    sculptor
}

/// The most of the way to the model underneath any vertex has gone.
fn furthest_erased(ball: &Ball, bare: &[Vec3]) -> f32 {
    let share = |v: usize| {
        let whole = bare[v] - ball.start[v];
        (ball.mesh.positions[v] - ball.start[v]).dot(whole) / whole.length_squared()
    };
    (0..bare.len()).map(share).fold(0.0, f32::max)
}

#[test]
fn a_radial_stroke_rubs_the_layer_out_the_same_in_every_sector_and_never_past_the_model() {
    let (mut ball, bare) = layered();
    let view = view_from_above(&ball.mesh);
    for symmetry in [RADIAL_6, RADIAL_12, RADIAL_4_MIRROR_X] {
        let partners = ball.partners_under(&symmetry.group());
        let floor = pair_error(&ball.mesh, &partners);
        assert!(floor < 1e-6, "{symmetry:?}: the layered ball is {floor:.2e} off symmetric before any stroke");
        for from_axis in [0.0, 0.6, 2.5, 6.0] {
            for strength in [0.3, 1.0] {
                let mut sculptor = eraser(strength);
                symmetry.set(&mut sculptor.settings);
                let points = stroke_points(from_axis * BRUSH_RADIUS, STROKE_FRAMES);
                let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
                let label = format!("Erase Layer at {strength}, {symmetry:?}, {from_axis}r from the axis");
                assert_symmetric(&label, &ball, &partners, Tolerance::OTHERS);
                let most = furthest_erased(&ball, &bare);
                assert!(most > 0.2 && most <= 1.0 + 1e-3, "{label}: a vertex went {most} of the way to the model underneath");
                ball.undo(delta);
                assert!(ball.mesh.positions == ball.start, "{label}: undo did not put every copy of the stroke back");
            }
        }
    }
}

/// A press on the axis lands every copy on one spot, and they are one dab:
/// the layer goes as far as it does with no symmetry, not twelve times as
/// fast. A stroke through the axis is twelve strokes crossing there, each
/// vertex taking, dab by dab, what the copy that asks most of it asks: more
/// than one stroke erases where they cross, as twelve strokes drawn by hand
/// would, and nowhere near twelve times it (the line the other brushes'
/// `coincident_copies_at_the_pole_act_as_one_dab` draws is one and a half).
#[test]
fn copies_of_the_eraser_on_one_spot_are_one_eraser() {
    let (mut ball, bare) = layered();
    let view = view_from_above(&ball.mesh);
    let mut erased = |symmetry: Option<Symmetry>, points: &[Vec3]| {
        // Lightly, so that the stroke is nowhere near done: twelve of it would show.
        let mut sculptor = eraser(0.05);
        if let Some(symmetry) = symmetry {
            symmetry.set(&mut sculptor.settings);
        }
        let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, points);
        let most = furthest_erased(&ball, &bare);
        ball.undo(delta);
        most
    };
    // On the ball as it stands with its layer, or the pointer would find the surface short of the axis.
    let pole = [ridges(point_on_ball(Vec3::Y))];
    let (alone, repeated) = (erased(None, &pole), erased(Some(RADIAL_12), &pole));
    println!("a press on the axis: {alone:.5} erased alone, {repeated:.5} with twelve repeats");
    assert!(alone > 0.02 && (repeated / alone - 1.0).abs() < 0.02);
    for radii_from_axis in [0.0, 0.3, 0.6, 1.0, 2.5] {
        let points = stroke_points(radii_from_axis * BRUSH_RADIUS, STROKE_FRAMES);
        let (alone, repeated) = (erased(None, &points), erased(Some(RADIAL_12), &points));
        let report = format!("{radii_from_axis}r from the axis: {alone:.4} erased alone, {repeated:.4} with twelve repeats");
        println!("{report}");
        assert!(alone > 0.05 && alone < 0.6 && (0.9..=1.5).contains(&(repeated / alone)), "{report}");
    }
}
