//! The Paint brush under symmetry round an axis: every sector takes the same
//! paint, next to the axis where all the copies overlap and clear of it, and
//! nothing moves. A turn by 360/N degrees is not exact in floats, so a
//! vertex and its partner may be weighed a rounding apart, and their bytes
//! may differ by one; what this catches is a sector painted differently.
//! (Across a mirror plane alone it is exact: `sculpt/paint/tests.rs`.)

use super::*;
use crate::sculpt::paint::PaintMode;

/// The largest difference between a vertex's colour coverage on the layer
/// and its partner's.
fn paint_pair_error(mesh: &Mesh, layer: u32, partners: &[(SymmetryOp, Vec<u32>)]) -> u8 {
    let stack = mesh.paint_stack().expect("paint");
    let data = &stack.layer(layer).expect("the layer").data;
    let mut worst = 0u8;
    for (_, partner) in partners {
        for (v, &p) in partner.iter().enumerate() {
            let (a, b) = (data.texel(v).colour.0, data.texel(p as usize).colour.0);
            worst = worst.max(a.iter().zip(b).map(|(x, y)| x.abs_diff(y)).max().unwrap_or(0));
        }
    }
    worst
}

#[test]
fn a_radial_paint_stroke_paints_every_sector_alike_and_moves_nothing() {
    let mut ball = Ball::new();
    let (layer, _) = ball.mesh.add_paint_layer(None).expect("a layer");
    let view = view_from_above(&ball.mesh);
    for symmetry in [RADIAL_6, RADIAL_4_MIRROR_X] {
        let partners = ball.partners_under(&symmetry.group());
        for from_axis in [0.6, 6.0] {
            for mode in [PaintMode::Paint, PaintMode::Blur] {
                let mut sculptor = sculptor_with(Brush::Paint);
                symmetry.set(&mut sculptor.settings);
                sculptor.settings.paint.mode = mode;
                sculptor.settings.paint.opacity = 0.7;
                let points = stroke_points(from_axis * BRUSH_RADIUS, STROKE_FRAMES);
                let delta = stroke_through(&mut ball.mesh, &mut sculptor, &view, &points);
                let label = format!("{mode:?} {symmetry:?} at {from_axis}r");
                let error = paint_pair_error(&ball.mesh, layer, &partners);
                let painted = (0..ball.mesh.vertex_count())
                    .filter(|&v| ball.mesh.paint_stack().unwrap().layer(layer).unwrap().data.texel(v).colour.0[3] > 0)
                    .count();
                println!("{label}: {painted} painted, pair {error}");
                assert!(painted > 100, "{label}: only {painted} vertices took paint");
                assert!(error <= 1, "{label}: a vertex and its partner are {error} apart");
                assert!(ball.mesh.positions == ball.start, "{label}: painting moved the surface");
                if mode == PaintMode::Blur {
                    // Undo the blur and the paint under it.
                    ball.undo(delta);
                }
            }
            let rows = ball.mesh.paint_rows();
            ball.mesh.clear_paint(rows[0].id, crate::mesh::paint::ChannelSet::ALL).ok();
        }
    }
}
