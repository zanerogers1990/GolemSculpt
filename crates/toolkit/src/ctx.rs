//! What a tool is handed with every call: the camera and pointer, the scene
//! to read, and the ways to change things.

use std::ops::Deref;

use glam::{Vec2, Vec3};
use sculpt_geom::{Aabb, LengthUnit, SymmetryGroup};

use crate::jobs::{Job, JobCtx, Jobs};
use crate::scene::{Command, Document, Hit, ObjectId, Placement, Receipt, Scene, Symmetry, apply};
use crate::tool::Mods;
use crate::view::View;

/// The plain facts of the moment. A [`Ctx`] reads as one of these: write
/// `cx.view`, `cx.pointer`, `cx.mods.shift`.
#[derive(Clone, Copy, Debug)]
pub struct Env {
    /// The camera, in world space.
    pub view: View,
    /// Where the pointer is, in physical pixels from the viewport's top-left
    /// corner. During `Event::Drag` it is that sample's position.
    pub pointer: Vec2,
    /// Whether the pointer is over the bare viewport and free: inside the
    /// window, not on a panel, not navigating the camera. Hover highlights
    /// and ghosts should show only while this is true.
    pub hovering: bool,
    /// Pen pressure, 0 to 1, when a pen is down.
    pub pressure: Option<f32>,
    pub mods: Mods,
    /// Seconds since the app started.
    pub time: f64,
    /// Seconds since the last frame.
    pub dt: f32,
    /// Whether this tool is the one that is up.
    pub active: bool,
    /// The selected object's symmetry, which is the one brushes are working
    /// with (every object has its own: `Scene::symmetry`). First the mirror
    /// axes: X, Y, Z.
    pub mirror: [bool; 3],
    /// Radial symmetry, on top of the mirror axes: how many times brushes
    /// repeat a stroke round `radial_axis` (1 when it is off), a direction
    /// through the origin of the space the symmetry sits in.
    /// [`Env::symmetry`] makes the one list of copies out of all of it.
    pub radial_count: u32,
    pub radial_axis: Vec3,
    /// `None` while that symmetry is the object's own, through its origin
    /// along its axes. `Some` when it is fixed in the world instead: where
    /// the selected object stands (see `Symmetry::world`).
    pub symmetry_world: Option<Placement>,
    /// The unit lengths are shown and typed in (see `sculpt_geom::units`).
    pub unit: LengthUnit,
    /// Whether tooltips are switched on. `theme::tip` takes this.
    pub tips: bool,
    /// Hide-the-panels view is on: draw no handles or labels.
    pub clean_view: bool,
}

impl Env {
    /// A still moment looking through `view`, for tests.
    pub fn at_rest(view: View) -> Env {
        Env {
            view,
            pointer: view.size * 0.5,
            hovering: true,
            pressure: None,
            mods: Mods::default(),
            time: 0.0,
            dt: 1.0 / 60.0,
            active: true,
            mirror: [false; 3],
            radial_count: 1,
            radial_axis: Vec3::Y,
            symmetry_world: None,
            unit: LengthUnit::default(),
            tips: true,
            clean_view: false,
        }
    }

    /// Every copy a brush stroke on the selected object is made as, the
    /// stroke itself first: the mirror images, the turns round the radial
    /// axis, and the two combined. What a tool that follows the brushes'
    /// symmetry repeats its own work under. The copies are in the selected
    /// object's own space, whether the symmetry is its own or the world's.
    pub fn symmetry(&self) -> SymmetryGroup {
        self.symmetry_settings().group()
    }

    /// The selected object's symmetry as one value: what `Scene::symmetry`
    /// says of that object.
    pub fn symmetry_settings(&self) -> Symmetry {
        Symmetry {
            mirror: self.mirror,
            radial_count: self.radial_count,
            radial_axis: self.radial_axis,
            world: self.symmetry_world,
        }
    }

    /// For the host: shows the tool this symmetry as the selected object's.
    pub fn set_symmetry(&mut self, symmetry: Symmetry) {
        (self.mirror, self.radial_count, self.radial_axis) = (symmetry.mirror, symmetry.radial_count, symmetry.radial_axis);
        self.symmetry_world = symmetry.world;
    }
}

/// A change to the selected object's radial symmetry that a tool asks for
/// (see [`Ctx::set_radial`]). What is left `None` stays as the Symmetry card
/// has it, so a tool that only wants it on, round an axis, does not have to
/// know the repeat count that is kept while it is off.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RadialChange {
    /// Switch radial symmetry on or off.
    pub on: Option<bool>,
    /// How many times a stroke is made, itself included (2 to 32).
    pub count: Option<u32>,
    /// The axis it goes round, a direction through the origin of the space
    /// the object's symmetry sits in (its own, unless it is fixed in the
    /// world).
    pub axis: Option<Vec3>,
}

/// Requests a tool made that the app carries out when the call returns.
/// Tool crates do not touch this; it is how the host reads them back.
#[derive(Debug, Default)]
pub struct Outbox {
    /// A line for the status area of the Scene panel, and whether it is good
    /// news.
    pub status: Option<(String, bool)>,
    /// The big fading word over the viewport.
    pub toast: Option<String>,
    /// Bring the tool up.
    pub start: bool,
    /// Put the tool away.
    pub finish: bool,
    /// Glide the camera to fit this box.
    pub frame: Option<Aabb>,
    pub unit: Option<LengthUnit>,
    /// New mirror axes, and a change to the radial symmetry, for the object
    /// `symmetry_of` names.
    pub mirror: Option<[bool; 3]>,
    pub radial: Option<RadialChange>,
    /// The object whose symmetry `mirror` and `radial` change: the one that
    /// was selected when the tool asked.
    pub symmetry_of: Option<ObjectId>,
    /// Draw another frame straight after this one.
    pub repaint: bool,
    /// A commit changed the scene.
    pub changed: bool,
}

/// A tool's view of the app for the length of one call.
pub struct Ctx<'a> {
    env: Env,
    /// The object the symmetry in `env` is of: the one that was selected
    /// when the call began, until the tool changes another's.
    shown: ObjectId,
    doc: &'a mut dyn Document,
    jobs: &'a Jobs,
    icons: &'a dyn Fn(&str) -> Option<egui::TextureId>,
    out: &'a mut Outbox,
}

impl Deref for Ctx<'_> {
    type Target = Env;

    fn deref(&self) -> &Env {
        &self.env
    }
}

impl<'a> Ctx<'a> {
    /// For the host: a context over its document.
    pub fn new(
        env: Env,
        doc: &'a mut dyn Document,
        jobs: &'a Jobs,
        icons: &'a dyn Fn(&str) -> Option<egui::TextureId>,
        out: &'a mut Outbox,
    ) -> Ctx<'a> {
        Ctx { env, shown: doc.active(), doc, jobs, icons, out }
    }

    /// The scene, for reading.
    pub fn scene(&self) -> &dyn Scene {
        &*self.doc
    }

    /// The symmetry `object` is sculpted under: its own (every object has
    /// one), with whatever this call has already asked to change in it,
    /// which `Scene::symmetry` only has once the call returns.
    pub fn symmetry_of(&self, object: ObjectId) -> Symmetry {
        if object == self.shown { self.env.symmetry_settings() } else { self.doc.symmetry(object) }
    }

    /// Before a change to the selected object's symmetry: if the tool has
    /// selected another object since the call began (one it has just made,
    /// say), that object's symmetry is the one to show and to change.
    fn aim_symmetry(&mut self) {
        let selected = self.doc.active();
        if selected != self.shown {
            self.env.set_symmetry(self.doc.symmetry(selected));
            self.shown = selected;
            // One object's symmetry is changed per call: the last one asked for.
            (self.out.mirror, self.out.radial) = (None, None);
        }
        self.out.symmetry_of = Some(selected);
    }

    /// The nearest visible surface under a pixel.
    pub fn pick(&self, px: Vec2) -> Option<Hit> {
        self.doc.raycast(&self.env.view.ray(px))
    }

    /// Changes the scene, as one undo step the user sees as `label`.
    ///
    /// The commands run in order, now; the scene read after this call is the
    /// changed one, and the receipt holds the ids of what was added. Adding,
    /// replacing, placing, showing, hiding, renaming and deleting, however
    /// many of each, come back with a single Z. `Command::Mask` is the
    /// exception: each is a step of its own, after the rest.
    pub fn commit(&mut self, label: &str, commands: Vec<Command>) -> Receipt {
        let receipt = apply(&mut *self.doc, label, commands);
        self.out.changed |= receipt.changed;
        self.out.repaint = true;
        receipt
    }

    /// Runs `work` on a background thread and returns the handle to its
    /// result; see [`Job`]. `label` names it to the user while it runs.
    pub fn spawn<T, F>(&mut self, label: &str, work: F) -> Job<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobCtx) -> Result<T, String> + Send + 'static,
    {
        self.out.repaint = true;
        self.jobs.spawn(label, work)
    }

    /// Puts a line in the status area of the Scene panel: what just happened
    /// and what to do next, or (`ok` false) what went wrong.
    pub fn status(&mut self, text: impl Into<String>, ok: bool) {
        self.out.status = Some((text.into(), ok));
    }

    /// Flashes a word or two over the viewport, as every tool switch and
    /// hotkey does.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.out.toast = Some(text.into());
    }

    /// Brings this tool up, once the call returns: for a button on the
    /// tool's card, which is there while other tools are up.
    pub fn start(&mut self) {
        self.out.start = true;
        self.out.finish = false;
    }

    /// Puts this tool away and goes back to sculpting, once the call
    /// returns.
    pub fn finish(&mut self) {
        self.out.finish = true;
        self.out.start = false;
    }

    /// Glides the camera to fit a world-space box.
    pub fn frame_view(&mut self, bounds: Aabb) {
        self.out.frame = Some(bounds);
    }

    /// Changes the unit lengths are shown in, for the whole app.
    pub fn set_unit(&mut self, unit: LengthUnit) {
        self.env.unit = unit;
        self.out.unit = Some(unit);
    }

    /// Switches the selected object's mirror axes, for a tool that follows
    /// them. Like the Symmetry card's ticks, it is not an undo step.
    ///
    /// Symmetry belongs to objects, so this changes the object that is
    /// selected at the moment of asking: after a commit that added and
    /// selected an object it is that new object's symmetry that is set, and
    /// the one selected before keeps its own. `cx.mirror` (and the rest of
    /// the symmetry a `Ctx` reads as) is that object's from then on. The
    /// same holds for [`Ctx::set_radial`].
    pub fn set_mirror(&mut self, mirror: [bool; 3]) {
        self.aim_symmetry();
        self.env.mirror = mirror;
        self.out.mirror = Some(mirror);
    }

    /// Switches the selected object's radial symmetry, for a tool that sets
    /// it to suit what it made (the Lathe, round the axis it turned its
    /// object about, right after the commit that made and selected it).
    /// `cx.radial_count` and `cx.radial_axis` follow at once as far as the
    /// change says what they are; the app has the rest from the next call.
    pub fn set_radial(&mut self, change: RadialChange) {
        self.aim_symmetry();
        if change.on == Some(false) {
            self.env.radial_count = 1;
        } else if let Some(count) = change.count.filter(|_| change.on == Some(true) || self.env.radial_count > 1) {
            self.env.radial_count = count.clamp(SymmetryGroup::MIN_RADIAL_COUNT, SymmetryGroup::MAX_RADIAL_COUNT);
        }
        if let Some(axis) = change.axis.filter(|axis| axis.length_squared() > 1e-12) {
            self.env.radial_axis = axis.normalize();
        }
        self.out.radial = Some(change);
    }

    /// Asks for another frame after this one. The app only draws when
    /// something changes; a tool that animates or is waiting on something of
    /// its own calls this each frame until it is done. (Running jobs keep
    /// frames coming by themselves.)
    pub fn repaint(&mut self) {
        self.out.repaint = true;
    }

    /// An icon from `assets/icons` by name, for `theme::ButtonStyle::icon`
    /// and `theme::tile`.
    pub fn icon(&self, name: &str) -> Option<egui::TextureId> {
        (self.icons)(name)
    }
}
