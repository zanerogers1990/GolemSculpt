//! Keeping work through a crash, on the app's side: the recovery copy
//! written every few minutes, the question at start-up about a copy an
//! earlier run left behind, and what happens when the app has to stop on a
//! fault. Where the copies live, and the rules they are kept by, are in
//! `crate::recovery`.
//!
//! A copy is written on the interface thread, between strokes, through the
//! same code that saves a scene. That is a pause: on the machine this was
//! written on, 18 ms for a million triangles, 55 to 100 ms for four million
//! and 160 ms for twelve million, most of it the wait for the disk
//! (`timing_of_a_recovery_copy` in `crate::recovery` measures it). It is
//! taken once the hand has been off the model for a second, and a toast says
//! what it was when it was long enough to feel. Writing on another thread
//! would need a second copy of every mesh, or the scene shared between
//! threads; a tenth of a second every three minutes does not earn that.
//!
//! Stopping on a fault. A panic on the interface thread unwinds to the event
//! loop (`App::guarded` in app.rs), and only then, with nothing left running
//! that could be changing the scene, is the scene looked at. If every
//! finished step is already in a copy, nothing more is written. Otherwise
//! the scene is checked (`recovery::sound`) and written beside the regular
//! copies, marked as made on a fault: whatever was being done when the fault
//! struck may be half done in it, which is why the question at start-up says
//! so and offers the last regular copy as well. A fault the graphics card
//! reports is noted by its callback and acted on between events, when the
//! scene is whole.
//!
//! What this cannot catch: the program being killed, the power going, or
//! the system running out of memory, none of which gives the program a last
//! word. Then the newest regular copy, at most a few minutes old, is what
//! there is.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use glam::Vec3;
use sculpt_toolkit::{Job, JobCtx};

use super::files::Mark;
use super::{Drag, Running};
use crate::doc::Recover;
use crate::io;
use crate::prefs::{self, Home};
use crate::recovery::{self, About, Faults, Kept, Outcome, Schedule, Store};
use crate::sculpt::PointerSample;
use crate::ui::question::Question;

/// The variable that makes a scripted run a check of recovery itself (see
/// [`Script`]).
const CHECK: &str = "SCULPT_TEST_RECOVERY";

/// In the script of a check: put the closing message up even though the run
/// is a scripted one.
const SHOW_MESSAGE: &str = "message";

/// A copy that took longer than this to write says so in a toast: the pause
/// could be felt, and should not be a mystery.
const FELT: Duration = Duration::from_millis(200);

/// Whether this run is a scripted check of recovery. Such a run opens
/// without taking the keyboard and leaves the Claude bridge's port alone,
/// like a screenshot run.
pub(super) fn checking() -> bool {
    std::env::var_os(CHECK).is_some_and(|spec| !spec.is_empty())
}

pub(super) struct State {
    store: Store,
    schedule: Schedule,
    /// The scene's mark when this run's newest copy was written: that copy
    /// holds every step up to it.
    held: Option<Mark>,
    /// Copies an earlier run left behind, newest first, as found at start-up.
    found: Vec<Kept>,
    /// The user has answered the question about them. They are removed once
    /// this run has a copy or a save of its own, or closes properly; until
    /// then they are still the only copy of that work.
    answered: bool,
    /// The scene was restored from one of them, and this run has no copy of
    /// its own yet: they are still what holds that work.
    restored: bool,
    /// The file a restored copy was a copy of, for Save to offer.
    document: Option<PathBuf>,
    /// Where the graphics device leaves word of a fault.
    pub(super) faults: Faults,
    script: Script,
}

impl State {
    pub(super) fn new(screenshot: bool) -> State {
        let script = Script::from_env();
        let home = Home::for_run(screenshot);
        // A scripted run keeps no copies and offers none, even with a
        // settings folder of its own (visual checks share those, and a
        // question over the picture would spoil every one of them), unless
        // it is a check of this very thing.
        let store =
            if prefs::scripted() && !script.on { Store::nowhere() } else { Store::at(home.folder(), home.writable()) };
        State {
            store,
            schedule: Schedule::new(script.every.unwrap_or(recovery::EVERY)),
            held: None,
            found: Vec::new(),
            answered: false,
            restored: false,
            document: None,
            faults: Faults::default(),
            script,
        }
    }

    /// Whether any copy of this scene's work is on disk.
    fn has_copy(&self) -> bool {
        self.held.is_some() || self.restored
    }
}

impl Running {
    // ---- start-up: a copy an earlier run left behind

    /// Start-up: if a run before this one ended without closing properly and
    /// left copies of its work, asks whether to bring the newest back.
    pub(super) fn recovery_start(&mut self) {
        self.recovery.found = self.recovery.store.left_behind();
        let Some(newest) = self.recovery.found.first() else { return };
        let today = io::time_stamp();
        let when = |kept: &Kept| recovery::when_words(&kept.about.when, &today);
        let regular = self.recovery.found.iter().find(|kept| !kept.stopped).filter(|_| newest.stopped);
        let earlier = regular.map(|kept| when(kept).unwrap_or_else(|| "before that".to_owned()));
        let question =
            Question::Recover { when: when(newest), what: newest.about.words(), stopped: newest.stopped, earlier };
        self.ui.graph.ask(question);
        self.redraw = true;
        if let Some(answer) = self.recovery.script.answer.take() {
            self.ui.graph.answer(false);
            if let Some(answer) = answer {
                self.recover(answer);
            }
        }
    }

    /// The answer to that question. (Not Now does not come here: it leaves
    /// the copies as they are, to be asked about next time.)
    pub(super) fn recover(&mut self, answer: Recover) {
        self.recovery.answered = true;
        let found = self.recovery.found.clone();
        if answer == Recover::Discard {
            let folder = found.first().and_then(|kept| kept.file.parent()).map(|dir| dir.display().to_string());
            let folder = folder.unwrap_or_default();
            let text = format!("Not restored. The copy stays in {folder} until you save the scene or close the app.");
            return self.status(text, true);
        }
        let first = match answer {
            Recover::Earlier => found.iter().position(|kept| !kept.stopped).unwrap_or(0),
            _ => 0,
        };
        // A copy that cannot be read is passed over for the one before it.
        let mut unread = 0;
        for kept in &found[first.min(found.len())..] {
            match self.restore(kept) {
                Ok(objects) => {
                    let when = recovery::when_words(&kept.about.when, &io::time_stamp());
                    let when = when.map_or(String::new(), |when| format!(" kept {when}"));
                    let note = if unread > 0 { " A newer copy could not be read." } else { "" };
                    let objects = if objects == 1 { "1 object".to_owned() } else { format!("{objects} objects") };
                    let text = format!(
                        "Restored the copy{when} ({objects}).{note} \
                         It is not saved yet: Save (Ctrl + S) asks where to keep it."
                    );
                    return self.status(text, unread == 0);
                }
                Err(e) => {
                    log::error!("{e}");
                    unread += 1;
                }
            }
        }
        // Nothing was restored, so nothing is settled: the copies stay, and
        // the next start asks again.
        self.recovery.answered = false;
        self.status("The recovery copy could not be read. The scene is as it was.", false);
    }

    /// Puts a recovery copy in place of the scene, as one undo step. The
    /// scene has no file after it and counts as unsaved, so Save asks where
    /// it should go and closing asks first: the copy is never written over
    /// the user's own file without their say.
    fn restore(&mut self, kept: &Kept) -> io::Result<usize> {
        // A copy that brings a fault of its own with it must not take the
        // app down: the next start would offer it again and fall the same way.
        let opened = catch_unwind(AssertUnwindSafe(|| self.replace_scene(&kept.file)));
        let (objects, _) = opened.unwrap_or_else(|_| Err(format!("{} could not be opened.", kept.file.display())))?;
        self.doc.path = None;
        self.recovery.restored = true;
        self.recovery.document = kept.about.document.clone();
        Ok(objects)
    }

    /// The file a restored scene was a copy of, for the Save dialog to start from.
    pub(super) fn recovered_document(&self) -> Option<PathBuf> {
        self.recovery.document.clone()
    }

    // ---- while the app runs: a copy every few minutes

    /// Between events: writes a copy if one is due. Gives back when to look
    /// again if nothing happens before then, for the event loop to wake at.
    /// This is asked from the loop, not from the frame: a window that is
    /// minimised draws no frames, and its work is owed a copy all the same.
    pub(super) fn recovery_tick(&mut self) -> Option<Instant> {
        let busy = self.doc.sculptor.is_active() || self.input.drag != Drag::None;
        let changed =
            self.recovery.store.keeps() && self.unsaved() && self.recovery.held != Some(self.history_mark());
        if self.recovery.schedule.due(Instant::now(), changed, busy) {
            self.recovery.schedule.restart();
            if let Err(e) = self.keep_copy(false) {
                log::warn!("no recovery copy was written: {e}");
                // Said once here; it is tried again when the next copy is due.
                self.status(format!("No recovery copy could be kept. {e}"), false);
            }
            self.redraw = true;
        }
        self.recovery.schedule.wake()
    }

    /// Writes the scene to a new recovery copy, through the code that saves
    /// a scene (whole or not at all, never over another file). `stopped`
    /// marks the copy made as the app stops on a fault: the scene is checked
    /// first, and no older copy is dropped for it.
    fn keep_copy(&mut self, stopped: bool) -> io::Result<()> {
        let started = Instant::now();
        let mark = self.history_mark();
        let file = self.recovery.store.next_file(stopped)?;
        let about = {
            let (meshes, objects, active) = self.scene_for_file()?;
            if stopped {
                recovery::sound(&meshes).map_err(|e| format!("The scene was not whole ({e})."))?;
            }
            io::save_scene_with(&meshes, &objects, active, &self.details(), &file)?;
            About {
                when: io::time_stamp(),
                document: self.doc.path.clone(),
                objects: objects.len(),
                triangles: meshes.iter().map(|mesh| mesh.triangle_count()).sum(),
            }
        };
        self.recovery.store.wrote(&file, &about);
        if !stopped {
            self.recovery.held = Some(mark);
            // This run has a copy of its own now.
            self.forget_answered();
        }
        let took = started.elapsed();
        if self.recovery.script.on {
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            println!("copy {name} {} triangles {:.0} ms", about.triangles, took.as_secs_f64() * 1000.0);
        }
        if took > FELT && !stopped {
            self.toast("Recovery copy kept");
        }
        Ok(())
    }

    /// Removes the copies an earlier run left, once the user has answered
    /// about them and this run no longer needs them.
    fn forget_answered(&mut self) {
        if self.recovery.answered {
            let found = std::mem::take(&mut self.recovery.found);
            self.recovery.store.remove(&found);
            (self.recovery.answered, self.recovery.restored) = (false, false);
        }
    }

    /// The scene has been saved to a file of the user's: its copies are
    /// removed, so a crash after this offers nothing older than that file.
    pub(super) fn recovery_saved(&mut self) {
        self.recovery.store.clear();
        self.recovery.held = None;
        self.recovery.document = None;
        self.recovery.schedule.restart();
        self.forget_answered();
    }

    /// The app is closing properly (the user said so, with or without
    /// saving): nothing is left for the next start to ask about.
    pub(super) fn recovery_exit(&mut self) {
        self.recovery.store.clear();
        self.forget_answered();
    }

    // ---- stopping on a fault

    /// The app is about to stop: makes sure of a copy of the unsaved work,
    /// and says what became of it. `whole` is whether the scene can be
    /// trusted as it stands (a fault of the graphics card's, found between
    /// events) or may have been caught mid-change (a panic).
    fn last_copy(&mut self, whole: bool) -> Outcome {
        if !self.unsaved() {
            return Outcome::NothingUnsaved;
        }
        if !self.recovery.store.keeps() {
            return Outcome::Lost;
        }
        // Every finished step is in a copy already. What was under way when
        // the fault struck is not worth risking a damaged copy for.
        if self.recovery.held == Some(self.history_mark()) {
            return Outcome::Kept;
        }
        match self.keep_copy(!whole) {
            Ok(()) => Outcome::Kept,
            Err(e) => {
                recovery::log(&format!("The scene could not be kept as the app stopped: {e}"));
                if self.recovery.has_copy() { Outcome::EarlierOnly } else { Outcome::Lost }
            }
        }
    }

    /// Between events: if the graphics card has reported a fault, keeps a
    /// copy of the scene, which is whole at this point, and closes the app.
    pub(super) fn stop_on_graphics_fault(&mut self) {
        let Some(fault) = self.recovery.faults.take() else { return };
        let outcome = self.last_copy(true);
        close(&format!("{fault}, and GolemSculpt has to close."), outcome)
    }

    // ---- scripted checks

    /// Whether the script still has steps to run: it needs frames without input.
    pub(super) fn recovery_script_running(&self) -> bool {
        self.recovery.script.running()
    }

    /// Once a frame: runs the steps a scripted check has for it.
    pub(super) fn recovery_frame(&mut self) {
        if !self.recovery.script.on {
            return;
        }
        self.recovery.script.frame += 1;
        let frame = self.recovery.script.frame;
        if let Some(result) = self.recovery.script.job.as_mut().and_then(|job| job.poll()) {
            println!("job came back: {result:?}");
            self.recovery.script.job = None;
        }
        let steps = &self.recovery.script.steps;
        let now: Vec<Step> = steps.iter().filter(|(at, _)| *at == frame).map(|&(_, step)| step).collect();
        for step in now {
            self.recovery_check_step(step);
        }
    }

    fn recovery_check_step(&mut self, step: Step) {
        // Where the scripted strokes go: a row lower each time.
        let row = 0.25 - 0.12 * self.recovery.script.strokes as f32;
        match step {
            Step::Stroke => {
                self.recovery.script.strokes += 1;
                self.symmetry_demo_stroke(Vec3::new(-0.15, row, 0.0), Vec3::new(0.2, row, 0.0));
                println!("stroke: {} steps to undo, unsaved {}", self.doc.history.steps().0, self.unsaved());
            }
            Step::Scene => match self.scene_for_file() {
                Ok((meshes, objects, _)) => {
                    let triangles: usize = meshes.iter().map(|mesh| mesh.triangle_count()).sum();
                    let print = recovery::fingerprint(&meshes, &objects);
                    let (objects, unsaved) = (objects.len(), self.unsaved());
                    println!("scene {print:016x} {objects} objects {triangles} triangles unsaved {unsaved}");
                }
                Err(e) => println!("scene: {e}"),
            },
            Step::Panic => panic!("a fault made on purpose ({CHECK})"),
            Step::PanicMidStroke => {
                // A stroke begun and half drawn: its change is in the mesh but in no undo step.
                self.sync_view();
                let from = self.view.project(Vec3::new(-0.15, row, 0.0)).unwrap_or(self.view.size * 0.5);
                let at = |i: usize| PointerSample {
                    pos: from + glam::Vec2::new(4.0 * i as f32, 0.0),
                    pressure: None,
                    time: i as f64 / 240.0,
                };
                let doc = &mut self.doc;
                let began = doc.begin_stroke(&self.local, at(0), false, false);
                for i in 1..=20 {
                    doc.sculptor.sample(&mut doc.mesh, &self.local, at(i));
                }
                doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
                println!("mid-stroke: began {began}, stroke under way {}", self.doc.sculptor.is_active());
                panic!("a fault made on purpose in the middle of a stroke ({CHECK})");
            }
            Step::JobPanic => {
                let work = |_: &JobCtx| -> Result<(), String> { panic!("a fault made on purpose in a job") };
                self.recovery.script.job = Some(self.doc.tools.jobs.spawn("The test job", work));
            }
            Step::Save => {
                let file = Home::for_run(false).folder().map(|dir| dir.join("saved by the check.sclp"));
                let saved = match file {
                    Some(file) => self.save_scene(&file),
                    None => Err("no settings folder to save in".to_owned()),
                };
                println!("save: {saved:?}, unsaved {}", self.unsaved());
            }
            Step::GraphicsError => {
                // A texture no wider than nothing: the device refuses it.
                let _ = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("a fault made on purpose"),
                    size: wgpu::Extent3d { width: 0, height: 0, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.gpu.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
            }
            Step::DeviceLost => {
                // A device that is destroyed says it is lost the next time it
                // is asked how its work is going. (One the driver loses says
                // so by itself, there and then.)
                self.gpu.device.destroy();
                let _ = self.gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
            }
            // As if the power had gone: no last word, no tidying.
            Step::Kill => std::process::exit(9),
            Step::Quit => self.quit(false),
        }
    }
}

/// The interface thread panicked and the panic has unwound to the event
/// loop: makes sure of a copy of the scene if there is one to make, tells
/// the user, and ends the program. `running` is `None` if the fault came
/// before the app was up.
pub(super) fn stop_after_panic(running: Option<&mut Running>) -> ! {
    let outcome = match running {
        Some(running) => {
            let without = if running.recovery.has_copy() { Outcome::EarlierOnly } else { Outcome::Lost };
            // A second fault while saving what the first left must end here too.
            catch_unwind(AssertUnwindSafe(|| running.last_copy(false))).unwrap_or(without)
        }
        None => Outcome::NothingUnsaved,
    };
    close("GolemSculpt ran into a fault and has to close.", outcome)
}

/// Tells the user why the app is closing and what became of their work,
/// then ends the program at once: after a fault, tidying up could do harm.
fn close(cause: &str, outcome: Outcome) -> ! {
    let words = recovery::farewell(cause, outcome, recovery::log_file());
    recovery::log(&words.replace("\n\n", " "));
    // A scripted run has nobody to press OK, unless the check is of the
    // message itself (`message` in the script).
    let shown = std::env::var(CHECK).is_ok_and(|spec| spec.split(',').any(|part| part.trim() == SHOW_MESSAGE));
    if prefs::scripted() {
        println!("stopped: {outcome:?}");
        eprintln!("{words}");
    }
    if shown || !prefs::scripted() {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("GolemSculpt")
            .set_description(words)
            .show();
    }
    std::process::exit(101)
}

// ---- the script of a scripted check

/// One thing a scripted check does in a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// One Clay stroke, as an undo step.
    Stroke,
    /// Prints the scene's fingerprint.
    Scene,
    /// Panics on the interface thread.
    Panic,
    /// Begins a stroke, draws half of it and panics.
    PanicMidStroke,
    /// Starts a background job that panics.
    JobPanic,
    /// Saves the scene, as Save does, to a file in the settings folder.
    Save,
    /// Asks the graphics device for something it refuses.
    GraphicsError,
    /// Destroys the graphics device, which then reports itself lost.
    DeviceLost,
    /// Ends the program on the spot.
    Kill,
    /// Closes the app properly, without saving.
    Quit,
}

/// What `SCULPT_TEST_RECOVERY` asks for, so that keeping copies, the
/// question at start-up and stopping on a fault can be checked end to end
/// without a person or a real crash. A list separated by commas:
///
/// - `every=0.3`: a copy is due after this many seconds, not three minutes;
/// - `answer=restore | earlier | discard | later`: answers the question at start-up;
/// - `message`: the message the app closes with on a fault is put up, as it
///   is for a person, and the run waits for it to be closed;
/// - `stroke@30`, `scene@40`, `save@45`, `panic@50`, `stroke-panic@50`,
///   `job-panic@50`, `graphics-error@50`, `device-lost@50`, `kill@50`,
///   `quit@50`: does that in the frame of that number (see [`Step`]).
///
/// Setting it at all (`on` will do) switches recovery on for a scripted run,
/// which otherwise keeps no copies. It needs `SCULPT_SETTINGS_DIR` to say where.
#[derive(Default)]
struct Script {
    on: bool,
    every: Option<Duration>,
    /// The answer to give at start-up; `Some(None)` is Not Now.
    answer: Option<Option<Recover>>,
    steps: Vec<(u32, Step)>,
    frame: u32,
    strokes: u32,
    /// The job `job-panic` started, until it comes back.
    job: Option<Job<()>>,
}

impl Script {
    fn from_env() -> Script {
        let spec = std::env::var(CHECK).ok().filter(|spec| !spec.is_empty());
        spec.map_or_else(Script::default, |spec| Script::parse(&spec))
    }

    fn parse(spec: &str) -> Script {
        let mut script = Script { on: true, ..Script::default() };
        let seconds = |text: &str| text.parse::<f32>().ok().filter(|seconds| seconds.is_finite());
        let step_at = |(name, at): (&str, &str)| Some((at.parse().ok()?, step_named(name)?));
        let words = ["on", SHOW_MESSAGE];
        for part in spec.split(',').map(str::trim).filter(|part| !part.is_empty() && !words.contains(part)) {
            if let Some(seconds) = part.strip_prefix("every=").and_then(seconds) {
                script.every = Some(Duration::from_secs_f32(seconds.clamp(0.01, 86_400.0)));
            } else if let Some(answer) = part.strip_prefix("answer=").and_then(answer_named) {
                script.answer = Some(answer);
            } else if let Some(step) = part.split_once('@').and_then(step_at) {
                script.steps.push(step);
            } else {
                eprintln!("{CHECK}: no such step as {part:?}");
            }
        }
        script
    }

    fn running(&self) -> bool {
        self.on && (self.job.is_some() || self.steps.iter().any(|(at, _)| *at > self.frame))
    }
}

fn answer_named(name: &str) -> Option<Option<Recover>> {
    Some(match name {
        "restore" => Some(Recover::Newest),
        "earlier" => Some(Recover::Earlier),
        "discard" => Some(Recover::Discard),
        "later" => None,
        _ => return None,
    })
}

fn step_named(name: &str) -> Option<Step> {
    Some(match name {
        "stroke" => Step::Stroke,
        "scene" => Step::Scene,
        "panic" => Step::Panic,
        "stroke-panic" => Step::PanicMidStroke,
        "job-panic" => Step::JobPanic,
        "save" => Step::Save,
        "graphics-error" => Step::GraphicsError,
        "device-lost" => Step::DeviceLost,
        "kill" => Step::Kill,
        "quit" => Step::Quit,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_reads_its_interval_its_answer_and_its_steps() {
        let script = Script::parse("every=0.25, answer=earlier,stroke@30,scene@40,stroke-panic@41,nonsense,quit@x");
        assert!(script.on);
        assert_eq!(script.every, Some(Duration::from_millis(250)));
        assert_eq!(script.answer, Some(Some(Recover::Earlier)));
        assert_eq!(script.steps, [(30, Step::Stroke), (40, Step::Scene), (41, Step::PanicMidStroke)]);
        assert_eq!(Script::parse("answer=later").answer, Some(None), "Not Now is an answer too");
    }

    #[test]
    fn a_script_keeps_frames_coming_until_its_last_step() {
        let mut script = Script::parse("on");
        assert!(script.on && !script.running(), "nothing to do: frames come on input, as ever");
        script = Script::parse("scene@3");
        assert!(script.running());
        script.frame = 3;
        assert!(!script.running(), "its last step has run");
        assert!(!Script::default().running() && !Script::default().on, "without the variable there is no script");
    }

    #[test]
    fn the_shortest_interval_a_script_can_ask_for_is_not_nothing() {
        // Rule 9 of crates/README.md: a limit is tested at its lowest input.
        assert_eq!(Script::parse("every=0").every, Some(Duration::from_millis(10)));
        assert_eq!(Script::parse("every=-5").every, Some(Duration::from_millis(10)));
        assert_eq!(Script::parse("every=NaN").every, None, "not a time at all");
    }
}
