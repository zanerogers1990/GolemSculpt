//! The app's side of the turntable, the 360 loop and the timelapse: stepping
//! the spin, keeping the clean view clean, and drawing the viewport a second
//! time, at the recording's own size, for the pictures that are saved.
//!
//! The rules are in `crate::turntable`, `crate::capture` and
//! `crate::timelapse`; this file is where they meet the window, the camera
//! and the renderer.
//!
//! A recorded picture is never a grab of the window. It is drawn again into
//! a target of its own (`capture::Target`), from this frame's list of
//! objects, after the window's frame has gone out: so its size and shape are
//! the file's and not the window's, and the panels, the brush ring and the
//! guides cannot be in it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};
use winit::event::MouseButton;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;

use super::{Drag, Running};
use crate::camera::{Camera, View};
use crate::capture::{self, LoopWriter, Target};
use crate::render::DrawItem;
use crate::render::spheres::SkinLook;
use crate::sculpt::{Brush, PointerSample};
use crate::ssphere::view::Armature;
use crate::timelapse::recorder::{Outcome, Recorder};
use crate::timelapse::rig::{Rig, Shot, Subject};
use crate::timelapse::{self, Phase, Schedule, activity};
use crate::turntable::{self, POINTER_REST_SECONDS, Progress};

/// How long a loop that was cancelled waits for its picture writers before
/// the app closes regardless.
const CLOSE_WAIT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct State {
    looping: Option<LoopRun>,
    timelapse: Option<TimelapseRun>,
    /// What the window drew this frame; a recorded picture draws the same.
    pub(super) items: Vec<DrawItem>,
    /// This frame's length, for the timelapse's clock.
    dt: f32,
    /// Clean view: where the pointer was last frame and when it last moved.
    pointer: Vec2,
    pointer_moved: Option<Instant>,
    /// A recording run from the command line, which closes the app after.
    script: Option<Script>,
    /// The script wants the app closed now, whatever is still recording.
    close: bool,
    /// Where this run's recordings go instead of the user's Videos folder
    /// (the self-check keeps its own in the folder it was given).
    folder: Option<PathBuf>,
}

/// A view to put the camera back to.
#[derive(Clone, Copy)]
struct Pose {
    yaw: f32,
    pitch: f32,
    distance: f32,
    pivot: Vec3,
}

struct LoopRun {
    writer: Option<LoopWriter>,
    target: Target,
    /// Frame 0's view. Pitch, distance and pivot are put back every frame,
    /// so a stray drag cannot break the loop.
    start: Pose,
    direction: f32,
    spinning_before: bool,
    clean_before: bool,
    /// The camera is placed for the next picture, to be taken once the
    /// window's frame is out.
    due: bool,
    /// The recording is over without a loop to show for it; this is why.
    /// Held until the picture writers have stopped, then they are removed.
    ending: Option<String>,
}

impl Drop for LoopRun {
    /// Reached with pictures still being written only when the app closes
    /// mid-recording: part of a loop is not a loop, so it is removed.
    fn drop(&mut self) {
        let Some(writer) = self.writer.take() else { return };
        writer.cancel();
        let start = Instant::now();
        while !writer.settled() && start.elapsed() < CLOSE_WAIT {
            std::thread::sleep(Duration::from_millis(5));
        }
        writer.discard();
    }
}

struct TimelapseRun {
    recorder: Recorder,
    rig: Rig,
    schedule: Schedule,
    target: Target,
}

/// `--record-loop` and `--demo-timelapse`: recordings made without a person
/// at the tablet, to check the pictures and the video by.
enum Script {
    Loop { folder: PathBuf, size: (u32, u32), frames: u32, started: bool },
    /// `frame` is the next step of the scripted session; `since` is when it
    /// began. The steps are paced by the clock (sixty a second), not by the
    /// app's frames, because the timelapse itself runs on real time.
    Timelapse { path: PathBuf, frame: u32, since: Option<Instant> },
    Check(Check),
}

/// `--record-loop <folder>` with `SCULPT_DEMO_CAMERA=check`: the app presses
/// its own keys and buttons and prints what happened, one line per thing
/// checked. It goes through the same handlers real input does, so it shows
/// that the turntable's hooks in the app are still connected.
struct Check {
    stage: u32,
    /// Frames drawn, and time passed, in this stage.
    frames: u32,
    since: Instant,
    /// The camera's yaw at a moment worth comparing with later.
    yaw: f32,
    /// Where the check last put the pointer.
    pointer: Vec2,
    failed: u32,
}

/// How many pictures each loop folder under `folder` holds.
fn loops_in(folder: &std::path::Path) -> Vec<usize> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut counts: Vec<usize> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("Turntable_"))
        .filter_map(|e| Some(std::fs::read_dir(e.path()).ok()?.flatten().count()))
        .collect();
    counts.sort();
    counts
}

/// The scripted timelapse's strokes: brush, radius, from and to (pixels from
/// the middle of a 900 px high view), and how long to rest afterwards, in
/// frames. The long rests are longer than the idle grace, so the recording
/// has pauses to leave out.
const SCRIPT_STROKES: [(Brush, f32, Vec2, Vec2, u32); 4] = [
    (Brush::Clay, 34.0, Vec2::new(-130.0, -110.0), Vec2::new(60.0, -70.0), 20),
    (Brush::Standard, 30.0, Vec2::new(-120.0, 20.0), Vec2::new(70.0, 50.0), 110),
    (Brush::Crease, 26.0, Vec2::new(-90.0, 110.0), Vec2::new(90.0, 130.0), 20),
    (Brush::Inflate, 40.0, Vec2::new(-20.0, -150.0), Vec2::new(40.0, 40.0), 80),
];
/// Frames a scripted stroke takes, and the pointer samples fed in each.
const SCRIPT_STROKE_FRAMES: u32 = 30;
const SCRIPT_SAMPLES_PER_FRAME: u32 = 2;
/// Frames the scripts let pass first, so the first pictures are of a settled view.
const SCRIPT_LEAD_IN: u32 = 12;

impl Running {
    /// Once, when the app has started: what the command line asked for.
    pub(super) fn recording_startup(&mut self) {
        if self.args.clean_view {
            turntable::set_clean_view(&mut self.doc, true);
        }
        // Looks and runs for checks made without a person at the tablet.
        let demo = std::env::var("SCULPT_DEMO_CAMERA").unwrap_or_default();
        match demo.as_str() {
            "turntable" => self.doc.turntable.pinned = true,
            "timelapse" => self.doc.timelapse.pinned = true,
            // What a loop looks like part of the way through recording.
            "recording" => {
                self.doc.opts.clean_view = true;
                self.doc.turntable.recording = Some(Progress { written: 37, total: 360 });
            }
            _ => {}
        }
        if let Some(folder) = self.args.record_loop.clone() {
            if demo == "check" {
                self.recording.folder = Some(folder);
                let check = Check { stage: 0, frames: 0, since: Instant::now(), yaw: 0.0, pointer: Vec2::ZERO, failed: 0 };
                self.recording.script = Some(Script::Check(check));
                return;
            }
            let size = self.args.loop_size.unwrap_or((640, 360));
            let frames = self.args.loop_frames.unwrap_or(12).max(2);
            self.recording.script = Some(Script::Loop { folder, size, frames, started: false });
        } else if let Some(path) = self.args.demo_timelapse.clone() {
            self.recording.script = Some(Script::Timelapse { path, frame: 0, since: None });
        }
    }

    /// A key went down. True when it was the turntable's, or when the clean
    /// view swallows it.
    pub(super) fn recording_key(&mut self, code: KeyCode) -> bool {
        let mods = self.input.mods;
        let bare = !(mods.control_key() || mods.alt_key() || mods.shift_key());
        let clean = self.doc.opts.clean_view;
        let looping = self.recording.looping.is_some();
        match code {
            // Tab for the panels. Not with a modifier: Alt + Tab is the system's.
            KeyCode::Tab => {
                if bare {
                    turntable::set_clean_view(&mut self.doc, !clean);
                }
                return true;
            }
            // O for "orbit": T is the Tube tool's.
            KeyCode::KeyO if bare => {
                if !looping {
                    let spin = &mut self.doc.turntable.spin;
                    spin.on = !spin.on;
                    let on = spin.on;
                    // The clean view is a presentation; the turning says it.
                    if !clean {
                        self.toast(if on { "Spin" } else { "Spin Off" });
                    }
                }
                return true;
            }
            KeyCode::Escape if looping => {
                self.cancel_loop("Recording cancelled.");
                return true;
            }
            KeyCode::Escape if clean => {
                turntable::set_clean_view(&mut self.doc, false);
                return true;
            }
            _ => {}
        }
        // None of the bare-key shortcuts work in the clean view: an R there
        // would remesh, a Z undo, a Delete ask about an object no panel is
        // showing. Fullscreen and quit still do.
        clean && code != KeyCode::F11 && !(code == KeyCode::KeyQ && mods.control_key())
    }

    /// Whether the pointer itself should be hidden: in the clean view, once
    /// it has rested. A presentation with an arrow parked on the model is
    /// not one. While a loop records it stays, for the Cancel button.
    pub(super) fn recording_hides_pointer(&self) -> bool {
        self.doc.opts.clean_view
            && self.recording.looping.is_none()
            && self.recording.pointer_moved.is_some_and(|t| t.elapsed().as_secs_f32() >= POINTER_REST_SECONDS)
    }

    /// Start of a frame, before the view is worked out and before the brush
    /// runs: turns the camera, and notes whether the model is being changed.
    pub(super) fn recording_step(&mut self, dt: f32) {
        self.recording.dt = dt;
        self.recording_requests();
        self.script_step();

        if let Some(run) = &mut self.recording.looping {
            let total = run.writer.as_ref().map_or(1, |w| w.total());
            let issued = run.writer.as_ref().map_or(0, |w| w.issued());
            run.due = run.ending.is_none() && run.writer.as_ref().is_some_and(|w| w.ready() && w.error().is_none());
            // While the picture writers catch up, the view holds the last
            // picture taken.
            let frame = if run.due { issued } else { issued.saturating_sub(1) };
            let camera = &mut self.doc.camera;
            camera.cancel_glide();
            camera.yaw = turntable::loop_yaw(run.start.yaw, run.direction, frame.clamp(0, total - 1), total);
            (camera.pitch, camera.distance, camera.pivot) = (run.start.pitch, run.start.distance, run.start.pivot);
            self.redraw = true;
        } else {
            // An orbit by hand or an axis snap has the yaw while it runs.
            let yielding = self.input.drag == Drag::Orbit || self.doc.camera.is_snapping();
            let table = &mut self.doc.turntable;
            let turn = table.spin.step(&table.settings, dt, yielding);
            if turn != 0.0 {
                self.doc.camera.add_yaw(turn);
            }
            if table.spin.moving() {
                self.redraw = true;
            }
        }

        // The timelapse's signal, from the two changes that last longer than
        // a frame: a stroke laying down dabs, and a drag on the gizmo. (The
        // rest report when they go onto the undo history.)
        let doc = &self.doc;
        if doc.sculptor.is_active() && (!self.input.samples.is_empty() || doc.sculptor.settings.build_up_on_hold) {
            match doc.sculptor.last_hit {
                Some(point) => activity::edited_at(doc.scene.stroke_object().id, point),
                None => activity::edited(),
            }
        } else if self.input.drag == Drag::Gizmo {
            activity::edited();
        }

        // The clean view's resting pointer.
        if self.doc.opts.clean_view {
            let now = Instant::now();
            let moved = self.input.pointer != self.recording.pointer || self.input.button.is_some();
            self.recording.pointer = self.input.pointer;
            if moved || self.recording.pointer_moved.is_none() {
                self.recording.pointer_moved = Some(now);
            }
            // Nothing else may ask for a frame when the rest is over.
            let hide_at = self.recording.pointer_moved.unwrap_or(now) + Duration::from_secs_f32(POINTER_REST_SECONDS);
            if hide_at > now {
                self.repaint_at = Some(self.repaint_at.map_or(hide_at, |t| t.min(hide_at)));
            }
        } else {
            self.recording.pointer_moved = None;
        }
    }

    /// Just before the window's frame is drawn: in the clean view the
    /// pulses, the tools' previews and the SSphere armature are left out.
    pub(super) fn recording_before_draw(&mut self) {
        let clean = self.doc.opts.clean_view;
        self.renderer.overlays = !clean;
        if clean {
            let view = self.view;
            self.show_skin_only(&view);
        }
    }

    /// The frame's shading inputs as the clean view wants them: without the
    /// darkening a mask puts on the surface. A mask is a working aid, like
    /// the guides; it is not part of the model being shown.
    pub(super) fn recording_look(&self, mut look: crate::look::LookUniform) -> crate::look::LookUniform {
        if self.doc.opts.clean_view {
            look.mask_tint[3] = 0.0;
        }
        look
    }

    /// End of a frame, once the window's picture has gone out: takes the
    /// pictures that are due.
    pub(super) fn recording_capture(&mut self, event_loop: &ActiveEventLoop) {
        self.loop_capture();
        self.timelapse_capture();
        self.script_end(event_loop);
    }

    /// What the cards' buttons asked for during the last interface pass.
    fn recording_requests(&mut self) {
        if let Some(request) = self.doc.turntable.request.take() {
            match request {
                turntable::Request::RecordLoop => self.start_loop(None),
                turntable::Request::CancelLoop => self.cancel_loop("Recording cancelled."),
                turntable::Request::OpenFolder => self.open_recordings(None),
            }
        }
        if let Some(request) = self.doc.timelapse.request.take() {
            match request {
                timelapse::Request::Start => self.start_timelapse(None),
                timelapse::Request::Stop => self.stop_timelapse(),
                // The last video's own folder, when there is one.
                timelapse::Request::OpenFolder => self.open_recordings(self.doc.timelapse.last_output.clone()),
            }
        }
    }

    /// Where recordings go.
    fn recordings_folder(&self) -> PathBuf {
        self.recording.folder.clone().unwrap_or_else(capture::recordings_folder)
    }

    fn open_recordings(&mut self, last: Option<PathBuf>) {
        let folder = last
            .filter(|file| file.exists())
            .and_then(|file| file.parent().map(PathBuf::from))
            .unwrap_or_else(|| self.recordings_folder());
        if let Err(e) = capture::open_folder(&folder) {
            self.status(e, false);
        }
    }

    // ---- drawing a picture at a size of its own ----------------------------

    /// A camera at `shot`, with the working camera's lens unless `fov_y`
    /// gives another.
    fn camera_at(&self, shot: Shot, fov_y: f32) -> Camera {
        let mut camera = Camera::default();
        (camera.yaw, camera.pitch, camera.distance, camera.pivot) = (shot.yaw, shot.pitch, shot.distance, shot.pivot);
        camera.ortho = shot.ortho;
        camera.fov_y = fov_y;
        camera
    }

    /// Where the working camera is.
    fn artist_shot(&self) -> Shot {
        let c = &self.doc.camera;
        Shot { yaw: c.yaw, pitch: c.pitch, distance: c.distance, pivot: c.pivot, ortho: c.ortho }
    }

    /// `camera`'s view of a picture of `size`, with the near plane set from
    /// the nearest surface it sees, as the window's own view is.
    fn capture_view(&self, camera: &Camera, size: (u32, u32)) -> View {
        let size = Vec2::new(size.0 as f32, size.1 as f32);
        let probe = camera.view(Vec2::ZERO, size, camera.distance, self.scene_size);
        let mut depth = camera.distance;
        for uv in [(0.5, 0.5), (0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
            let (origin, dir) = probe.ray(Vec2::new(uv.0, uv.1) * size);
            if let Some(hit) = self.doc.scene.pick(origin, dir, &self.doc.mesh) {
                let forward = probe.depth_of(hit.pos);
                if forward > 0.0 {
                    depth = depth.min(forward);
                }
            }
        }
        camera.view(Vec2::ZERO, size, depth.max(1e-5), self.scene_size)
    }

    /// With the SSphere tool up, shows its skin without the spheres, links,
    /// ghost and mirror plane: the skin is the model as far as anyone looking
    /// at it is concerned, the rest is scaffolding. The tool puts its own
    /// drawing back on its next frame.
    fn show_skin_only(&mut self, view: &View) {
        if !self.spheres_active() {
            return;
        }
        let s = &self.doc.spheres;
        let look = if s.preview { SkinLook::Solid } else { SkinLook::Shell };
        let model = s.frame().matrix();
        self.renderer.spheres.show(&self.gpu, view, &Armature::default(), s.skin(), look, s.shell_lift(), model);
    }

    /// Draws what the window just drew into `target`, as `camera` sees it,
    /// with nothing over the model.
    fn draw_capture(&mut self, target: &mut Target, camera: Camera) {
        let view = self.capture_view(&camera, target.size);
        // The frame's inputs are worked out from the app's own view and
        // camera, so those are lent this shot for the moment.
        let window_view = std::mem::replace(&mut self.view, view);
        let window_camera = std::mem::replace(&mut self.doc.camera, camera);
        let mut globals = self.globals();
        let look = self.recording_look(self.look_uniform());
        self.doc.camera = window_camera;
        self.view = window_view;
        // No symmetry guides.
        globals.symmetry[..3].fill(0.0);

        self.show_skin_only(&view);
        self.renderer.overlays = false;
        // This goes out as its own batch of work: the renderer's settings
        // are shared with the window's frame, and each batch reads them as
        // they were last written.
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.swap_side(&mut target.side);
        self.renderer.draw(
            &self.gpu,
            &mut encoder,
            &target.view,
            target.size,
            &globals,
            &look,
            &self.doc.opts,
            &self.recording.items,
        );
        self.renderer.swap_side(&mut target.side);
        self.gpu.queue.submit([encoder.finish()]);
        self.renderer.overlays = !self.doc.opts.clean_view;
    }

    /// Whether the graphics card can hold a picture this large.
    fn fits(&self, size: (u32, u32)) -> bool {
        let limit = self.gpu.max_texture_side() as u32;
        size.0 <= limit && size.1 <= limit
    }

    // ---- 360 loop ----------------------------------------------------------

    /// Records one turn from the view as it is. `script` gives the folder,
    /// size and number of pictures for a run from the command line.
    fn start_loop(&mut self, script: Option<(PathBuf, (u32, u32), u32)>) {
        if self.recording.looping.is_some() || self.doc.sculptor.is_active() {
            return;
        }
        let settings = self.doc.turntable.settings;
        let (folder, size, total) = script.unwrap_or_else(|| {
            let folder = capture::unused(self.recordings_folder().join(capture::stamped("Turntable")));
            (folder, (settings.loop_width, settings.loop_height), settings.loop_frames())
        });
        let size = capture::even_size(size.0, size.1);
        let fail = |app: &mut Running, why: String| {
            app.doc.turntable.status = why;
            app.doc.turntable.status_ok = false;
        };
        if !self.fits(size) {
            return fail(self, format!("This graphics card cannot draw a {} x {} picture.", size.0, size.1));
        }
        let writer = match LoopWriter::create(folder, total, size) {
            Ok(writer) => writer,
            Err(e) => return fail(self, e),
        };
        let camera = &mut self.doc.camera;
        camera.cancel_glide();
        let start = Pose { yaw: camera.yaw, pitch: camera.pitch, distance: camera.distance, pivot: camera.pivot };
        let run = LoopRun {
            writer: Some(writer),
            target: Target::new(&self.gpu, size),
            start,
            direction: settings.direction(),
            spinning_before: self.doc.turntable.spin.on,
            clean_before: self.doc.opts.clean_view,
            due: false,
            ending: None,
        };
        self.doc.turntable.spin.halt();
        // The window shows what is being written, so it gets the clean view
        // too, and no stroke can land on the model mid-turn.
        turntable::set_clean_view(&mut self.doc, true);
        self.doc.turntable.recording = Some(Progress { written: 0, total });
        self.doc.turntable.status.clear();
        self.recording.looping = Some(run);
        self.redraw = true;
    }

    /// Stops a recording that will not be kept. What was written is removed
    /// once the picture writers have stopped.
    fn cancel_loop(&mut self, why: &str) {
        if let Some(run) = &mut self.recording.looping {
            if run.ending.is_none() {
                if let Some(writer) = &run.writer {
                    writer.cancel();
                }
                run.ending = Some(why.to_owned());
            }
        }
    }

    fn loop_capture(&mut self) {
        let Some(mut run) = self.recording.looping.take() else { return };
        let Some(mut writer) = run.writer.take() else { return };
        if run.due {
            run.due = false;
            // The window's camera where it stands, with a wider lens if the
            // picture is narrower than the window.
            let window = self.view.size.x / self.view.size.y.max(1.0);
            let picture = run.target.size.0 as f32 / run.target.size.1 as f32;
            let fov_y = capture::widened_fov(self.doc.camera.fov_y, window, picture);
            let camera = self.camera_at(self.artist_shot(), fov_y);
            self.draw_capture(&mut run.target, camera);
            writer.push(crate::render::read_texture(&self.gpu, &run.target.texture));
        }
        let (written, total) = (writer.written(), writer.total());
        self.doc.turntable.recording = Some(Progress { written, total });
        if run.ending.is_none() {
            if let Some(e) = writer.error() {
                writer.cancel();
                run.ending = Some(format!("Recording failed. {e}"));
            }
        }
        self.redraw = true;

        let outcome = match &run.ending {
            Some(why) if writer.settled() => Some(Err(why.clone())),
            None if writer.complete() => Some(Ok(writer.folder().to_owned())),
            _ => None,
        };
        let Some(outcome) = outcome else {
            run.writer = Some(writer);
            self.recording.looping = Some(run);
            return;
        };

        // Back to the starting view, which the next picture would have been anyway.
        let camera = &mut self.doc.camera;
        (camera.yaw, camera.pitch, camera.distance, camera.pivot) =
            (run.start.yaw, run.start.pitch, run.start.distance, run.start.pivot);
        let table = &mut self.doc.turntable;
        table.recording = None;
        table.spin.on = run.spinning_before;
        match outcome {
            Ok(folder) => {
                let seconds = total as f32 / table.settings.loop_fps.max(1) as f32;
                let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                table.status = format!("Saved {total} pictures ({seconds:.1} s loop) in {name}");
                table.status_ok = true;
                if let Some(Script::Loop { .. }) = self.recording.script {
                    println!("loop: {total} pictures of {}x{} in {}", run.target.size.0, run.target.size.1, folder.display());
                }
            }
            Err(why) => {
                writer.discard();
                table.status = why;
                table.status_ok = false;
            }
        }
        if !run.clean_before {
            turntable::set_clean_view(&mut self.doc, false);
        }
    }

    // ---- timelapse ---------------------------------------------------------

    /// Starts recording. `script` names the file for a run from the command line.
    fn start_timelapse(&mut self, script: Option<PathBuf>) {
        if self.recording.timelapse.is_some() {
            return;
        }
        let options = self.doc.timelapse.options;
        let size = capture::even_size(options.width.max(320), options.height.max(240));
        let fail = |app: &mut Running, why: String| {
            app.doc.timelapse.status = format!("Could not start recording. {why}");
            app.doc.timelapse.status_ok = false;
        };
        if !self.fits(size) {
            return fail(self, format!("This graphics card cannot draw a {} x {} picture.", size.0, size.1));
        }
        let path = script.unwrap_or_else(|| {
            capture::unused(self.recordings_folder().join(crate::io::export_file_name("Timelapse", "mp4")))
        });
        let recorder = match Recorder::start(&self.gpu, path, size, options.frame_rate) {
            Ok(recorder) => recorder,
            Err(e) => return fail(self, e),
        };
        self.recording.timelapse = Some(TimelapseRun {
            recorder,
            rig: Rig::default(),
            schedule: Schedule::default(),
            target: Target::new(&self.gpu, size),
        });
        let t = &mut self.doc.timelapse;
        t.phase = Phase::Recording;
        t.status = "Recording. Sculpt, and the video follows.".into();
        t.status_ok = true;
        self.redraw = true;
    }

    /// Stops taking frames; the file is finished over the next frames, so
    /// pressing Stop never makes the app hitch.
    fn stop_timelapse(&mut self) {
        if let Some(run) = &mut self.recording.timelapse {
            run.recorder.stop();
            self.doc.timelapse.phase = Phase::Saving;
            self.doc.timelapse.status = "Saving the video...".into();
            self.redraw = true;
        }
    }

    fn timelapse_capture(&mut self) {
        let Some(mut run) = self.recording.timelapse.take() else { return };
        // The encoder gave up mid-recording (a full disk, most likely).
        if !run.recorder.stopping() && run.recorder.error().is_some() {
            run.recorder.stop();
            self.doc.timelapse.phase = Phase::Saving;
        }
        if run.recorder.stopping() {
            match run.recorder.finished() {
                Some(outcome) => self.timelapse_concluded(outcome),
                None => {
                    // Still writing: keep the frames coming so this is asked again.
                    self.redraw = true;
                    self.recording.timelapse = Some(run);
                }
            }
            return;
        }
        run.recorder.poll();

        // Read every frame, so the sliders work while recording. The size
        // and the frame rate were fixed when it started.
        let options = self.doc.timelapse.options;
        let signal = activity::current();
        let editing = signal.editing(activity::now(), options.idle_grace);
        if run.schedule.tick(self.recording.dt, editing, options.capture_interval()) && run.recorder.has_room() {
            // The camera moves only on recorded frames, one frame of video
            // at a time (see timelapse/rig.rs).
            let scene = &self.doc.scene;
            let subject = signal.point.and_then(|(id, _)| scene.get(id)).unwrap_or(scene.active());
            let bounds = scene.bounds_of(subject, &self.doc.mesh);
            let size = run.target.size;
            let shot = run.rig.step(
                &options,
                &Subject {
                    artist: self.artist_shot(),
                    edit_point: signal.point.and_then(|(id, p)| Some(scene.get(id)?.matrix().transform_point3(p))),
                    bounds: (!bounds.is_empty()).then_some(bounds),
                    fov_y: self.doc.camera.fov_y,
                    aspect: size.0 as f32 / size.1 as f32,
                },
            );
            let camera = self.camera_at(shot, self.doc.camera.fov_y);
            self.draw_capture(&mut run.target, camera);
            run.recorder.capture(&self.gpu, &run.target.texture);
        }

        let frames = run.recorder.frames_taken();
        let seconds = frames as f32 / options.frame_rate.max(1) as f32;
        let status = if editing {
            format!("Recording   {seconds:.1} s of video ({frames} frames)")
        } else {
            format!("Paused while you are not sculpting   {seconds:.1} s of video so far")
        };
        // While the model is changing every frame counts; after that, frames
        // are only needed until the last copies are back, and for the card
        // to show a line that has just changed (the interface was built
        // before this ran).
        if editing || run.recorder.pending() || status != self.doc.timelapse.status {
            self.redraw = true;
        }
        self.doc.timelapse.status = status;
        self.recording.timelapse = Some(run);
    }

    fn timelapse_concluded(&mut self, outcome: Outcome) {
        let t = &mut self.doc.timelapse;
        t.phase = Phase::Idle;
        match &outcome.error {
            None => {
                let name = outcome.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                t.status = format!("Saved {name} ({:.1} s)", outcome.seconds);
                t.status_ok = true;
                t.last_output = Some(outcome.path.clone());
            }
            Some(e) => {
                t.status = e.clone();
                t.status_ok = false;
            }
        }
        if outcome.dropped > 0 {
            log::warn!("timelapse: {} frames were dropped because the encoder fell behind", outcome.dropped);
        }
        if let Some(Script::Timelapse { .. }) = self.recording.script {
            println!(
                "timelapse: {} frames, {:.2} s, {} dropped, {} -> {}",
                outcome.frames,
                outcome.seconds,
                outcome.dropped,
                outcome.error.as_deref().unwrap_or("ok"),
                outcome.path.display()
            );
            // What the file says of itself, read back from disk.
            match timelapse::mp4::summary(&outcome.path) {
                Ok(file) => println!("timelapse file: {file:?}"),
                Err(e) => println!("timelapse file: {e}"),
            }
        }
        self.redraw = true;
    }

    // ---- recordings run from the command line ------------------------------

    /// Start of a frame: the script's next move.
    fn script_step(&mut self) {
        let lead_in = self.frames >= SCRIPT_LEAD_IN;
        match &mut self.recording.script {
            Some(Script::Loop { folder, size, frames, started }) if lead_in && !*started => {
                *started = true;
                let spec = (folder.clone(), *size, *frames);
                self.start_loop(Some(spec));
            }
            Some(Script::Timelapse { path, since, .. }) if lead_in && since.is_none() => {
                *since = Some(Instant::now());
                let path = path.clone();
                // Small and in real time, so a few strokes make a few seconds.
                let options = &mut self.doc.timelapse.options;
                (options.width, options.height, options.speed) = (640, 360, 1.0);
                self.start_timelapse(Some(path));
            }
            Some(Script::Timelapse { frame, since: Some(since), .. }) => {
                let at = *frame;
                if at as f32 <= since.elapsed().as_secs_f32() * 60.0 {
                    *frame += 1;
                    self.script_stroke(at);
                }
            }
            _ => {}
        }
        if self.recording.script.is_some() {
            self.redraw = true;
        }
    }

    /// Frame `at` of the scripted session: strokes fed to the brush a few
    /// samples a frame, as a pen would, with rests between them and a turn
    /// of the view half way through.
    fn script_stroke(&mut self, at: u32) {
        let mut start = 0;
        for (index, (brush, radius, from, to, rest)) in SCRIPT_STROKES.into_iter().enumerate() {
            let length = SCRIPT_STROKE_FRAMES + 1 + rest;
            if at >= start + length {
                start += length;
                continue;
            }
            let step = at - start;
            // `SCULPT_DEMO_CAMERA=close`: the window is closed with the
            // recording still running, part of the way through the third
            // stroke. The video must come out finished all the same.
            if index == 2 && step == 12 && std::env::var("SCULPT_DEMO_CAMERA").is_ok_and(|v| v == "close") {
                println!("timelapse: closing the app mid-recording");
                self.recording.close = true;
            }
            if step > SCRIPT_STROKE_FRAMES {
                // Resting. Part of the way through the long rest after the
                // second stroke the "artist" turns the model, for the camera
                // to follow.
                if index == 1 && step == SCRIPT_STROKE_FRAMES + 40 {
                    self.doc.camera.orbit(Vec2::new(-140.0, 30.0), None);
                }
                return;
            }
            let k = self.view.size.y / 900.0;
            let centre = self.view.size * 0.5;
            let total = SCRIPT_STROKE_FRAMES * SCRIPT_SAMPLES_PER_FRAME;
            let sample = |i: u32| PointerSample {
                pos: centre + from.lerp(to, i as f32 / total as f32) * k,
                pressure: None,
                time: i as f64 / 120.0,
            };
            if step == 0 {
                self.sync_view();
                let settings = &mut self.doc.sculptor.settings;
                settings.brush = brush;
                settings.mask_mode = false;
                settings.radius_px = radius * k;
                let doc = &mut self.doc;
                if doc.sculptor.begin(&mut doc.mesh, &self.local, sample(0), false, false) {
                    self.input.samples.clear();
                }
            } else if self.doc.sculptor.is_active() {
                let first = (step - 1) * SCRIPT_SAMPLES_PER_FRAME + 1;
                self.input.samples.extend((first..first + SCRIPT_SAMPLES_PER_FRAME).map(sample));
                if step == SCRIPT_STROKE_FRAMES {
                    self.finish_stroke();
                }
            }
            return;
        }
        // Every stroke and rest is done.
        if self.doc.timelapse.phase == Phase::Recording {
            self.stop_timelapse();
        }
    }

    /// End of a frame: closes the app once the script's recording is over.
    fn script_end(&mut self, event_loop: &ActiveEventLoop) {
        if self.recording.close {
            return event_loop.exit();
        }
        let done = match &self.recording.script {
            Some(Script::Loop { started, .. }) => *started && self.recording.looping.is_none(),
            Some(Script::Timelapse { since, .. }) => since.is_some() && self.recording.timelapse.is_none(),
            Some(Script::Check(_)) => return self.check_step(event_loop),
            None => false,
        };
        if done {
            if let Some(Script::Loop { .. }) = self.recording.script {
                let table = &self.doc.turntable;
                if !table.status_ok {
                    eprintln!("loop: {}", table.status);
                }
            } else if !self.doc.timelapse.status_ok {
                eprintln!("timelapse: {}", self.doc.timelapse.status);
            }
            self.recording.script = None;
            event_loop.exit();
        }
    }

    /// One frame of the self-check (see [`Check`]).
    fn check_step(&mut self, event_loop: &ActiveEventLoop) {
        let Some(Script::Check(mut c)) = self.recording.script.take() else { return };
        if self.frames < SCRIPT_LEAD_IN {
            c.since = Instant::now();
            self.recording.script = Some(Script::Check(c));
            return;
        }
        c.frames += 1;
        let mut failed = 0;
        let mut expect = |what: &str, ok: bool| {
            println!("check: {what} ... {}", if ok { "ok" } else { "FAILED" });
            failed += u32::from(!ok);
        };
        // Signed shortest way from one yaw to another.
        let turned = |from: f32, to: f32| (to - from + 180.0).rem_euclid(360.0) - 180.0;
        let yaw = self.doc.camera.yaw;
        let waited = c.since.elapsed().as_secs_f32();
        // Whether nothing but the check has moved the pointer (the real one
        // may wander over the window while this runs).
        let undisturbed = self.input.pointer == c.pointer;
        let stage = c.stage;
        match stage {
            0 => {
                c.yaw = yaw;
                self.key_pressed(event_loop, KeyCode::KeyO);
                expect("O starts the spin", self.doc.turntable.spin.on);
                c.stage += 1;
            }
            1 if c.frames >= 45 => {
                let turn = turned(c.yaw, yaw);
                expect(&format!("the view goes round to its right ({turn:.1} degrees in {waited:.2} s)"), turn < -3.0);
                self.key_pressed(event_loop, KeyCode::Tab);
                expect("Tab puts the clean view up", self.doc.opts.clean_view);
                let time = self.time();
                expect("the clean view shows its hint", self.doc.turntable.hint_alpha(time) > 0.99);
                let (steps, brush) = (self.doc.history.steps(), self.doc.sculptor.settings.brush);
                for key in [KeyCode::KeyR, KeyCode::Digit3, KeyCode::KeyM, KeyCode::KeyZ] {
                    self.key_pressed(event_loop, key);
                }
                let s = &self.doc.sculptor.settings;
                let untouched = self.doc.history.steps() == steps && s.brush == brush && !s.mask_mode;
                expect("R, 3, M and Z do nothing in the clean view", untouched);
                self.input.pointer = self.view.size * 0.5;
                self.input.inside = true;
                c.pointer = self.input.pointer;
                self.pointer_pressed(MouseButton::Left, None);
                let orbiting = self.input.drag == Drag::Orbit && !self.doc.sculptor.is_active();
                expect("a press on the model orbits and does not sculpt", orbiting);
                c.yaw = self.doc.camera.yaw;
                c.stage += 1;
            }
            2 if c.frames >= 6 => {
                if undisturbed {
                    expect("the spin lets go of the view while it is dragged", turned(c.yaw, yaw).abs() < 0.01);
                } else {
                    println!("check: the spin lets go of the view while it is dragged ... skipped (the real pointer moved)");
                }
                expect("the pointer shows while it is in use", !self.recording_hides_pointer());
                self.pointer_released(MouseButton::Left);
                c.pointer = self.input.pointer;
                c.stage += 1;
            }
            3 if waited >= POINTER_REST_SECONDS + 0.5 => {
                if undisturbed {
                    expect("the pointer hides once it has rested", self.recording_hides_pointer());
                } else {
                    println!("check: the pointer hides once it has rested ... skipped (the real pointer moved)");
                }
                self.key_pressed(event_loop, KeyCode::Escape);
                expect("Esc leaves the clean view", !self.doc.opts.clean_view);
                self.key_pressed(event_loop, KeyCode::KeyO);
                expect("O stops the spin", !self.doc.turntable.spin.on);
                c.stage += 1;
            }
            4 if !self.doc.turntable.spin.moving() => {
                expect(&format!("the spin eases out ({} frames)", c.frames), c.frames > 3);
                let s = &mut self.doc.turntable.settings;
                (s.speed, s.loop_fps, s.loop_width, s.loop_height) = (180.0, 24, 640, 360);
                c.yaw = yaw;
                self.doc.turntable.request = Some(turntable::Request::RecordLoop);
                c.stage += 1;
            }
            5 if c.frames >= 6 => {
                let recording = self.recording.looping.is_some() && self.doc.turntable.recording.is_some();
                expect("Record 360 Loop starts recording, in the clean view", recording && self.doc.opts.clean_view);
                self.key_pressed(event_loop, KeyCode::KeyO);
                self.key_pressed(event_loop, KeyCode::Tab);
                expect("O and Tab wait while it records", !self.doc.turntable.spin.on && self.doc.opts.clean_view);
                c.stage += 1;
            }
            6 if self.recording.looping.is_none() => {
                let table = &self.doc.turntable;
                expect(&format!("the loop finishes: \"{}\"", table.status), table.status_ok && table.recording.is_none());
                expect("the panels come back", !self.doc.opts.clean_view);
                expect("the view is back where it started", yaw == c.yaw);
                let pictures = loops_in(&self.recordings_folder());
                expect(&format!("one loop of 48 pictures is on disk ({pictures:?})"), pictures == [48]);
                self.doc.turntable.request = Some(turntable::Request::RecordLoop);
                c.stage += 1;
            }
            7 if c.frames >= 5 => {
                self.key_pressed(event_loop, KeyCode::Escape);
                expect("Esc cancels a recording", self.recording.looping.as_ref().is_some_and(|run| run.ending.is_some()));
                c.stage += 1;
            }
            8 if self.recording.looping.is_none() => {
                let table = &self.doc.turntable;
                expect(&format!("the cancelled loop says so: \"{}\"", table.status), !table.status_ok && table.recording.is_none());
                let pictures = loops_in(&self.recordings_folder());
                expect(&format!("nothing of the cancelled loop is left ({pictures:?})"), pictures == [48]);
                expect("the panels come back", !self.doc.opts.clean_view);
                c.stage += 1;
            }
            _ => {}
        }
        c.failed += failed;
        if c.stage != stage {
            (c.frames, c.since) = (0, Instant::now());
        }
        if c.stage > 8 || c.frames > 1800 {
            if c.stage <= 8 {
                println!("check: stage {} never finished ... FAILED", c.stage);
                c.failed += 1;
            }
            println!("check: done, {} failed", c.failed);
            event_loop.exit();
            return;
        }
        self.recording.script = Some(Script::Check(c));
    }
}
