//! The app's side of Render Picture (`crate::still`): taking the camera as
//! it is, drawing the picture a few samples a frame into a target of its
//! own, and writing the PNG on a worker thread.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use winit::event_loop::ActiveEventLoop;

use super::Running;
use crate::capture::{self, Target};
use crate::still::{Progress, Request};
use crate::timelapse::rig::Shot;

/// How long a frame may spend drawing samples, so the window keeps
/// answering while a picture renders.
const FRAME_BUDGET: Duration = Duration::from_millis(40);
/// Frames a scripted render lets pass first, so it is of a settled view.
const SCRIPT_LEAD_IN: u32 = 12;

pub(super) struct StillRun {
    target: Target,
    /// Where the camera was when Render was pressed, and its lens.
    shot: Shot,
    fov_y: f32,
    samples: u32,
    transparent: bool,
    path: PathBuf,
    /// Samples drawn so far, counting any drawn again after a change.
    drawn: u32,
    /// Samples the picture holds.
    shown: u32,
    /// Once every sample is in: the worker writing the PNG puts its result here.
    saved: Option<Arc<Mutex<Option<Result<(), String>>>>>,
    cancelled: bool,
}

/// Where rendered pictures go: the Sculpt folder in the user's Pictures.
pub fn pictures_folder() -> PathBuf {
    match std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        Some(home) => PathBuf::from(home).join("Pictures").join("Sculpt"),
        None => PathBuf::from("Renders"),
    }
}

impl Running {
    /// What the card's buttons asked for during the last interface pass.
    pub(super) fn still_requests(&mut self) {
        let size = self.window.inner_size();
        let still = &mut self.doc.still;
        (still.window, still.limit) = ((size.width, size.height), self.gpu.max_texture_side() as u32);
        match still.request.take() {
            Some(Request::Render) => self.start_still(None),
            Some(Request::Cancel) => {
                if let Some(run) = &mut self.still {
                    run.cancelled = true;
                }
            }
            Some(Request::OpenFolder) => {
                if let Err(e) = capture::open_folder(&pictures_folder()) {
                    self.status(e, false);
                }
            }
            None => {}
        }
        // A scripted run (--render-picture): the settings it was given, and
        // Render once the view has settled.
        if let Some(path) = self.args.render_picture.clone() {
            if self.frames == SCRIPT_LEAD_IN && self.still.is_none() {
                let s = &mut self.doc.still.settings;
                s.scale = self.args.render_scale.unwrap_or(1).clamp(1, 4);
                s.samples = self.args.render_samples.unwrap_or(64).max(1);
                s.transparent = self.args.render_transparent;
                self.start_still(Some(path));
            }
        }
    }

    /// Starts rendering the view as it is, to `path` if given (a scripted
    /// run's) or to a new name in the Pictures folder.
    pub(super) fn start_still(&mut self, path: Option<PathBuf>) {
        if self.still.is_some() {
            return;
        }
        let still = &mut self.doc.still;
        let size = still.size();
        let settings = still.settings;
        let path = path.unwrap_or_else(|| {
            capture::unused(pictures_folder().join(crate::io::export_file_name("Render", "png")))
        });
        if let Some(folder) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(folder) {
                still.status = format!("Could not make {}: {e}", folder.display());
                still.status_ok = false;
                return;
            }
        }
        // The window's camera where it stands; the picture has the
        // window's shape, so the same lens frames the same view.
        let (shot, fov_y) = (self.artist_shot(), self.doc.camera.fov_y);
        let still = &mut self.doc.still;
        still.progress = Some(Progress { samples: 0, total: settings.samples, saving: false });
        still.status.clear();
        self.still = Some(StillRun {
            target: Target::new(&self.gpu, size),
            shot,
            fov_y,
            samples: settings.samples.max(1),
            transparent: settings.transparent,
            path,
            drawn: 0,
            shown: 0,
            saved: None,
            cancelled: false,
        });
        self.redraw = true;
    }

    /// End of a frame, once the window's picture has gone out: draws the
    /// next few samples, or writes the picture once they are all in.
    pub(super) fn still_capture(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut run) = self.still.take() else {
            // A scripted render that could not start (its folder could not
            // be made) ends the run, saying why.
            let still = &self.doc.still;
            if self.args.render_picture.is_some() && self.frames > SCRIPT_LEAD_IN && !still.status_ok {
                eprintln!("render failed: {}", still.status);
                event_loop.exit();
            }
            return;
        };
        self.redraw = true;
        if run.cancelled && run.saved.is_none() {
            let still = &mut self.doc.still;
            (still.progress, still.status, still.status_ok) = (None, "Render cancelled.".into(), false);
            return;
        }
        if let Some(saved) = &run.saved {
            let outcome = saved.lock().ok().and_then(|mut s| s.take());
            let Some(outcome) = outcome else {
                self.still = Some(run);
                return;
            };
            let still = &mut self.doc.still;
            still.progress = None;
            match outcome {
                Ok(()) => {
                    let name = run.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let place = run.path.parent().map(|p| capture::tail(p, 2)).unwrap_or_default();
                    let (w, h) = run.target.size;
                    still.status = format!("Saved {name} ({w} x {h}, {} samples) in {place}", run.shown);
                    still.status_ok = true;
                    if self.args.render_picture.is_some() {
                        println!("render: {w}x{h}, {} samples, in {}", run.shown, run.path.display());
                        event_loop.exit();
                    }
                }
                Err(e) => {
                    still.status = e;
                    still.status_ok = false;
                    if self.args.render_picture.is_some() {
                        eprintln!("render failed: {}", still.status);
                        event_loop.exit();
                    }
                }
            }
            return;
        }

        // A few more samples, as many as fit in this frame's budget. What
        // the picture shows changing meanwhile (a stroke) starts its samples
        // again: it is done when it holds them all, or, should they keep
        // starting again, after four times as many draws.
        let camera = self.camera_at(run.shot, run.fov_y);
        let (done, drawn, shown) =
            self.draw_capture(&mut run.target, camera, run.samples, run.transparent, run.samples, FRAME_BUDGET);
        run.drawn += drawn;
        run.shown = shown.min(run.samples);
        let finished = done || run.drawn >= run.samples.saturating_mul(4);
        self.doc.still.progress = Some(Progress { samples: run.shown, total: run.samples, saving: finished });
        if finished {
            let picture = crate::render::read_texture_as(&self.gpu, &run.target.texture, run.transparent);
            let slot = Arc::new(Mutex::new(None));
            let (into, path, transparent) = (slot.clone(), run.path.clone(), run.transparent);
            rayon::spawn(move || {
                // Opaque pictures without the unused channel: a quarter less to write.
                let written = if transparent {
                    picture.save(&path)
                } else {
                    image::DynamicImage::ImageRgba8(picture).into_rgb8().save(&path)
                };
                let outcome = written.map_err(|e| format!("Could not write {}: {e}", path.display()));
                if let Ok(mut slot) = into.lock() {
                    *slot = Some(outcome);
                }
            });
            run.saved = Some(slot);
        }
        self.still = Some(run);
    }
}
