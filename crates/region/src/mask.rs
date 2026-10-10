//! Masking what a shape covers: the screen-space counterpart of the mask
//! brush.
//!
//! A region covers everything under it, front and back, straight through the
//! model. Each of these is one mask step, undone on its own like a brush
//! stroke.

use rayon::prelude::*;
use sculpt_cut::select::{mark_vertices_in_region, mask_targets};
use sculpt_cut::{ScreenProjection, ScreenRegion};
use sculpt_toolkit::{Command, Ctx, ObjectId, View};

use crate::screen::{cut_views, symmetry};

/// The selected object, when it has vertices to mask.
fn target(cx: &Ctx) -> Result<ObjectId, String> {
    let scene = cx.scene();
    match scene.active_mesh() {
        Some(mesh) if mesh.vertex_count() > 0 && mesh.mask.len() == mesh.vertex_count() => Ok(scene.active()),
        Some(_) => Err("No geometry.".to_owned()),
        None => Err("No object selected.".to_owned()),
    }
}

/// Sets the mask to `value` (1 masks, 0 unmasks) on every vertex of the
/// selected object that `region` covers as seen through `view`, or with
/// `outside` on every vertex it does not. With mirror axes on, a vertex also
/// counts as covered when its mirrored position is, so a box over one arm
/// takes both; radial symmetry does the same for every turn round its axis.
/// Hidden parts are left alone. Returns what to tell the user.
///
/// `covered` is the per-vertex scratch buffer, kept by the caller across
/// gestures: at millions of vertices it is the largest thing a gesture
/// allocates.
pub fn apply(
    cx: &mut Ctx,
    view: &View,
    region: &ScreenRegion,
    value: f32,
    outside: bool,
    covered: &mut Vec<bool>,
) -> Result<String, String> {
    let object = target(cx)?;
    let scene = cx.scene();
    let mesh = scene.mesh(object).ok_or("No object selected.")?;
    let projections: Vec<ScreenProjection> = cut_views(scene, object, view).iter().map(|v| v.projection).collect();
    mark_vertices_in_region(mesh.positions, &projections, &symmetry(cx), region, outside, covered);
    // Hidden vertices are skipped: hiding is how a limb is got out of the
    // way, and a box over the torso masking the arm hidden behind it would
    // defeat that.
    let targets = mask_targets(covered, mesh.hidden_vertices)?;

    let mut mask = mesh.mask.to_vec();
    for &vertex in &targets {
        mask[vertex as usize] = value;
    }
    cx.commit("Mask Region", vec![Command::Mask { object, mask }]);
    Ok(format!("{} {} vertices.", if value > 0.0 { "Masked" } else { "Unmasked" }, targets.len()))
}

/// Unmasks the whole of the selected object: what a click without a drag
/// does.
pub fn clear(cx: &mut Ctx) -> Result<String, String> {
    let object = target(cx)?;
    let mesh = cx.scene().mesh(object).ok_or("No object selected.")?;
    if !mesh.mask.par_iter().any(|&m| m > 0.0) {
        return Ok("Nothing is masked.".to_owned());
    }
    let mask = vec![0.0; mesh.vertex_count()];
    cx.commit("Clear Mask", vec![Command::Mask { object, mask }]);
    Ok("Cleared the mask.".to_owned())
}

/// Swaps masked and free on the selected object.
pub fn invert(cx: &mut Ctx) -> Result<String, String> {
    let object = target(cx)?;
    let mesh = cx.scene().mesh(object).ok_or("No object selected.")?;
    let mask: Vec<f32> = mesh.mask.par_iter().map(|&m| 1.0 - m).collect();
    cx.commit("Invert Mask", vec![Command::Mask { object, mask }]);
    Ok("Inverted the mask.".to_owned())
}
