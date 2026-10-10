//! The Keys tool itself: which two pieces it has, the joints planned on them,
//! and building them in.
//!
//! The planned joints are a scaffold. While the tool is up they are previews
//! with their own undo history; Apply builds them into both pieces on a
//! background job and commits the two new meshes as one undo step. The
//! scaffold stays with the tool afterwards, so undoing that step hands the
//! joints back as they were planned.
//!
//! Split across files: this one (the pair, apply, undo), `input.rs` (the
//! pointer and keys), `preview.rs` (the pictures), `panel.rs` (the numbers).

use std::time::Duration;

use glam::{Quat, Vec3};
use sculpt_geom::{Aabb, LengthUnit, units};
use sculpt_toolkit::{
    Button, Command, Ctx, Draw, Event, Info, Job, Key, MeshData, NewObject, ObjectId, Placement, Preview, Tool,
};

use crate::builder::{Built, NEW_VERTEX};
use crate::cutting::split_by_plane;
use crate::input::{Dragging, Hit, ViewSide};
use crate::pair::{Pair, Showing, Snapshot, Source};
use crate::planner::{self, KeyPlan, KeySettings};
use crate::profile::KeyShape;

/// Steps of the joints' own history that are kept.
const HISTORY: usize = 128;

/// What a finished build hands back: both pieces ready for the scene, and
/// what became of each key.
pub(crate) struct Applied {
    meshes: [MeshData; 2],
    /// The plan as built: which keys could not be made, and why.
    plan: KeyPlan,
    built: usize,
    failed: usize,
    first_problem: Option<String>,
}

pub struct Keys {
    pub(crate) settings: KeySettings,
    /// Joints are planned as soon as the tool has its two pieces.
    pub(crate) auto_plan: bool,
    pub(crate) view: ViewSide,
    pub(crate) pair: Option<Pair>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    pub(crate) hover: Option<Hit>,
    pub(crate) drag: Option<Dragging>,
    /// The joints as they were when the key in hand was taken hold of, until
    /// it moves and that becomes a step of the history.
    pub(crate) held: Option<Snapshot>,
    /// Pegs as solids: for the half each stands on (A, then B), the ones
    /// that fit and the ones that do not.
    pub(crate) pegs: [[Preview; 2]; 2],
    /// Sockets as shells, for the half each is sunk into (A, then B).
    pub(crate) sockets: [Preview; 2],
    pub(crate) pegs_stale: bool,
    pub(crate) job: Option<Job<Applied>>,
    job_started: f64,
    /// Where the Size and Height sliders stand while their Auto box is
    /// ticked, in millimetres.
    pub(crate) slider_size: f32,
    pub(crate) slider_height: f32,
}

impl Default for Keys {
    fn default() -> Keys {
        Keys {
            settings: KeySettings::default(),
            auto_plan: true,
            view: ViewSide::Pegs,
            pair: None,
            undo: Vec::new(),
            redo: Vec::new(),
            hover: None,
            drag: None,
            held: None,
            pegs: Default::default(),
            sockets: Default::default(),
            pegs_stale: true,
            job: None,
            job_started: 0.0,
            slider_size: 5.0,
            slider_height: 5.0,
        }
    }
}

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn Tool> {
    Box::new(Keys::default())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

impl Keys {
    /// Keys that will be built, on the pair in hand.
    pub fn key_count(&self) -> usize {
        self.pair.as_ref().map_or(0, |pair| pair.plan.key_count())
    }

    /// Keys placed where they do not fit.
    pub fn invalid_count(&self) -> usize {
        self.pair.as_ref().map_or(0, |pair| pair.plan.invalid_count())
    }

    /// The two objects being keyed.
    pub fn objects(&self) -> Option<(ObjectId, ObjectId)> {
        self.pair.as_ref().map(|pair| (pair.a, pair.b))
    }

    pub fn settings(&self) -> &KeySettings {
        &self.settings
    }

    /// The pair, while its joints can be edited: the scene shows the halves
    /// they were planned on, and no build is running.
    pub(crate) fn editable(&self) -> Option<&Pair> {
        self.pair.as_ref().filter(|pair| pair.showing == Showing::Plain && self.job.is_none())
    }

    pub(crate) fn editable_mut(&mut self) -> Option<&mut Pair> {
        let idle = self.job.is_none();
        self.pair.as_mut().filter(|pair| pair.showing == Showing::Plain && idle)
    }

    /// Says something in the Scene panel's status line.
    pub(crate) fn say(&mut self, cx: &mut Ctx, text: impl Into<String>, ok: bool) {
        cx.status(text, ok);
    }

    // ------------------------------------------------------------- the pair

    fn drop_pair(&mut self) {
        self.pair = None;
        self.undo.clear();
        self.redo.clear();
        self.hover = None;
        self.drag = None;
        self.held = None;
        self.pegs_stale = true;
    }

    fn take(&mut self, pair: Pair) {
        self.drop_pair();
        self.pair = Some(pair);
    }

    /// Settles which two pieces the tool works on, as it comes up: the two
    /// selected objects; else the selected object and the one whose cut face
    /// mates with it; else the pair from last time, if it still stands.
    fn open(&mut self, cx: &mut Ctx) {
        self.end_drag();
        self.hover = None;
        self.pegs_stale = true;
        let scene = cx.scene();
        let selected = scene.selected();
        let active = scene.active();

        // The pair from last time, while the scene still shows what it knows.
        let standing = self.pair.as_ref().is_some_and(|pair| match pair.look(scene) {
            Showing::Plain => pair.unsculpted(scene),
            Showing::Keyed => true,
            Showing::Changed => false,
        });
        if !standing {
            self.drop_pair();
        }

        let mut problem = None;
        if let [first, second, ..] = selected[..] {
            if !self.pair.as_ref().is_some_and(|pair| pair.is(first, second)) {
                match Pair::of(scene, first, second, Source::Selected, &self.settings, self.auto_plan) {
                    Ok(pair) => self.take(pair),
                    Err(why) => {
                        self.drop_pair();
                        problem = Some(why);
                    }
                }
            }
        } else if !self.pair.as_ref().is_some_and(|pair| pair.has(active)) {
            // A pair kept from before gives way to the selected object's own,
            // but is not thrown away for an object that has none.
            match Pair::with_mate(scene, active, &self.settings, self.auto_plan) {
                Some(pair) => self.take(pair),
                None if self.pair.is_none() => {
                    let name = scene.info(active).map_or(String::new(), |info| info.name.to_string());
                    problem = Some(format!(
                        "Nothing to key: {name} touches no other object face to face. Split a model in two first, or click the piece that mates with it."
                    ));
                }
                None => {}
            }
        }

        if let Some(pair) = &mut self.pair {
            pair.showing = pair.look(cx.scene());
            // Joints kept from last time take up the settings as they are
            // now; they may have been changed with the tool put away.
            if standing && pair.showing == Showing::Plain {
                for jp in &mut pair.plan.joints {
                    planner::resize(jp, &self.settings);
                }
            }
        }
        match problem {
            Some(why) => self.say(cx, why, false),
            None => self.greet(cx),
        }
    }

    /// Tries the selected object with one the user clicked.
    pub(crate) fn pair_with(&mut self, cx: &mut Ctx, other: ObjectId) -> bool {
        let active = cx.scene().active();
        match Pair::of(cx.scene(), active, other, Source::Clicked, &self.settings, self.auto_plan) {
            Ok(pair) => {
                self.take(pair);
                self.greet(cx);
            }
            Err(why) => self.say(cx, why, false),
        }
        true
    }

    /// Which two pieces were picked and how, in the user's words.
    pub(crate) fn pair_line(&self, cx: &Ctx) -> Option<String> {
        let pair = self.pair.as_ref()?;
        let [a, b] = pair.names(cx.scene());
        let how = match pair.source {
            Source::Selected => "the two selected objects",
            Source::Found => "the selected object and the piece touching it",
            Source::Clicked => "the selected object and the piece you clicked",
        };
        Some(format!("{a} + {b} ({how})"))
    }

    /// What was picked and what can be done with it, as the tool comes up.
    fn greet(&mut self, cx: &mut Ctx) {
        let Some(line) = self.pair_line(cx) else { return };
        let Some(pair) = &self.pair else { return };
        if pair.showing == Showing::Keyed {
            self.say(cx, format!("{line} already have their keys. Undo (Z) takes them off so the joints can be changed."), true);
            return;
        }
        if pair.plan.joints.iter().all(|jp| jp.map.eligible_cells() == 0) {
            self.say(cx, format!("{line}: no flat mating face found between these parts."), false);
            return;
        }
        let (count, unfit, notes) = (self.key_count(), self.invalid_count(), self.notes());
        let unfit = if unfit > 0 { format!(", {unfit} that will not fit") } else { String::new() };
        let notes = if notes.is_empty() { String::new() } else { format!(" {notes}.") };
        self.say(
            cx,
            format!(
                "Keys on {line}: {count} joint{}{unfit}.{notes} Click the cut face to add one, drag to move, right-click to remove, wheel to turn. Enter builds them.",
                plural(count)
            ),
            true,
        );
    }

    /// What the planner had to settle for, across the joints.
    pub(crate) fn notes(&self) -> String {
        let Some(pair) = &self.pair else { return String::new() };
        let joints = &pair.plan.joints;
        let mut notes: Vec<String> = Vec::new();
        for jp in joints {
            let Some(note) = &jp.note else { continue };
            if !notes.contains(note) && (!jp.keys.is_empty() || joints.len() == 1) {
                notes.push(note.clone());
            }
        }
        // A cut through legs and antennae makes joints too small to key; say
        // so once, not per sliver.
        let skipped = joints.iter().filter(|jp| jp.keys.is_empty()).count();
        if joints.len() > 1 && skipped > 0 {
            notes.push(format!("{skipped} of {} cut faces too small for a key", joints.len()));
        }
        notes.join("; ")
    }

    /// "2 Circle keys, 5.50 mm x 5.50 mm deep, pegs on Sphere".
    fn describe(&self, cx: &Ctx, built: usize) -> String {
        let Some(pair) = &self.pair else { return "No keys.".to_string() };
        let keyed = pair.plan.joints.iter().filter(|jp| !jp.keys.is_empty());
        let main = keyed.max_by(|x, y| x.joint.area.total_cmp(&y.joint.area));
        let Some(main) = main else { return "No keys.".to_string() };
        let [a, b] = pair.names(cx.scene());
        format!(
            "{built} {} key{}, {} x {} deep, pegs on {}",
            self.settings.shape.name(),
            plural(built),
            units::format(main.size, LengthUnit::Millimetres, true),
            units::format(main.height, LengthUnit::Millimetres, true),
            if main.peg_on_a { a } else { b }
        )
    }

    /// Looks at what the scene shows of the pair, every frame: notices the
    /// keys being undone and redone, and the pieces going away.
    fn watch(&mut self, cx: &mut Ctx) {
        let Some(pair) = &mut self.pair else { return };
        // A running build is of the halves as they were; `land` judges it.
        if self.job.is_some() {
            return;
        }
        let now = pair.look(cx.scene());
        if now == pair.showing {
            return;
        }
        let before = std::mem::replace(&mut pair.showing, now);
        self.pegs_stale = true;
        match (before, now) {
            // The scaffold comes back as it was planned.
            (Showing::Keyed, Showing::Plain) => {
                let count = self.key_count();
                let (s, how) = (plural(count), if cx.active { "" } else { " J changes them." });
                self.say(cx, format!("Keys taken off. The {count} planned joint{s} are back.{how}"), true);
            }
            (_, Showing::Changed) => {
                self.drop_pair();
                if cx.active {
                    let text = "The pieces these joints were planned on have changed. Select one and press J to start again.";
                    self.say(cx, text, false);
                }
            }
            _ => {}
        }
    }

    // --------------------------------------------------------- the settings

    /// The settings changed: resizes the keys where they stand and re-checks
    /// them.
    pub(crate) fn settings_changed(&mut self) {
        let settings = self.settings;
        if let Some(pair) = self.editable_mut() {
            for jp in &mut pair.plan.joints {
                planner::resize(jp, &settings);
            }
        }
        self.pegs_stale = true;
    }

    // -------------------------------------------------------- joint history

    /// Makes the joints as they are now a step Z can come back to: call it
    /// before changing them.
    pub(crate) fn push_edit(&mut self) {
        if let Some(snapshot) = self.pair.as_ref().map(|pair| pair.snapshot()) {
            self.push_snapshot(snapshot);
        }
    }

    pub(crate) fn push_snapshot(&mut self, snapshot: Snapshot) {
        self.undo.push(snapshot);
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Takes the last step back off the history: for a gesture that is being
    /// abandoned.
    pub(crate) fn pop_edit(&mut self) -> Option<Snapshot> {
        self.undo.pop()
    }

    /// Z or Shift+Z on the joints. False when there is nothing to step, so
    /// the press goes on to the scene's history.
    fn step(&mut self, redo: bool) -> bool {
        if self.drag.is_some() {
            return true;
        }
        let settings = self.settings;
        if self.editable().is_none() {
            return false;
        }
        let (from, to) = if redo { (&mut self.redo, &mut self.undo) } else { (&mut self.undo, &mut self.redo) };
        let (Some(snapshot), Some(pair)) = (from.pop(), self.pair.as_mut()) else { return false };
        to.push(pair.snapshot());
        pair.restore(snapshot, &settings);
        self.pegs_stale = true;
        true
    }

    // ---------------------------------------------------- panel operations

    /// Re-plans every joint from scratch with the current settings.
    pub(crate) fn auto_place(&mut self, cx: &mut Ctx) {
        if self.editable().is_none() {
            return;
        }
        self.push_edit();
        let settings = self.settings;
        if let Some(pair) = &mut self.pair {
            for jp in &mut pair.plan.joints {
                planner::auto_place(jp, &settings);
            }
        }
        self.pegs_stale = true;
        let (count, notes) = (self.key_count(), self.notes());
        let notes = if notes.is_empty() { String::new() } else { format!(" {notes}.") };
        self.say(cx, format!("{count} joint{} placed.{notes}", plural(count)), count > 0);
    }

    pub(crate) fn clear_keys(&mut self, cx: &mut Ctx) {
        if self.editable().is_none() {
            return;
        }
        self.push_edit();
        if let Some(pair) = &mut self.pair {
            pair.plan.joints.iter_mut().for_each(|jp| jp.keys.clear());
        }
        self.pegs_stale = true;
        self.say(cx, "Joints cleared. Click the cut face to add one, or Auto Place.", true);
    }

    /// Swaps which half gets the pegs, on every joint.
    pub(crate) fn flip_sides(&mut self, cx: &mut Ctx) {
        if self.editable().is_none() {
            return;
        }
        self.push_edit();
        let settings = self.settings;
        if let Some(pair) = &mut self.pair {
            for jp in &mut pair.plan.joints {
                jp.peg_on_a = !jp.peg_on_a;
                planner::validate_all(jp, &settings);
            }
        }
        self.pegs_stale = true;
        self.say(cx, "Pegs and sockets swapped sides.", true);
    }

    // ---------------------------------------------------------------- apply

    /// Starts building the joints into both pieces, off the interface thread.
    /// `pause` slows the build down, so the demo has progress to show.
    pub(crate) fn apply(&mut self, cx: &mut Ctx, pause: Duration) {
        if self.job.is_some() {
            return;
        }
        let Some(pair) = self.editable() else {
            self.say(cx, "Nothing to build: the tool has no pair of pieces with joints to add.", false);
            return;
        };
        if pair.plan.key_count() == 0 {
            self.say(cx, "No joints to build. Click the cut face to add one, or Auto Place.", false);
            return;
        }
        // The job cannot borrow from the tool: it gets the plan (whose two
        // halves are shared, not copied) and the masks to carry over.
        let mut plan = pair.plan.clone();
        let masks = pair.masks.clone();
        self.end_drag();
        self.job_started = cx.time;
        self.job = Some(cx.spawn("Add Keys", move |job| {
            job.progress("Building the joints");
            std::thread::sleep(pause);
            let planner::BuildResult { a, b, built, failed, first_problem } = planner::build(&mut plan);
            let problem = || first_problem.clone().unwrap_or_else(|| "unknown problem".to_string());
            let (Some(a), Some(b)) = (a, b) else {
                return Err(format!("Could not build the keys: {}.", problem()));
            };
            if built == 0 {
                return Err(format!("No joint could be built: {}.", problem()));
            }
            let mut baked = Vec::with_capacity(2);
            for (half, (mask, which)) in [a, b].into_iter().zip(masks.iter().zip(["first", "second"])) {
                if job.cancelled() {
                    return Err("Cancelled.".to_string());
                }
                job.progress(format!("Preparing the {which} piece"));
                std::thread::sleep(pause);
                // Baked here, so the commit on the interface thread is
                // instant however large the pieces are.
                let mask = carried_mask(mask, &half);
                baked.push(job.bake(half.mesh, mask));
            }
            if job.cancelled() {
                return Err("Cancelled.".to_string());
            }
            let meshes: [MeshData; 2] = baked.try_into().map_err(|_| "Could not prepare the pieces.".to_string())?;
            Ok(Applied { meshes, plan, built, failed, first_problem })
        }));
    }

    /// A finished build becomes the two pieces' geometry: one undo step.
    fn land(&mut self, cx: &mut Ctx, result: Result<Applied, String>) {
        let applied = match result {
            Ok(applied) => applied,
            Err(why) => return self.say(cx, why, false),
        };
        let Some(pair) = &mut self.pair else { return };
        // The build took a while; the pieces must still be the ones it was
        // made from.
        if pair.look(cx.scene()) != Showing::Plain {
            return self.say(cx, "The pieces changed while their keys were being built. Nothing was changed.", false);
        }
        pair.take_verdicts(&applied.plan);
        let [mesh_a, mesh_b] = applied.meshes;
        let (a, b) = (pair.a, pair.b);
        let both = vec![Command::Replace { object: a, mesh: mesh_a }, Command::Replace { object: b, mesh: mesh_b }];
        let receipt = cx.commit("Add Keys", both);
        let Some(pair) = &mut self.pair else { return };
        pair.applied(cx.scene());
        // The joints are built: their own history has nothing left to step.
        self.undo.clear();
        self.redo.clear();
        self.pegs_stale = true;

        let seconds = (cx.time - self.job_started).max(0.0);
        let mut text = format!("{} ({:.0} ms).", self.describe(cx, applied.built), seconds * 1e3);
        if applied.failed > 0 {
            let why = applied.first_problem.unwrap_or_else(|| "it did not fit".to_string());
            text.push_str(&format!(" {} could not be built: {why}.", applied.failed));
        }
        text.push_str(" Undo (Z) takes the keys off and gives the joints back.");
        self.say(cx, text, applied.failed == 0 && receipt.failed == 0);
        if cx.active {
            cx.finish();
        }
    }
}

/// A half's mask carried onto the same half with keys built in: the vertices
/// it came with keep their value, the keys' own are unmasked. `None` for a
/// half with no mask to carry.
fn carried_mask(mask: &[f32], built: &Built) -> Option<Vec<f32>> {
    if mask.iter().all(|&m| m == 0.0) {
        return None;
    }
    let value = |&source: &u32| match source {
        NEW_VERTEX => 0.0,
        kept => mask.get(kept as usize).copied().unwrap_or(0.0),
    };
    Some(built.source.iter().map(value).collect())
}

impl Tool for Keys {
    fn info(&self) -> Info {
        Info {
            id: "keys",
            name: "Keys",
            icon: "tool_keys",
            tip: "Joints for printing in pieces: pegs on one half of a cut and matching sockets in the other (J).",
            hotkey: Some(Key::J),
        }
    }

    fn enter(&mut self, cx: &mut Ctx) {
        self.open(cx);
    }

    fn leave(&mut self, _cx: &mut Ctx) {
        // The joints stay as planned for next time; only a drag in progress
        // is let go, where it stands.
        self.end_drag();
        self.hover = None;
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::Press(Button::Left) => self.press(cx),
            Event::Press(Button::Right) => self.right_press(cx),
            Event::Drag => {
                self.drag_to(cx);
                true
            }
            Event::Release => {
                if self.end_drag() {
                    self.report_keys(cx);
                }
                true
            }
            Event::Click(Button::Left) => self.click(cx),
            Event::Wheel(notches) => self.wheel(cx, notches),
            Event::KeyDown(key) => self.key(cx, key),
            Event::Undo => self.step(false),
            Event::Redo => self.step(true),
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        // A build started here keeps running if the user switches tool, so
        // its result is collected whether or not this tool is up.
        if let Some(result) = self.job.as_mut().and_then(Job::poll) {
            self.job = None;
            self.land(cx, result);
        }
        self.watch(cx);
        if cx.active {
            self.draw(cx, draw);
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        crate::panel::panel(self, ui, cx);
    }

    fn can_undo(&self, redo: bool) -> bool {
        self.editable().is_some() && !(if redo { &self.redo } else { &self.undo }).is_empty()
    }

    /// Splits the selected object in two and brings the tool up on the
    /// halves with their joints planned: the cut faces the camera, on a model
    /// made big enough for two joints. `what` goes on from there: `move`
    /// leaves a joint in mid-drag, `holes` / `both` change the view, a
    /// shape's name ("hexagon", "star"...) plans with that shape, `job`
    /// leaves a slow build running, and `applied` builds the keys into a
    /// model cut sideways and opens the halves like a book, so the pegs on
    /// one and the sockets in the other both show.
    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        let view = cx.view;
        let (normal, scale, beyond) = match what {
            "applied" => ((view.right - view.forward * 0.3).normalize(), 1.0, 0.0),
            _ => ((-view.forward + view.right * 0.5 + view.up * 0.42).normalize(), 1.5, 0.07),
        };
        let Some(middle) = self.demo_split(cx, normal, scale, beyond) else { return };
        let named = |shape: &KeyShape| shape.name().to_lowercase().replace([' ', '-'], "") == what;
        if let Some(shape) = KeyShape::ALL.into_iter().find(named) {
            self.settings.shape = shape;
        }
        self.open(cx);
        match what {
            "holes" => self.view = ViewSide::Holes,
            "both" => self.view = ViewSide::Both,
            "move" => self.demo_move(cx),
            "job" => self.apply(cx, Duration::from_millis(1500)),
            "applied" => {
                self.apply(cx, Duration::ZERO);
                if let Some(job) = self.job.take() {
                    let result = job.wait();
                    self.land(cx, result);
                }
                self.demo_open(cx, middle);
            }
            _ => {}
        }
    }
}

impl Keys {
    /// Makes the selected object `scale` times its size and cuts it along
    /// the plane with normal `normal`, `beyond` of its size past its middle
    /// (so the half left behind the plane is the thinner, and gets the
    /// pegs). The half the normal points into becomes a new object. Returns
    /// the middle of the whole model.
    fn demo_split(&mut self, cx: &mut Ctx, normal: Vec3, scale: f32, beyond: f32) -> Option<Vec3> {
        let scene = cx.scene();
        let object = scene.active();
        let info = scene.info(object)?;
        let name = info.name.to_string();
        let was = info.placement;
        let placement = Placement { scale: was.scale * scale, ..was };
        let mesh = scene.mesh(object)?.to_tri_mesh();
        let bounds = scene.bounds(object);
        let middle = was.position + (bounds.center() - was.position) * scale;
        let point = middle - normal * bounds.size().max_element() * scale * beyond;
        // The plane in the object's own space.
        let to_world = placement.matrix();
        let local_normal = to_world.transpose().transform_vector3(normal);
        let cut = match split_by_plane(&mesh, to_world.inverse().transform_point3(point), local_normal) {
            Ok(cut) => cut,
            Err(why) => {
                self.say(cx, why, false);
                return None;
            }
        };
        cx.commit(
            "Split",
            vec![
                Command::Place { object, placement },
                Command::Replace { object, mesh: cut.outside.mesh.into() },
                Command::Add(
                    NewObject::new(format!("{name} Piece"), cut.inside.mesh)
                        .placed(placement)
                        .unselected()
                        .with_symmetry_of(object),
                ),
            ],
        );
        Some(middle)
    }

    /// Takes hold of the first joint and carries it part of the way across
    /// the face, button still down.
    fn demo_move(&mut self, cx: &mut Ctx) {
        let Some(pair) = self.editable() else { return };
        let Some((jp, key)) = pair.plan.joints.iter().find_map(|jp| Some((jp, jp.keys.first()?))) else { return };
        let on_a = self.shown_side_is_a(jp);
        let display = pair.display(cx.scene(), on_a);
        // Across the face's length, so it does not run into another joint.
        let along = glam::Vec2::Y * jp.size * 0.6;
        let world = |plane| display.transform_point3(jp.joint.from_plane(plane, 0.0));
        let (from, to) = (cx.view.project(world(key.position)), cx.view.project(world(key.position + along)));
        let (Some(from), Some(to)) = (from, to) else { return };
        if self.press_at(cx, from) {
            for step in 1..=4 {
                self.drag_at(cx, from.lerp(to, step as f32 / 4.0));
            }
        }
    }

    /// Opens the two keyed halves of a model cut sideways like a book: each
    /// moved half the model's width to its own side and turned about the
    /// upright until its cut face looks toward the camera.
    fn demo_open(&mut self, cx: &mut Ctx, middle: Vec3) {
        let Some((a, b)) = self.objects() else { return };
        let scene = cx.scene();
        let (Some(info_a), Some(info_b)) = (scene.info(a), scene.info(b)) else { return };
        let width = Aabb::union(scene.bounds(a), scene.bounds(b)).size().max_element();
        let view = cx.view;
        // About the middle of the whole model, so the cut faces swing open.
        let opened = |was: Placement, degrees: f32, side: f32| {
            let turn = Quat::from_axis_angle(view.up, degrees.to_radians());
            Placement {
                position: middle + view.right * (side * width * 0.42) + turn * (was.position - middle),
                rotation: turn * was.rotation,
                ..was
            }
        };
        let (placement_a, placement_b) = (opened(info_a.placement, -30.0, -1.0), opened(info_b.placement, 60.0, 1.0));
        let open = vec![
            Command::Place { object: a, placement: placement_a },
            Command::Place { object: b, placement: placement_b },
        ];
        cx.commit("Open", open);
    }
}
