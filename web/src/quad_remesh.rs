//! Quad Remesh on the web: the Quads slider and the Quad Remesh button in the
//! Remesh card, under Dynamic Topology, as on the desktop (`ui/mod.rs`), with
//! the same words. The remesher is the desktop's `src/quadremesh/` (compiled
//! in as `crate::quadremesh`).
//!
//! It takes seconds on a big model, so it does not run inside a click:
//!
//! - In the many-threads build it runs on one of the page's worker threads,
//!   and the page goes on drawing; the status line counts the seconds. If the
//!   model is changed (or another object picked) before it is done, its
//!   result is not used, and the status line says so.
//! - In the one-thread builds the page has no other thread to run it on: the
//!   status line first says it is working and that the page will be still
//!   until it is done, that line is drawn, and then it runs.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Ui};

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use super::GolemApp;
use crate::mesh::Mesh;
use crate::quadremesh::{self, Report};

type Outcome = Result<(Mesh, Report), String>;

/// The Quads setting and the remesh under way.
pub(crate) struct QuadState {
    pub target: f32,
    job: Option<Job>,
}

impl Default for QuadState {
    fn default() -> QuadState {
        // The desktop's own starting number.
        QuadState { target: 5000.0, job: None }
    }
}

struct Job {
    /// The object it was started on, and its mesh as it was then.
    object: u64,
    mesh: (u64, u64),
    target: usize,
    was_closed: bool,
    began: f64,
    /// Frames drawn since it was asked for (the one-thread way waits for one).
    frames: u32,
    /// Where a worker thread leaves what it made; `None` when it runs here.
    slot: Option<Arc<Mutex<Option<Outcome>>>>,
}

/// Whether the page has worker threads to run a long job on.
fn threads() -> bool {
    cfg!(feature = "threads") && rayon::current_num_threads() > 1
}

fn fmt_count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.0}k", n as f32 / 1e3),
        _ => format!("{:.2}M", n as f32 / 1e6),
    }
}

impl GolemApp {
    /// Whether a Quad Remesh is under way.
    pub(crate) fn quad_busy(&self) -> bool {
        self.shaping.quad.job.is_some()
    }

    fn quad_remesh_start(&mut self, now: f64) {
        if self.quad_busy() {
            return;
        }
        self.deform_commit();
        let target = self.shaping.quad.target as usize;
        let was_closed = crate::printable::census(&self.mesh).is_closed();
        let mut job = Job {
            object: self.active,
            mesh: (self.mesh.uid, self.mesh.edits()),
            target,
            was_closed,
            began: now,
            frames: 0,
            slot: None,
        };
        if threads() {
            let slot = Arc::new(Mutex::new(None));
            let out = slot.clone();
            // The worker gets the surface only, as a mesh of its own.
            let copy = Mesh::new(self.mesh.positions.clone(), self.mesh.indices.clone(), None);
            rayon::spawn(move || {
                let made = quadremesh::quad_remesh_mesh(&copy, target);
                if let Ok(mut out) = out.lock() {
                    *out = Some(made);
                }
            });
            job.slot = Some(slot);
            self.status = format!("Quad remeshing to about {} quads...", fmt_count(target));
        } else {
            self.status = format!(
                "Quad remeshing to about {} quads... The page will be still until it is done.",
                fmt_count(target)
            );
        }
        self.shaping.quad.job = Some(job);
    }

    /// Runs every frame: starts the one-thread remesh once its line has been
    /// drawn, counts the seconds of a threaded one, and takes in the result.
    pub(crate) fn quad_frame(&mut self, ctx: &egui::Context, now: f64) {
        let Some(job) = self.shaping.quad.job.as_mut() else { return };
        job.frames += 1;
        let outcome = match &job.slot {
            Some(slot) => match slot.lock().ok().and_then(|mut s| s.take()) {
                Some(outcome) => outcome,
                None => {
                    self.status = format!(
                        "Quad remeshing to about {} quads... {:.0} s",
                        fmt_count(job.target),
                        (now - job.began).max(0.0)
                    );
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                    return;
                }
            },
            None if job.frames < 3 => {
                ctx.request_repaint();
                return;
            }
            None => quadremesh::quad_remesh_mesh(&self.mesh, job.target),
        };
        let Some(job) = self.shaping.quad.job.take() else { return };
        if job.object != self.active || job.mesh != (self.mesh.uid, self.mesh.edits()) {
            self.status = "The model changed while Quad Remesh was working, so its result was not used. Press Quad Remesh again.".into();
            return;
        }
        match outcome {
            Ok((mesh, report)) => {
                self.replace_mesh(mesh, "Quad remesh");
                let note = std::mem::take(&mut self.status);
                // The quads are the point: show them.
                self.set_wire(true);
                let mut text = format!("Quad remesh: {}.{}", report.summary(), report.advice(job.was_closed));
                // What the replacement said about levels or layers lost.
                if let Some(i) = note.find(" Its ") {
                    text += &note[i..];
                }
                self.status = text;
            }
            Err(e) => self.status = e,
        }
    }

    /// The Quads slider and the button, inside the Remesh card.
    pub(crate) fn quad_rows(&mut self, ui: &mut Ui, now: f64) {
        let tips = self.ui.tips;
        ui.add_space(6.0);
        let range = quadremesh::TARGET_RANGE;
        let state = &mut self.shaping.quad;
        let mut log = state.target.log2();
        if theme::slider(ui, "Quads", &mut log, range.start().log2()..=range.end().log2(), |v| fmt_count(v.exp2() as usize)).changed() {
            // Two significant figures: 5,200 rather than 5,187.
            let step = 10f32.powf(log.exp2().log10().floor() - 1.0);
            state.target = ((log.exp2() / step).round() * step).clamp(*range.start(), *range.end());
        }
        let busy = state.job.is_some();
        let mut pressed = false;
        ui.add_enabled_ui(!busy, |ui| {
            let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
            let label = if busy { "Quad Remeshing..." } else { "Quad Remesh" };
            pressed = tip(
                theme::button(ui, ui.available_width(), label, style),
                tips,
                "Rebuilds the surface as about this many even quads that follow the form. Export keeps the quads.",
            )
            .clicked();
        });
        if self.sculptor.settings.symmetry.iter().any(|&on| on) {
            theme::hint(ui, "Quad Remesh ignores symmetry: the two sides may not match triangle for triangle afterwards.");
        }
        if pressed {
            self.quad_remesh_start(now);
        }
    }
}
