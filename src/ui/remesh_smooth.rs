//! The Remesh card's Smooth: how much a remesh irons out, from Off (the
//! remesh as sharp as it can make it) to 10, with a line under it that says
//! what the number does. What it does to the mesh is `remesh::smoothed`.

use super::theme;
use super::tip;
use crate::doc::Options;
use crate::remesh::MAX_SMOOTH;

pub fn slider(ui: &mut egui::Ui, opts: &mut Options, tips: bool) {
    let readout = |v: f32| if v.round() == 0.0 { "Off".to_owned() } else { format!("{:.0}", v.round()) };
    let slider = theme::slider(ui, "Smooth", &mut opts.remesh_smooth, 0.0..=MAX_SMOOTH, readout);
    if tip(
        slider,
        tips,
        "How much Remesh irons out. Off copies the old surface exactly, sharpest and quickest. A little takes out the \
         jagged edges along grooves, and the old mesh's facets that come back when the resolution goes up. More \
         smooths further, and softens the finest lines. Remesh takes a little longer above Off.",
    )
    .changed()
    {
        opts.remesh_smooth = opts.remesh_smooth.round().clamp(0.0, MAX_SMOOTH);
    }
    theme::hint(ui, words(opts.remesh_smooth));
}

/// The line under the slider.
fn words(smooth: f32) -> &'static str {
    match smooth.round() as u32 {
        0 => "Off: as crisp as Remesh can make it.",
        1..=3 => "Light: irons out jagged groove edges and old facets.",
        _ => "Strong: smoother still; the finest lines soften.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_says_what_the_number_does() {
        assert!(words(0.0).starts_with("Off"));
        assert!(words(crate::remesh::DEFAULT_SMOOTH).starts_with("Light"));
        assert!(words(MAX_SMOOTH).starts_with("Strong"));
    }
}
