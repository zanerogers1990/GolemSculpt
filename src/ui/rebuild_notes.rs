//! A line under each button that rebuilds a model (Remesh, Quad Remesh,
//! Decimate, Join, the booleans) saying what it will throw away, when there
//! is any of it: subdivision levels, sculpt layers, the mask, hidden parts.
//! Undo brings all of it back, and the line says so; it is there so that
//! nobody finds out from the status line afterwards. And under the remesh
//! and boolean resolution sliders, about how many triangles that resolution
//! would make, by the kernel's own estimate (`budgeted_resolution`).
//!
//! What each one loses is the README's account of it (Subdivision levels,
//! Sculpt layers, Hidden parts, Decimate): every rebuild drops the levels and
//! merges the layers into the model; Remesh, Quad Remesh, the booleans and a
//! Join with remesh build a new surface, with no mask and everything showing;
//! Decimate keeps the mask (it protects) and what is hidden; a plain Join keeps
//! each mask but shows what was hidden.
//!
//! Whether a mesh has a mask and how much surface it has take a pass over it,
//! so they are worked out once per edit of the scene (`History::edits`), not
//! once a frame, and kept in egui's memory between frames.

use egui::{Id, Ui};
use glam::Vec3;
use rayon::prelude::*;
use sculpt_volume::budget::{MAX_RESOLUTION, MAX_TRIANGLES, MIN_RESOLUTION};
use sculpt_volume::remesh::{budgeted_resolution, surface_area};

use super::{fmt_count, theme};
use crate::doc::Doc;
use crate::mesh::Mesh;

/// The rebuilds, by what they keep.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Rebuild {
    /// Remesh, Quad Remesh, a boolean, a Join with remesh: a new surface.
    Surface,
    /// Decimate: fewer of the same triangles.
    Decimate,
    /// A plain Join: the shells as they are, in one mesh.
    Join,
}

/// What a mesh holds that a rebuild may throw away, and its surface area,
/// as of one edit.
#[derive(Clone, Copy, Default)]
struct Held {
    /// The edit and the mesh this was worked out for.
    stamp: (u64, usize),
    levels: bool,
    layers: bool,
    mask: bool,
    hidden: bool,
    /// Worked out only when a slider asks for it.
    area: Option<f64>,
}

/// What `mesh` holds, from egui's memory if nothing has been edited since it
/// was last worked out. `area` asks for its surface area too.
fn held(ui: &Ui, mesh: &Mesh, edits: u64, area: bool) -> Held {
    let id = Id::new(("rebuild_notes", mesh.uid));
    let stamp = (edits, mesh.triangle_count());
    let mut kept: Held = ui.ctx().data(|d| d.get_temp(id)).filter(|h: &Held| h.stamp == stamp).unwrap_or_else(|| Held {
        stamp,
        levels: mesh.level_count() > 1,
        layers: mesh.layer_count() > 0,
        mask: mesh.mask.par_iter().any(|&m| m > 0.0),
        hidden: mesh.hidden_triangle_count() > 0,
        area: None,
    });
    if area && kept.area.is_none() {
        kept.area = Some(surface_area(&mesh.positions, &mesh.indices));
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, kept));
    kept
}

/// Whether `mesh` has any mask, as of edit `edits`: for the status line by
/// the bottom bar (status_row.rs), from the same memory as the hints.
pub(super) fn masked(ui: &Ui, mesh: &Mesh, edits: u64) -> bool {
    held(ui, mesh, edits, false).mask
}

/// The selected objects' meshes, each once (instances share one), with the
/// scale each is shown at. The bright one first.
fn selected_meshes(doc: &Doc) -> Vec<(&Mesh, Vec3)> {
    let scene = &doc.scene;
    let mut seen = Vec::new();
    let mut out = Vec::new();
    let active = scene.active_id();
    let ids = std::iter::once(active).chain(scene.selected().iter().copied().filter(|&id| id != active));
    for id in ids {
        let Some(object) = scene.get(id) else { continue };
        if seen.contains(&object.mesh) {
            continue;
        }
        seen.push(object.mesh);
        if let Some(mesh) = scene.mesh(object.mesh, &doc.mesh) {
            out.push((mesh, object.scale));
        }
    }
    out
}

/// The line under a rebuild's button: what it will lose, of the selected
/// object (`all` false) or of every selected object (`all`, for Join and the
/// booleans). Nothing is drawn when it would lose nothing.
pub fn hint(ui: &mut Ui, doc: &Doc, what: Rebuild, all: bool) {
    let edits = doc.history.edits;
    let meshes: Vec<&Mesh> = if all { selected_meshes(doc).into_iter().map(|(m, _)| m).collect() } else { vec![&doc.mesh] };
    let mut has = Held::default();
    for mesh in meshes {
        let h = held(ui, mesh, edits, false);
        has.levels |= h.levels;
        has.layers |= h.layers;
        has.mask |= h.mask;
        has.hidden |= h.hidden;
    }
    if let Some(text) = words(what, &has) {
        theme::hint(ui, &text);
    }
}

/// "Drops the subdivision levels and clears the mask. Undo (Ctrl+Z) brings them
/// back.", or `None` when nothing would be lost.
fn words(what: Rebuild, has: &Held) -> Option<String> {
    let mut lost: Vec<&str> = Vec::new();
    if has.levels {
        lost.push("drops the subdivision levels");
    }
    if has.layers {
        lost.push("merges the sculpt layers into the model");
    }
    if has.mask && what == Rebuild::Surface {
        lost.push("clears the mask");
    }
    if has.hidden && what != Rebuild::Decimate {
        lost.push("shows the hidden parts again");
    }
    let (last, rest) = lost.split_last()?;
    let list = if rest.is_empty() { (*last).to_owned() } else { format!("{} and {last}", rest.join(", ")) };
    let mut text = list;
    text[..1].make_ascii_uppercase();
    Some(format!("{text}. Undo (Ctrl+Z) brings it all back."))
}

/// "About 1.2M triangles." under a remesh resolution slider, for the
/// selected object at `resolution`; or what the most is, where that is too
/// many for a remesh.
pub fn remesh_estimate(ui: &mut Ui, doc: &Doc, resolution: f32) {
    let mesh = &doc.mesh;
    let area = held(ui, mesh, doc.history.edits, true).area.unwrap_or(0.0);
    let longest = mesh.bounds().size().max_element() as f64;
    estimate_line(ui, area, longest, resolution, false);
}

/// The same under the boolean's slider, counted along the bright object's
/// longest side as the boolean is, over the surface of every selected
/// object: the result has no more than that, so it is an upper end.
pub fn boolean_estimate(ui: &mut Ui, doc: &Doc, resolution: f32) {
    let parts = selected_meshes(doc);
    let Some(&(bright, scale)) = parts.first() else { return };
    let longest = (bright.bounds().size() * scale.abs()).max_element() as f64;
    let area = world_area(ui, doc, &parts);
    estimate_line(ui, area, longest, resolution, true);
}

/// The same under Join's remesh slider, along the longest side of what is
/// joined.
pub fn join_estimate(ui: &mut Ui, doc: &Doc, resolution: f32) {
    let parts = selected_meshes(doc);
    let longest = doc.scene.selection_bounds(&doc.mesh).size().max_element() as f64;
    let area = world_area(ui, doc, &parts);
    estimate_line(ui, area, longest, resolution, true);
}

/// The surface of these meshes at the scale each is shown at. A scale that
/// is not the same on every axis is taken at its mean, near enough for an
/// estimate.
fn world_area(ui: &Ui, doc: &Doc, parts: &[(&Mesh, Vec3)]) -> f64 {
    parts
        .iter()
        .map(|(mesh, scale)| {
            let s = scale.abs();
            let mean = ((s.x * s.y * s.z) as f64).cbrt();
            held(ui, mesh, doc.history.edits, true).area.unwrap_or(0.0) * mean * mean
        })
        .sum()
}

fn estimate_line(ui: &mut Ui, area: f64, longest: f64, resolution: f32, upper: bool) {
    let resolution = resolution.round() as u32;
    let Some(text) = estimate_words(area, longest, resolution, upper) else { return };
    theme::hint(ui, &text);
}

fn estimate_words(area: f64, longest: f64, resolution: u32, upper: bool) -> Option<String> {
    let (_, triangles) = budgeted_resolution(area, longest, resolution, usize::MAX);
    if triangles == 0 {
        return None;
    }
    let (fits, _) = budgeted_resolution(area, longest, resolution, MAX_TRIANGLES);
    if fits < resolution.clamp(MIN_RESOLUTION, MAX_RESOLUTION) {
        return Some(format!(
            "About {} triangles: more than a remesh may make ({} million). {fits} or lower will do.",
            fmt_count(triangles),
            MAX_TRIANGLES / 1_000_000
        ));
    }
    let about = if upper { "Up to about" } else { "About" };
    Some(format!("{about} {} triangles.", fmt_count(triangles)))
}

/// The line under Quad Remesh when the selected object has symmetry on: a
/// quad remesh does not keep it.
pub fn quad_symmetry_hint(ui: &mut Ui, doc: &Doc) {
    let symmetry = doc.scene.active_symmetry();
    if symmetry.mirror.iter().any(|&on| on) || symmetry.radial.on {
        theme::hint(ui, "Quad Remesh ignores symmetry: the two sides may not match triangle for triangle afterwards.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(levels: bool, layers: bool, mask: bool, hidden: bool) -> Held {
        Held { levels, layers, mask, hidden, ..Held::default() }
    }

    #[test]
    fn a_hint_names_only_what_that_rebuild_loses() {
        assert_eq!(words(Rebuild::Surface, &has(false, false, false, false)), None, "nothing to lose: no line");
        assert_eq!(
            words(Rebuild::Surface, &has(true, true, true, true)).unwrap(),
            "Drops the subdivision levels, merges the sculpt layers into the model, clears the mask and shows the \
             hidden parts again. Undo (Ctrl+Z) brings it all back."
        );
        // Decimate keeps the mask and what is hidden.
        assert_eq!(words(Rebuild::Decimate, &has(false, false, true, true)), None);
        assert_eq!(
            words(Rebuild::Decimate, &has(true, false, true, true)).unwrap(),
            "Drops the subdivision levels. Undo (Ctrl+Z) brings it all back."
        );
        // A plain Join keeps each mask, and shows what was hidden.
        assert_eq!(
            words(Rebuild::Join, &has(false, false, true, true)).unwrap(),
            "Shows the hidden parts again. Undo (Ctrl+Z) brings it all back."
        );
    }

    #[test]
    fn the_estimate_says_when_a_resolution_is_too_fine() {
        // A unit ball: its surface is 4 pi over a longest side of 2.
        let area = 4.0 * std::f64::consts::PI;
        let fine = estimate_words(area, 2.0, 128, false).unwrap();
        assert!(fine.starts_with("About "), "{fine}");
        let (_, tris) = budgeted_resolution(area, 2.0, 128, usize::MAX);
        assert!(tris > 10_000 && tris < 1_000_000, "{tris} triangles at 128");
        assert!(estimate_words(area, 2.0, 1024, false).is_some());
        assert_eq!(estimate_words(0.0, 2.0, 128, false), None, "nothing to measure: no line");
        assert!(estimate_words(area, 2.0, 128, true).unwrap().starts_with("Up to about "));
    }
}
