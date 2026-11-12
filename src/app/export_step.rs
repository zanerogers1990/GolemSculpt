//! Export STEP: the selected object as solid bodies for CAD (Fusion 360,
//! SolidWorks, Onshape, FreeCAD, SolidCraft), written by `io/step.rs`.
//!
//! Each triangle becomes a flat face of the solid, and CAD programs slow
//! to a crawl long before a sculpt's millions of triangles, so the file is
//! made from a copy of the model decimated to the Export menu's STEP Faces
//! (`Options::step_faces`). The decimation is the Decimate button's, a
//! mirror kept as it keeps one; the model itself is not touched. A model
//! with no more triangles than that goes out as it is.
//!
//! It runs in the background, on a job of its own: decimating a large
//! model takes some seconds, and the file is tens of megabytes. Its
//! progress and a Cancel show at the bottom of the view, and sculpting
//! carries on; the file is of the model as it was when the box was
//! answered. The Claude bridge and the tests call `export_step_to`, which
//! does the same work without the job and without the save box.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::Mat4;
use sculpt_toolkit::Job;

use super::Running;
use crate::decimate::{self, Source};
use crate::io::{self, StepWritten};
use crate::mesh::Mesh;
use crate::printable::grouped;

/// Fewest and most faces STEP Faces offers. Under a thousand a sculpt is
/// lumps; past half a million no CAD program handles it.
pub const STEP_FACES: RangeInclusive<f32> = 1_000.0..=500_000.0;

/// How often a frame is drawn while an export works, to say when it is done.
const POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// An export at work: the status line it ends with, or why it failed.
pub(super) struct Pending {
    job: Job<String>,
}

/// What a STEP export reads, copied out of the scene so that the work can be
/// done on a thread of its own.
struct Work {
    model: Model,
    name: String,
    places: Vec<(String, Mat4)>,
    /// What the status line says of the object's copies.
    note: String,
    target: usize,
    path: PathBuf,
}

/// The model to write: already few enough triangles, or to be decimated first.
enum Model {
    AsItIs(Mesh),
    Decimate(Box<Source>),
}

/// The count to bring the model down to: what the menu says, held to its range.
fn faces_of(setting: f32) -> usize {
    let setting = if setting.is_finite() { setting } else { *STEP_FACES.start() };
    setting.clamp(*STEP_FACES.start(), *STEP_FACES.end()).round() as usize
}

impl Running {
    /// The STEP line of the Export menu: asks where to put the file, then
    /// writes it in the background.
    pub(super) fn export_step_action(&mut self) {
        if self.export_step.is_some() {
            return self.status("Export STEP is still working on the last one. Wait for it, or press its Cancel.", false);
        }
        let file = io::export_file_name(&self.doc.scene.active().name, "step");
        let pick = rfd::FileDialog::new().add_filter("STEP for CAD", &["step", "stp"]).set_file_name(file).save_file();
        let Some(path) = pick else { return };
        let work = match self.export_step_work(path) {
            Ok(work) => work,
            Err(why) => return self.status(why, false),
        };
        let job = self.doc.tools.jobs.spawn("Export STEP", move |cx| {
            cx.progress("0%");
            let mut progress = |done: f32| {
                cx.progress(format!("{:.0}%", done * 100.0));
                !cx.cancelled()
            };
            work.run(&mut progress)
        });
        self.export_step = Some(Pending { job });
        self.redraw = true;
    }

    /// Writes the selected object as a STEP file at `path` there and then,
    /// as the menu's line does in the background: for the Claude bridge and
    /// the tests, which have no save box to answer. Returns the status line.
    pub(super) fn export_step_to(&mut self, path: &Path) -> Result<String, String> {
        let text = self.export_step_work(path.to_owned())?.run(&mut |_| true)?;
        self.status(text.clone(), true);
        Ok(text)
    }

    /// Once a frame: says how an export that has finished went.
    pub(super) fn export_step_frame(&mut self) {
        let Some(pending) = &mut self.export_step else { return };
        let Some(result) = pending.job.poll() else {
            let at = Instant::now() + POLL;
            self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            return;
        };
        self.export_step = None;
        self.redraw = true;
        match result {
            Ok(text) => {
                self.toast("Export STEP");
                self.status(text, true);
            }
            // A cancel is the user's own doing, and no bad news.
            Err(why) => self.status(&why, why.contains("cancelled")),
        }
    }

    /// The selected object, its copies and the count, copied out for an export to `path`.
    fn export_step_work(&self, path: PathBuf) -> Result<Work, String> {
        let doc = &self.doc;
        let triangles = doc.mesh.live_triangle_count();
        if triangles == 0 {
            return Err("There is nothing to export: this object has no surface.".into());
        }
        let target = faces_of(doc.opts.step_faces);
        let model = if triangles > target {
            // The Decimate button's own reading of the model: tidied, and its mirror kept.
            Model::Decimate(Box::new(self.decimate_source()))
        } else {
            let tidy = crate::scene::tidied(&doc.mesh);
            // A copy of its surface only: that is all the file holds.
            Model::AsItIs(tidy.unwrap_or_else(|| Mesh::new(doc.mesh.positions.clone(), doc.mesh.indices.clone(), None)))
        };
        Ok(Work {
            model,
            name: doc.scene.active().name.clone(),
            places: self.export_places(),
            note: self.export_note(),
            target,
            path,
        })
    }
}

impl Work {
    /// Decimates if it is to, writes the file, and says how it went.
    fn run(self, progress: &mut dyn FnMut(f32) -> bool) -> Result<String, String> {
        let Work { model, name, places, note, target, path } = self;
        let cancelled = || "Export STEP was cancelled. No file was written.".to_owned();
        let (mesh, before) = match model {
            Model::AsItIs(mesh) => (mesh, None),
            Model::Decimate(source) => {
                let before = source.triangle_count();
                // Decimating is most of the work; writing the file is the rest.
                let mut share = |done: f32| progress(done * 0.8);
                match decimate::decimated(&source, target, &mut share)? {
                    Some((mesh, _)) => (mesh, Some(before)),
                    None => return Err(cancelled()),
                }
            }
        };
        if !progress(0.8) {
            return Err(cancelled());
        }
        let named: Vec<(&str, Mat4)> = places.iter().map(|(n, m)| (n.as_str(), *m)).collect();
        let written = io::export_step_placed(&mesh, &name, &named, &path)?;
        progress(1.0);
        Ok(line(&name, &note, &path, written, before))
    }
}

/// The status line after a STEP export.
fn line(name: &str, note: &str, path: &Path, written: StepWritten, before: Option<usize>) -> String {
    let StepWritten { solids, surfaces, faces, mended } = written;
    let decimated = before.map_or_else(String::new, |before| format!(", decimated from {}", grouped(before)));
    let bodies = match (solids, surfaces) {
        (1, 0) => "a solid".to_owned(),
        (s, 0) => format!("{s} solids"),
        (0, 1) => "a surface".to_owned(),
        (0, s) => format!("{s} surfaces"),
        (s, f) => format!("{s} solid{}, {f} surface{}", if s == 1 { "" } else { "s" }, if f == 1 { "" } else { "s" }),
    };
    let mut text = format!(
        "Exported \"{name}\"{note} to {} as {bodies} for CAD, {} flat faces{decimated} (millimetres, Z up).",
        path.display(),
        grouped(faces)
    );
    if mended > 0 {
        text += &format!(" {} damaged points were written mended.", grouped(mended));
    }
    if surfaces > 0 {
        text += " A surface is a piece that is not closed (it has holes, or sheets meeting along an edge): CAD shows it \
                 but cannot cut or join with it. Remesh closes a model; Check for Printing shows what is open.";
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_is_held_to_the_range() {
        assert_eq!(faces_of(25_000.4), 25_000);
        assert_eq!(faces_of(10.0), 1_000);
        assert_eq!(faces_of(f32::NAN), 1_000);
        assert_eq!(faces_of(9e9), 500_000);
    }

    #[test]
    fn the_line_says_what_went_out() {
        let path = Path::new("lure.step");
        let one = StepWritten { solids: 1, surfaces: 0, faces: 25_000, mended: 0 };
        let text = line("Lure", "", path, one, Some(2_000_000));
        assert!(text.contains("as a solid for CAD, 25,000 flat faces, decimated from 2,000,000"), "{text}");
        assert!(!text.contains("surface"));
        let mixed = StepWritten { solids: 2, surfaces: 1, faces: 900, mended: 0 };
        let text = line("Lure", "", path, mixed, None);
        assert!(text.contains("as 2 solids, 1 surface for CAD, 900 flat faces (millimetres"), "{text}");
        assert!(text.contains("cannot cut or join"));
    }

    /// A real sculpt through the whole of it, for a CAD program to open:
    /// `STEP_FROM=scene.sclp STEP_TO=out.step cargo test -- --ignored step_of_a_sculpt`.
    #[test]
    #[ignore]
    fn step_of_a_sculpt() {
        let (Ok(from), Ok(to)) = (std::env::var("STEP_FROM"), std::env::var("STEP_TO")) else { return };
        let faces = std::env::var("STEP_FACES").ok().and_then(|f| f.parse().ok()).unwrap_or(25_000);
        let scene = io::load_scene(Path::new(&from)).unwrap();
        let object = &scene.objects[scene.active];
        let mesh = &scene.meshes[object.mesh];
        let model = Mat4::from_scale_rotation_translation(object.scale, object.rotation, object.position);
        let started = Instant::now();
        let work = Work {
            model: Model::Decimate(Box::new(Source::of(mesh))),
            name: object.name.clone(),
            places: vec![(String::new(), model)],
            note: String::new(),
            target: faces,
            path: PathBuf::from(&to),
        };
        let text = work.run(&mut |_| true).unwrap();
        println!("{text} in {:.1} s", started.elapsed().as_secs_f64());
    }
}
