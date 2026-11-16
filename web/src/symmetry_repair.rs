//! Symmetry Repair on the web: the desktop's fold under the Symmetry card
//! (`src/ui/symmetry.rs`), with its four buttons and Match Tolerance.
//!
//! - **Check Symmetry** counts the vertices that pair up across the object's
//!   own plane and says whether the two sides are mirror images.
//! - **Make Symmetric** moves one side onto the mirror image of the other,
//!   each vertex onto its partner: no triangle changes.
//! - **Cut & Mirror** throws one side away and rebuilds it as the mirror
//!   image of the other.
//! - **Symmetry Cleanup** puts the vertices near the plane exactly on it and
//!   merges the doubled ones that leaves.
//!
//! What they compute is the desktop's own `sculpt_symmetry::ops`; the lines
//! they say are the desktop's (`src/app/symmetry_repair.rs`,
//! `src/app/cut_mirror.rs`). What differs is only how they run: the desktop
//! runs them as background jobs, the web one after the other, since the page
//! has one thread to draw with and these take well under a second at the
//! sizes a browser holds.
//!
//! Each change is one undo step: a stroke when only vertices move (Make
//! Symmetric, a Cleanup with nothing to merge), the old mesh whole when the
//! mesh is rebuilt.

use eframe::egui::Ui;
use glam::Vec3;
use sculpt_symmetry::ops::{self, CleanupChange};
use sculpt_symmetry::{Axis, Moved};

use super::GolemApp;
use super::ui::theme::{self, ButtonStyle};
use super::ui::tip;
use crate::mesh::Mesh;
use crate::mesh::groups::{self, Fit, Source};
use crate::mesh::layers::Ride;
use crate::mesh::paint::transfer as paint;
use crate::printable::{self, Cure};

/// Match Tolerance where the slider starts: the tolerance the mesh itself
/// suggests, about half a triangle's width.
pub(crate) const DEFAULT_TOLERANCE: f32 = 1.0;

/// How far apart the two ends of a pair may be from mirror images of each
/// other and still be called so by Check Symmetry, as a share of the model's
/// size (the desktop's `OUT_OF_TRUE`).
const OUT_OF_TRUE: f32 = 1e-5;

/// The card's own settings: the plane, and Match Tolerance. Neither is kept
/// between visits, as on the desktop.
pub(crate) struct Repair {
    pub axis: Axis,
    pub tolerance: f32,
}

impl Default for Repair {
    fn default() -> Repair {
        Repair { axis: Axis::X, tolerance: DEFAULT_TOLERANCE }
    }
}

impl Repair {
    fn tolerance_scale(&self) -> f32 {
        let t = self.tolerance;
        if t.is_finite() { t.clamp(ops::MIN_TOLERANCE_SCALE, ops::MAX_TOLERANCE_SCALE) } else { DEFAULT_TOLERANCE }
    }
}

/// A button pressed on the card.
pub(crate) enum Pressed {
    Check,
    Make(bool),
    Cut(bool),
    Cleanup,
}

/// `text` as a sentence of the status line.
fn sentence(text: String) -> String {
    if text.ends_with('.') { text } else { text + "." }
}

/// Check Symmetry's line, and whether the mesh is symmetric (the desktop's
/// `symmetry_across`).
pub(crate) fn symmetry_across(positions: &[Vec3], indices: &[u32], name: &str, axis: Axis, tolerance_scale: f32) -> (String, bool) {
    let Some(map) = ops::build_map(positions, indices, axis, tolerance_scale) else {
        return (format!("\"{name}\": {}.", ops::status(positions, indices, axis, tolerance_scale)), false);
    };
    let mut text = format!("\"{name}\" across {}.", map.summary());
    if map.is_symmetric() {
        let apart = (0..positions.len() as u32).filter_map(|v| {
            let partner = map.partner_of(v).filter(|&partner| partner > v)?;
            Some(positions[v as usize].distance(axis.reflect(positions[partner as usize])))
        });
        let allowed = map.diagonal() * OUT_OF_TRUE;
        let (out, furthest) = apart.filter(|&d| d > allowed).fold((0, 0.0f32), |(n, far), d| (n + 1, far.max(d)));
        if out == 0 {
            text += " Every pair is the mirror image of the other.";
            return (text, true);
        }
        let share = 100.0 * furthest / map.diagonal().max(f32::MIN_POSITIVE);
        text += &format!(
            " The vertices all pair up, but {out} of the pairs are not mirror images of each other, the furthest by \
             {share:.2}% of the model's size. Make Symmetric lines them up."
        );
        return (text, false);
    }
    let off_plane = map.vertex_count() - map.on_plane_count();
    let too_many = map.unmatched_count() as f32 > off_plane as f32 * ops::MAX_UNMATCHED_FRACTION;
    text += if too_many {
        " Too many to match up vertex for vertex: raise Match Tolerance, or use Cut & Mirror, which rebuilds one \
         side from the other."
    } else {
        " Make Symmetric can match the two sides up."
    };
    (text, false)
}

/// `mesh` cut at its own `axis` plane and rebuilt as the side kept and its
/// mirror image (the desktop's `cut_and_mirrored_at`).
pub(crate) fn cut_and_mirrored(mesh: &Mesh, axis: Axis, tolerance_scale: f32, from_positive: bool) -> Result<(Mesh, String), String> {
    if mesh.triangle_count() == 0 {
        return Err("Nothing to cut and mirror.".into());
    }
    let built = ops::mirror_and_weld(&mesh.positions, &mesh.indices, Some(&mesh.mask), axis, tolerance_scale, from_positive)?;
    let mut text = format!("{}.", built.message());
    if mesh.any_hidden() {
        text += " Its hidden parts were cut with the rest, and everything is showing again.";
    }
    let mut cut = Mesh::new(built.mesh.positions, built.mesh.indices, built.mask);
    mirrored_groups(mesh, &mut cut, axis as usize, from_positive);
    Ok((cut, text))
}

/// Face groups and paint onto a mesh rebuilt as one side and its mirror
/// image: the side kept keeps its own, the other side gets their mirror
/// image (the desktop's `groups::mirrored`).
fn mirrored_groups(old: &Mesh, new: &mut Mesh, axis: usize, positive: bool) {
    if old.has_groups() {
        let source = Source { fold: Some((axis, positive)), ..Source::of(old) };
        groups::carry(new, &[source], Fit::Part);
    }
    if old.has_paint() {
        let source = Source { fold: Some((axis, positive)), ..Source::of(old) };
        paint::carry(new, &[source], Fit::Part);
    }
}

/// Moves the vertices `moved` lists as one stroke, and returns its undo
/// record (the desktop's `move_as_stroke`). Hidden vertices are passed over.
fn move_as_stroke(mesh: &mut Mesh, moved: &Moved) -> Option<crate::mesh::StrokeDelta> {
    if moved.is_empty() {
        return None;
    }
    mesh.layer_ride = Ride::Still;
    let leaves = mesh.leaves_of(&moved.indices);
    mesh.begin_stroke();
    mesh.move_listed(&moved.indices, |k, _| Some(moved.positions[k]));
    mesh.positions_changed(&leaves);
    mesh.end_stroke(false)
}

/// What the line adds about hidden vertices a move left where they were.
fn hidden_note(mesh: &Mesh, moved: &Moved) -> String {
    match moved.indices.iter().filter(|&&v| mesh.is_vertex_hidden(v)).count() {
        0 => String::new(),
        1 => " 1 of those vertices is hidden and was left where it was.".to_owned(),
        n => format!(" {n} of those vertices are hidden and were left where they were."),
    }
}

/// The Symmetry Repair fold, under the Symmetry card's buttons: the button
/// pressed, if any, for `GolemApp::symmetry_repair` to carry out once the
/// card is drawn.
pub(crate) fn section(ui: &mut Ui, repair: &mut Repair, tips: bool) -> Option<Pressed> {
        if !theme::fold(ui, "Symmetry Repair", false, false) {
            return None;
        }
        theme::about(
            ui,
            "For an object whose two sides no longer match. Check Symmetry counts the vertices that pair up across the \
             plane. Make Symmetric moves one side onto the mirror image of the other and keeps every triangle and the \
             mask; Cut & Mirror throws one side away and rebuilds it as the mirror image of the other. Symmetry \
             Cleanup puts the vertices near the plane exactly on it.",
        );
        let mut pressed = None;
        theme::hint(ui, "Symmetry Plane");
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            for choice in Axis::ALL {
                let active = (repair.axis == choice).then_some(theme::AXIS[choice.index()]);
                let style = ButtonStyle { active, ..Default::default() };
                let help = format!(
                    "The buttons below work across the object's own {} plane: the one Mirror {} sculpts across.",
                    choice.name(),
                    choice.name()
                );
                if tip(theme::button(ui, w, choice.name(), style), tips, &help).clicked() {
                    repair.axis = choice;
                }
            }
        });
        let axis = repair.axis;

        // Equal travel doubles the tolerance, so where it starts there is room either way.
        let range = ops::MIN_TOLERANCE_SCALE..=ops::MAX_TOLERANCE_SCALE;
        let mut log = repair.tolerance.clamp(*range.start(), *range.end()).log2();
        let ends = range.start().log2()..=range.end().log2();
        let slider = theme::slider(ui, "Match Tolerance", &mut log, ends, |v| tolerance_text(v.exp2()));
        let help = "How far apart two vertices may be and still count as a mirrored pair, from tight to loose. Widen it \
                    if halves that were built separately are not matching up. For Cut & Mirror it is how near the plane \
                    a vertex must be to be kept as part of the seam.";
        if tip(slider, tips, help).changed() {
            repair.tolerance = log.exp2().clamp(*range.start(), *range.end());
        }

        let check = theme::button(ui, ui.available_width(), "Check Symmetry", ButtonStyle::default());
        let help = "Says on the status line how many of the object's vertices pair up across the plane, and how many \
                    have no partner. Changes nothing.";
        if tip(check, tips, help).clicked() {
            pressed = Some(Pressed::Check);
        }

        theme::heading(ui, "Make Symmetric");
        side_buttons(ui, axis, tips, &mut pressed, Pressed::Make, |copied, moved| {
            format!(
                "Moves the {moved} side's vertices onto the mirror image of the {copied} side, each onto its own partner \
                 across the object's own {} plane. No triangle changes. With a mask painted only the unmasked part \
                 moves. Where too much of the model has no partner nothing is changed and the status line says so: \
                 Cut & Mirror is for that. One undo step (Ctrl+Z).",
                axis.name()
            )
        });

        theme::heading(ui, "Cut & Mirror");
        side_buttons(ui, axis, tips, &mut pressed, Pressed::Cut, |kept, rebuilt| {
            format!(
                "Keeps the {kept} side of the object and rebuilds its {rebuilt} side as the exact mirror image of it, \
                 across the object's own {} plane. What was on the {rebuilt} side is thrown away. The mask comes \
                 across; hidden parts are cut too and show again. One undo step (Ctrl+Z).",
                axis.name()
            )
        });

        ui.add_space(4.0);
        let cleanup = theme::button(ui, ui.available_width(), "Symmetry Cleanup", ButtonStyle::default());
        let help = "Puts the vertices that sit close to the plane exactly on it, and merges the doubled ones that leaves: \
                    two halves joined along the plane become one surface. Neither side is moved as a whole. One undo \
                    step (Ctrl+Z).";
        if tip(cleanup, tips, help).clicked() {
            pressed = Some(Pressed::Cleanup);
        }

        pressed
}

impl GolemApp {
    /// Carries out a button of the Symmetry Repair fold.
    pub(crate) fn symmetry_repair(&mut self, pressed: Pressed) {
        if self.sculptor.is_active() {
            return;
        }
        let axis = self.repair.axis;
        match pressed {
            Pressed::Check => self.check_symmetry(axis),
            Pressed::Make(from_positive) => self.make_symmetric(axis, from_positive),
            Pressed::Cut(from_positive) => self.cut_and_mirror(axis, from_positive),
            Pressed::Cleanup => self.symmetry_cleanup(axis),
        }
    }

    fn active_name(&self) -> String {
        self.active_object().name.clone()
    }

    /// Check Symmetry: a line on the status bar, nothing changed.
    pub(crate) fn check_symmetry(&mut self, axis: Axis) {
        let name = self.active_name();
        let tidied = super::tidy(&self.mesh);
        let m = tidied.as_ref().unwrap_or(&self.mesh);
        let (text, _) = symmetry_across(&m.positions, &m.indices, &name, axis, self.repair.tolerance_scale());
        self.status = text;
    }

    /// Make Symmetric: one side onto the mirror image of the other, as one
    /// stroke (or, on a mesh with dynamic topology's leftovers, as the whole
    /// mesh, tidied, since a stroke numbers vertices as they are).
    pub(crate) fn make_symmetric(&mut self, axis: Axis, from_positive: bool) {
        let tolerance = self.repair.tolerance_scale();
        match super::tidy(&self.mesh) {
            None => {
                let m = &self.mesh;
                match ops::make_symmetric(&m.positions, &m.indices, Some(&m.mask), axis, tolerance, from_positive) {
                    Ok(done) => {
                        let text = sentence(done.message()) + &hidden_note(&self.mesh, &done.moved);
                        if let Some(delta) = move_as_stroke(&mut self.mesh, &done.moved) {
                            self.stroke_done(delta);
                        }
                        self.status = text;
                    }
                    Err(why) => self.status = why,
                }
            }
            Some(mut tidied) => {
                match ops::make_symmetric(&tidied.positions, &tidied.indices, Some(&tidied.mask), axis, tolerance, from_positive) {
                    Ok(done) => {
                        let text = sentence(done.message()) + &hidden_note(&tidied, &done.moved);
                        if move_as_stroke(&mut tidied, &done.moved).is_some() {
                            self.replace_mesh(tidied, "Make Symmetric");
                        }
                        self.status = text;
                    }
                    Err(why) => self.status = why,
                }
            }
        }
    }

    /// Cut & Mirror: the object rebuilt as one side and its mirror image.
    pub(crate) fn cut_and_mirror(&mut self, axis: Axis, from_positive: bool) {
        let tidied = super::tidy(&self.mesh);
        let m = tidied.as_ref().unwrap_or(&self.mesh);
        match cut_and_mirrored(m, axis, self.repair.tolerance_scale(), from_positive) {
            Ok((mesh, text)) => {
                let name = self.active_name();
                let census = printable::census(&mesh);
                self.replace_mesh(mesh, "Cut & Mirror");
                self.status = match printable::warning(&[(&name, census)], Cure::Remesh) {
                    Some(warning) => format!("{text} {warning}"),
                    None => text,
                };
            }
            Err(why) => self.status = why,
        }
    }

    /// Symmetry Cleanup: vertices near the plane put on it, doubled ones merged.
    pub(crate) fn symmetry_cleanup(&mut self, axis: Axis) {
        let tolerance = self.repair.tolerance_scale();
        let tidied = super::tidy(&self.mesh);
        let m = tidied.as_ref().unwrap_or(&self.mesh);
        let cleaned = match ops::cleanup(&m.positions, &m.indices, Some(&m.mask), axis, tolerance) {
            Ok(cleaned) => cleaned,
            Err(why) => {
                self.status = why;
                return;
            }
        };
        let mut text = sentence(cleaned.message());
        match cleaned.change {
            CleanupChange::Nothing => self.status = text,
            CleanupChange::Moved(moved) => match tidied {
                None => {
                    text += &hidden_note(&self.mesh, &moved);
                    if let Some(delta) = move_as_stroke(&mut self.mesh, &moved) {
                        self.stroke_done(delta);
                    }
                    self.status = text;
                }
                Some(mut tidied) => {
                    text += &hidden_note(&tidied, &moved);
                    if move_as_stroke(&mut tidied, &moved).is_some() {
                        self.replace_mesh(tidied, "Symmetry Cleanup");
                    }
                    self.status = text;
                }
            },
            CleanupChange::Rebuilt { mesh: built, mask, .. } => {
                if m.any_hidden() {
                    text += " Its hidden parts were cleaned up with the rest, and everything is showing again.";
                }
                let mesh = Mesh::new(built.positions, built.indices, mask);
                let name = self.active_name();
                let census = printable::census(&mesh);
                self.replace_mesh(mesh, "Symmetry Cleanup");
                self.status = match printable::warning(&[(&name, census)], Cure::Remesh) {
                    Some(warning) => format!("{text} {warning}"),
                    None => text,
                };
            }
        }
    }
}

/// A button for each side of the plane that can be the one a repair copies:
/// "+X to -X" and "-X to +X". `help` is given the side copied and the side
/// changed, in that order.
fn side_buttons(
    ui: &mut Ui,
    axis: Axis,
    tips: bool,
    pressed: &mut Option<Pressed>,
    action: impl Fn(bool) -> Pressed,
    help: impl Fn(&str, &str) -> String,
) {
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        for from_positive in [true, false] {
            let (from, to) = cut_sides(axis, from_positive);
            let label = format!("{from} to {to}");
            if tip(theme::button(ui, w, &label, ButtonStyle::default()), tips, &help(&from, &to)).clicked() {
                *pressed = Some(action(from_positive));
            }
        }
    });
}

/// The side a repair copies and the side it changes: "+X" and "-X".
fn cut_sides(axis: Axis, from_positive: bool) -> (String, String) {
    let (plus, minus) = (format!("+{}", axis.name()), format!("-{}", axis.name()));
    if from_positive { (plus, minus) } else { (minus, plus) }
}

/// Match Tolerance as the slider shows it: "1x", "0.25x", "2.83x".
fn tolerance_text(scale: f32) -> String {
    let text = format!("{scale:.2}");
    format!("{}x", text.trim_end_matches('0').trim_end_matches('.'))
}

