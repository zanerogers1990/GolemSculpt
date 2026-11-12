//! The box the mask brush borrows (Shift + drag while mask painting).

use glam::Vec2;
use sculpt_toolkit::testing::Bench;

use super::{CENTRE, mask, positions};
use crate::QuickBox;
use crate::testing::sphere;

fn boxed(bench: &mut Bench, from: Vec2, to: Vec2, unmask: bool) -> Result<String, String> {
    let quick = QuickBox::begin(from, unmask);
    bench.call(|cx| quick.finish(cx, to))
}

#[test]
fn a_box_masks_what_it_covers_and_one_over_empty_space_clears_the_mask() {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(4, 1.0));
    let said = boxed(&mut bench, Vec2::ZERO, Vec2::new(CENTRE.x + 3.0, 600.0), false).unwrap();
    assert!(said.starts_with("Masked "), "{said}");
    for (p, m) in positions(&bench, ball).iter().zip(mask(&bench, ball)) {
        assert_eq!(m, if p.x <= 1e-6 { 1.0 } else { 0.0 }, "{p}");
    }
    assert_eq!(bench.doc.undo_steps(), 1);

    // The right button: the top half comes back out.
    boxed(&mut bench, Vec2::ZERO, Vec2::new(800.0, CENTRE.y + 3.0), true).unwrap();
    for (p, m) in positions(&bench, ball).iter().zip(mask(&bench, ball)) {
        assert_eq!(m, if p.x <= 1e-6 && p.y < -1e-6 { 1.0 } else { 0.0 }, "{p}");
    }

    // A box in the corner, clear of the ball, takes the whole mask off.
    assert_eq!(boxed(&mut bench, Vec2::ZERO, Vec2::new(60.0, 60.0), false).unwrap(), "Cleared the mask.");
    assert!(mask(&bench, ball).iter().all(|m| *m == 0.0));
    assert_eq!(bench.doc.undo_steps(), 3, "each box is one step");
}

#[test]
fn a_tap_clears_the_mask_on_empty_space_and_leaves_it_on_the_model() {
    let mut bench = Bench::new();
    let ball = bench.add("Ball", sphere(4, 1.0));
    boxed(&mut bench, Vec2::ZERO, Vec2::new(CENTRE.x + 3.0, 600.0), false).unwrap();
    let masked = mask(&bench, ball);

    boxed(&mut bench, CENTRE, CENTRE + Vec2::splat(2.0), false).unwrap();
    assert_eq!(mask(&bench, ball), masked, "a slip of the pen on the model keeps the mask");

    let corner = Vec2::new(20.0, 20.0);
    assert_eq!(boxed(&mut bench, corner, corner, false).unwrap(), "Cleared the mask.");
    assert!(mask(&bench, ball).iter().all(|m| *m == 0.0));
}
