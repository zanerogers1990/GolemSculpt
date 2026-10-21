//! Esc in the middle of a stroke (app/escape.rs) ends the stroke the
//! ordinary way and undoes its record at once. These check that this puts
//! the surface back exactly, for a plain stroke, a mask stroke and a stroke
//! under mirror symmetry, and leaves the sculptor ready for the next.

use super::stroke_tests::{across, sculptor_with, sphere, view_of};
use super::*;

/// Draws part of a stroke with `sculptor` and cancels it as Esc does.
/// Returns how far any vertex and any mask value ended up from where they
/// were, and whether the stroke had changed anything before it was cancelled.
fn cancelled(sculptor: &mut Sculptor) -> (f32, f32, bool) {
    let mut mesh = sphere(20_000);
    let view = view_of(&mesh);
    let (positions, mask) = (mesh.positions.clone(), mesh.mask.clone());
    let path = across(&view, -0.3, 0.3, 40);
    let sample = |i: usize| PointerSample { pos: path[i], pressure: Some(0.8), time: i as f64 / 240.0 };
    assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
    for i in 1..30 {
        sculptor.sample(&mut mesh, &view, sample(i));
        sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
    }
    let changed = mesh.positions != positions || mesh.mask != mask;
    // As `Running::cancel_stroke`: samples still queued are dropped.
    if let Some(delta) = sculptor.end(&mut mesh, &view) {
        mesh.apply_delta(&delta, true);
    }
    let moved = mesh.positions.iter().zip(&positions).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
    let masked = mesh.mask.iter().zip(&mask).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
    assert!(!sculptor.is_active(), "the sculptor is ready for the next stroke");
    (moved, masked, changed)
}

#[test]
fn a_cancelled_stroke_puts_the_surface_back_exactly() {
    for brush in [Brush::Clay, Brush::Standard, Brush::Inflate, Brush::Move, Brush::SnakeHook] {
        let (moved, masked, changed) = cancelled(&mut sculptor_with(brush, 0.15, 0.8));
        assert!(changed, "{}: the stroke did something before it was cancelled", brush.name());
        assert_eq!((moved, masked), (0.0, 0.0), "{}: the surface is back exactly", brush.name());
    }
}

#[test]
fn a_cancelled_mask_stroke_takes_the_mask_back() {
    let mut sculptor = sculptor_with(Brush::Standard, 0.15, 0.8);
    sculptor.settings.mask_mode = true;
    let (moved, masked, changed) = cancelled(&mut sculptor);
    assert!(changed, "the mask stroke painted something");
    assert_eq!((moved, masked), (0.0, 0.0));
}

#[test]
fn a_cancelled_mirrored_stroke_takes_both_sides_back() {
    let mut sculptor = sculptor_with(Brush::Clay, 0.15, 0.8);
    sculptor.settings.symmetry = [true, false, false];
    let (moved, _, changed) = cancelled(&mut sculptor);
    assert!(changed);
    assert_eq!(moved, 0.0);
}
