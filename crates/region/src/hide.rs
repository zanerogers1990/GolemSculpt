//! Hiding what a shape covers: "hide the arms so I can work on the abdomen".
//!
//! Hiding is per face, not per vertex: a triangle is hidden when all three of
//! its vertices fall under the shape, so a triangle straddling the edge stays,
//! and the border of a hidden area behaves like ordinary surface rather than
//! a frozen wall. Nothing is removed; hidden faces are only not drawn, not
//! hit by the pointer and not touched by brushes. Each hide, show, Show All
//! and Invert is an undo step of its own, like a mask.
//!
//! A hide that would leave nothing of the object on screen is refused: an
//! object that has vanished gives no sign that it is still there, and a tool
//! that can make the model disappear by overshooting a drag is not one people
//! trust.

use sculpt_cut::select::{mark_vertices_in_region, plan_hide};
use sculpt_cut::{ScreenProjection, ScreenRegion};
use sculpt_toolkit::{Command, Ctx, View};

use crate::screen::{cut_views, symmetry};

/// What is said when a drag would hide everything. `plan_hide` gives the same
/// sentence when it sees the overshoot coming; this is for the app's own
/// refusal, should a commit ever reach it.
const WHOLE_OBJECT: &str = "That would hide the whole object - skipped.";

/// The triangles of the selected object a hide gesture (or, with `hide`
/// false, a show gesture) would flip: those wholly under `region` that are
/// not already as wanted. Fails, with what to tell the user, when the drag
/// would change nothing or would leave nothing on screen.
///
/// `covered` is the per-vertex scratch buffer the caller keeps across
/// gestures.
pub fn plan(
    cx: &Ctx,
    view: &View,
    region: &ScreenRegion,
    outside: bool,
    hide: bool,
    covered: &mut Vec<bool>,
) -> Result<Vec<u32>, String> {
    let scene = cx.scene();
    let object = scene.active();
    let mesh = scene.mesh(object).ok_or("No object selected.")?;
    let projections: Vec<ScreenProjection> = cut_views(scene, object, view).iter().map(|v| v.projection).collect();
    mark_vertices_in_region(mesh.positions, &projections, &symmetry(cx.mirror), region, outside, covered);
    plan_hide(mesh.indices, covered, mesh.hidden_triangles, hide)
}

/// Hides what `region` covers on the selected object as seen through `view`,
/// or with `hide` false shows it again; with `outside`, everything the shape
/// does not cover. One undo step. Returns what to tell the user.
pub fn apply(
    cx: &mut Ctx,
    view: &View,
    region: &ScreenRegion,
    outside: bool,
    hide: bool,
    covered: &mut Vec<bool>,
) -> Result<String, String> {
    let triangles = plan(cx, view, region, outside, hide, covered)?;
    let count = triangles.len();
    let object = cx.scene().active();
    let label = if hide { "Hide Region" } else { "Show Region" };
    let receipt = cx.commit(label, vec![Command::HideFaces { object, triangles, hide }]);
    if receipt.failed > 0 {
        return Err(if hide { WHOLE_OBJECT } else { "Nothing hidden there." }.to_owned());
    }
    Ok(if hide {
        format!("Hid {count} faces. A click shows everything again.")
    } else {
        format!("Showed {count} faces.")
    })
}

/// Shows everything on the selected object again: what a click without a
/// drag does.
pub fn show_all(cx: &mut Ctx) -> Result<String, String> {
    let scene = cx.scene();
    let object = scene.active();
    let mesh = scene.mesh(object).ok_or("No object selected.")?;
    if mesh.hidden_triangles.is_none() {
        return Ok("Nothing is hidden.".to_owned());
    }
    let receipt = cx.commit("Show All", vec![Command::ShowAllFaces { object }]);
    if receipt.failed > 0 {
        return Err("Nothing is hidden.".to_owned());
    }
    Ok("Everything is showing again.".to_owned())
}

/// Swaps what is hidden and what shows on the selected object: hide what is
/// in the way, invert, and only it is left to work on.
pub fn invert(cx: &mut Ctx) -> Result<String, String> {
    let scene = cx.scene();
    let object = scene.active();
    let mesh = scene.mesh(object).ok_or("No object selected.")?;
    if mesh.hidden_triangles.is_none() {
        return Ok("Nothing is hidden, so there is nothing to swap.".to_owned());
    }
    let receipt = cx.commit("Invert Visible", vec![Command::InvertFaces { object }]);
    if receipt.failed > 0 {
        return Err("Nothing is hidden, so there is nothing to swap.".to_owned());
    }
    Ok("Swapped the hidden and the showing parts.".to_owned())
}
