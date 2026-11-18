//! Subdivision levels on the web: the Subdivision card on the Mesh tab and
//! its keys (Ctrl+D, Page Up, Page Down).
//!
//! What a level is and how a mesh holds its levels is the desktop's
//! `mesh/levels.rs`, compiled here as it is; the geometry is the
//! `sculpt-multires` crate. This file is the desktop's `ui/levels.rs` and
//! `app/levels.rs` cut down to the web version:
//!
//! - A change of level is a new mesh in the object's place, one undo step
//!   that keeps the old mesh whole (`Step::Mesh`), as a remesh is. The old
//!   mesh carries its own levels, so undo and redo are a swap.
//! - Deleting levels leaves the mesh as it is and changes only its levels:
//!   a step that keeps the levels as they were (`Step::Levels`).
//! - Whatever else puts a new mesh in the object's place (Remesh, Decimate,
//!   a new shape, a tool that rebuilds it) drops the levels, and the status
//!   line says so (`GolemApp::rebuilt_note`); Ctrl+Z brings them back.
//! - Dynamic topology does nothing on a mesh with levels (`Mesh::refine`),
//!   and the Remesh card says so.
//! - The browser keeps a level at most `MAX_TRIANGLES` triangles: a tab
//!   runs short of memory long before the desktop's limit.

use eframe::egui::{self, Color32, Ui};
use sculpt_multires::Split;

use super::layers::megabytes;
use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use super::{GolemApp, MAX_TRIANGLES, Step, group_digits, tidy};
use crate::mesh::{Mesh, levels};

/// The card's colour on the panel, as on the desktop.
const TINT: Color32 = Color32::from_rgb(170, 150, 235);
const WARNING: Color32 = Color32::from_rgb(242, 190, 77);

/// What the card asks for.
enum LevelAction {
    Subdivide,
    To(usize),
    By(i32),
    Delete { lower: bool },
}

/// "level 2 of 4".
fn place(mesh: &Mesh) -> String {
    let (level, count) = mesh.levels.as_ref().map_or((0, 1), |l| (l.current(), l.count()));
    format!("level {} of {count}", level + 1)
}

fn fmt_count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.0}k", n as f32 / 1e3),
        _ => format!("{:.2}M", n as f32 / 1e6),
    }
}

/// About what a Subdivide to `after` triangles adds to the memory the page
/// takes, as the desktop works it out: the levels hold about 35 bytes for
/// each triangle of the finest level and the model about 80, less what the
/// levels the object has already hold.
fn subdivide_bytes(mesh: &Mesh, after: usize) -> usize {
    let held = mesh.levels.as_ref().map_or(0, |l| l.whole_bytes());
    (after * (35 + 80)).saturating_sub(held)
}

/// Why the active mesh cannot be subdivided here, if it cannot.
fn refusal(mesh: &Mesh) -> Option<String> {
    if let Some(why) = levels::subdivide_refusal(mesh) {
        return Some(why);
    }
    let after = levels::triangles_after(mesh);
    (after > MAX_TRIANGLES).then(|| {
        format!(
            "Subdividing would make {:.1} million triangles; a browser tab keeps at most {} million.",
            after as f64 / 1e6,
            MAX_TRIANGLES / 1_000_000
        )
    })
}

impl GolemApp {
    /// Subdivide: a finer level above the finest, and the object moves to it.
    pub(super) fn subdivide(&mut self) {
        self.layer_strength_done();
        if let Some(why) = refusal(&self.mesh) {
            self.status = why;
            return;
        }
        let began = web_time::Instant::now();
        let split = if self.subdivide_smooth { Split::Smooth } else { Split::Flat };
        // Without what dynamic topology's merges left: levels number every vertex.
        let tidied = tidy(&self.mesh);
        match levels::subdivided(tidied.as_ref().unwrap_or(&self.mesh), split) {
            Ok((mesh, note)) => {
                let mut text = format!(
                    "Subdivided to {}: {} triangles, in {:.1} s.",
                    place(&mesh),
                    group_digits(mesh.live_triangle_count()),
                    began.elapsed().as_secs_f32()
                );
                if note.quads_set_aside {
                    text += " Its quads were not its triangles cut up, so it was subdivided as triangles.";
                }
                self.put_level(mesh, text);
            }
            Err(why) => self.status = why,
        }
    }

    /// Page Up and Page Down: `by` levels finer, or coarser if negative.
    pub(super) fn level_by(&mut self, by: i32) {
        let Some((level, top)) = self.mesh.levels.as_ref().map(|l| (l.current(), l.top())) else {
            self.status = "This object has no subdivision levels yet. Subdivide (Ctrl+D) adds one.".into();
            return;
        };
        let to = (level as i64 + by as i64).clamp(0, top as i64) as usize;
        if to == level {
            self.status = if by > 0 {
                "This is the finest level. Subdivide (Ctrl+D) adds a finer one.".into()
            } else {
                "This is the coarsest level.".into()
            };
            return;
        }
        self.level_to(to);
    }

    /// Moves the active object to level `level` (0 is the coarsest). What
    /// was sculpted on the level it leaves goes with it.
    fn level_to(&mut self, level: usize) {
        self.layer_strength_done();
        let Some(from) = self.mesh.levels.as_ref().map(|l| l.current()) else { return };
        if level == from {
            return;
        }
        let began = web_time::Instant::now();
        match levels::stepped(&self.mesh, level) {
            Ok((mesh, carried)) => {
                let mut text = format!(
                    "On {}: {} triangles ({:.0} ms).",
                    place(&mesh),
                    group_digits(mesh.live_triangle_count()),
                    began.elapsed().as_secs_f64() * 1000.0
                );
                if carried {
                    text += &format!(" What was sculpted on level {} was carried into the finer levels.", from + 1);
                }
                self.put_level(mesh, text);
            }
            Err(why) => self.status = why,
        }
    }

    /// A mesh on another level in the active object's place, as one undo step.
    fn put_level(&mut self, mesh: Mesh, text: String) {
        let before = std::mem::replace(&mut self.mesh, mesh);
        self.push_undo(Step::Mesh(self.active, Box::new(before)));
        self.upload_whole = true;
        self.mark_changed();
        self.status = text;
    }

    /// Delete Lower and Delete Higher: drops the levels below the one the
    /// active object is on, or above it. The mesh on screen does not change.
    fn delete_levels(&mut self, lower: bool) {
        self.layer_strength_done();
        let which = if lower { "lower" } else { "higher" };
        let Some((level, count)) = self.mesh.levels.as_ref().map(|l| (l.current(), l.count())) else {
            self.status = "This object has no subdivision levels to delete.".into();
            return;
        };
        let gone = if lower { level } else { count - 1 - level };
        if gone == 0 {
            self.status = format!("There are no {which} levels: this is {}.", place(&self.mesh));
            return;
        }
        let fewer = levels::without(&self.mesh, lower);
        let before = std::mem::replace(&mut self.mesh.levels, fewer);
        self.push_undo(Step::Levels(self.active, before));
        self.mark_changed();
        let left = match &self.mesh.levels {
            Some(_) => format!("It is on {} now.", place(&self.mesh)),
            None => "It has no levels now: it is an ordinary mesh.".to_owned(),
        };
        let levels = if gone == 1 { "level" } else { "levels" };
        self.status = format!("Deleted the {gone} {which} {levels}. {left} Ctrl+Z brings them back.");
    }

    /// What to add to the status line when a command put a new mesh in the
    /// active object's place and `before`'s levels or layers did not come
    /// with it; empty when nothing was lost.
    pub(super) fn rebuilt_note(before: &Mesh, after: &Mesh) -> String {
        let levels = before.level_count() > 1 && after.level_count() <= 1;
        let layers = before.layer_count() > 0 && after.layer_count() == 0;
        match (levels, layers) {
            (true, true) => " Its subdivision levels were dropped and its layers merged into the model. Ctrl+Z brings them back.",
            (true, false) => " Its subdivision levels were dropped. Ctrl+Z brings them back.",
            (false, true) => " Its sculpt layers were merged into the model. Ctrl+Z brings them back.",
            (false, false) => "",
        }
        .to_owned()
    }

    /// Ctrl+D, Page Up and Page Down, while sculpting.
    pub(super) fn level_keys(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        let (sub, up, down) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::D),
                i.consume_key(Modifiers::NONE, Key::PageUp),
                i.consume_key(Modifiers::NONE, Key::PageDown),
            )
        });
        if sub {
            self.subdivide();
        } else if up {
            self.level_by(1);
        } else if down {
            self.level_by(-1);
        }
    }

    // ---- the card ---------------------------------------------------------------

    /// The Subdivision card (Mesh tab), as the desktop's.
    pub(super) fn levels_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let mesh = &self.mesh;
        let place = mesh.levels.as_ref().map(|l| (l.current(), l.count()));
        let summary = place.map_or(String::new(), |(level, count)| format!("Level {} of {count}", level + 1));
        let mut actions = Vec::new();
        let mut smooth = self.subdivide_smooth;
        theme::category(ui, "Subdivision", TINT, &summary, true, |ui| {
            theme::about(
                ui,
                "Levels of the same model, each with four times the triangles of the one below. Shape the big forms on a \
                 low level and the detail on a high one: what is sculpted on one is kept on the others.\n\n\
                 A big change made on a low level comes back up with the fine detail riding on it.",
            );
            let refused = refusal(mesh);
            ui.add_enabled_ui(refused.is_none(), |ui| {
                let style = ButtonStyle { badge: "Ctrl+D", active: Some(ACCENT()), ..Default::default() };
                let help = "Adds a level with four times the triangles above the finest one and moves to it (Ctrl+D). \
                            The model is rounded a little as it is divided; tick Flat to keep every point where it is. \
                            Ctrl+Z takes the level away again.";
                if tip(theme::button(ui, ui.available_width(), "Subdivide", style), tips, help).clicked() {
                    actions.push(LevelAction::Subdivide);
                }
            });
            match &refused {
                Some(why) => theme::hint(ui, why),
                None => {
                    let (now, after) = (mesh.live_triangle_count(), levels::triangles_after(mesh));
                    let more = megabytes(subdivide_bytes(mesh, after));
                    theme::hint(ui, &format!("{} to {} triangles: about {more} more memory.", fmt_count(now), fmt_count(after)));
                }
            }
            let mut flat = !smooth;
            let help = "Subdivide only cuts each face into four and moves nothing: for hard-edged shapes that must keep \
                        their flat faces and sharp corners.";
            tip(theme::checkbox(ui, &mut flat, "Flat (cut the faces, move nothing)", ACCENT()), tips, help);
            smooth = !flat;

            let Some((level, count)) = place else { return };
            let Some(stack) = mesh.levels.as_ref() else { return };
            ui.add_space(6.0);
            for row in (0..count).rev() {
                let flat = if stack.is_flat(row) { ", flat" } else { "" };
                let label = format!("Level {}   {} triangles{flat}", row + 1, fmt_count(stack.triangle_count(row)));
                let style = ButtonStyle { active: (row == level).then(ACCENT), height: 24.0, ..Default::default() };
                let help = if row == level {
                    "The level the object is on: what you sculpt or save now is this level."
                } else {
                    "Go to this level. What was sculpted on the level you leave goes with you."
                };
                if tip(theme::button(ui, ui.available_width(), &label, style), tips, help).clicked() && row != level {
                    actions.push(LevelAction::To(row));
                }
            }
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                ui.add_enabled_ui(level > 0, |ui| {
                    let style = ButtonStyle { badge: "PgDn", ..Default::default() };
                    let help = "One level coarser (Page Down): fewer, bigger triangles, for changing the big forms. The \
                                detail of the finer levels is kept and comes back when you go up again.";
                    if tip(theme::button(ui, w, "Lower", style), tips, help).clicked() {
                        actions.push(LevelAction::By(-1));
                    }
                });
                ui.add_enabled_ui(level + 1 < count, |ui| {
                    let style = ButtonStyle { badge: "PgUp", ..Default::default() };
                    let help = "One level finer (Page Up). What you changed on this level is carried up: the finer \
                                detail moves and turns with the surface under it.";
                    if tip(theme::button(ui, w, "Higher", style), tips, help).clicked() {
                        actions.push(LevelAction::By(1));
                    }
                });
            });
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                ui.add_enabled_ui(level > 0, |ui| {
                    let help = "Throws away the levels below this one, so this becomes the coarsest. The model does not \
                                change. Ctrl+Z brings them back.";
                    if tip(theme::button(ui, w, "Delete Lower", ButtonStyle::default()), tips, help).clicked() {
                        actions.push(LevelAction::Delete { lower: true });
                    }
                });
                ui.add_enabled_ui(level + 1 < count, |ui| {
                    let help = "Throws away the levels above this one, and the detail that was sculpted on them, so this \
                                becomes the finest. Ctrl+Z brings them back.";
                    if tip(theme::button(ui, w, "Delete Higher", ButtonStyle::default()), tips, help).clicked() {
                        actions.push(LevelAction::Delete { lower: false });
                    }
                });
            });
            let bytes = stack.whole_bytes();
            theme::hint(ui, &format!("The levels take {} of memory beside the model itself.", megabytes(bytes)));
        });
        self.subdivide_smooth = smooth;
        for action in actions {
            match action {
                LevelAction::Subdivide => self.subdivide(),
                LevelAction::To(level) => self.level_to(level),
                LevelAction::By(by) => self.level_by(by),
                LevelAction::Delete { lower } => self.delete_levels(lower),
            }
        }
    }

    /// Under Dynamic Topology: why it does nothing on this object, when that
    /// is because of its levels.
    pub(super) fn dyntopo_levels_note(&self, ui: &mut Ui) {
        if self.mesh.level_count() > 1 {
            let text = "Off for this object while it has subdivision levels: its levels need its triangles to stay as \
                        they are. Subdivide adds detail instead.";
            ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(11.0)).italics().color(WARNING)).wrap());
        }
    }
}
