//! The trait a tool implements, and the input it is sent.

use crate::ctx::Ctx;
use crate::draw::Draw;
use crate::saved::{Arrival, Saved};
use crate::scene::{ObjectId, Placement};

/// Keyboard keys, as egui names them (`Key::A`, `Key::Num1`, `Key::Escape`,
/// `Key::Delete`, `Key::OpenBracket`...).
pub use egui::Key;

/// What the app needs to know to list a tool and bring it up.
#[derive(Clone, Copy, Debug)]
pub struct Info {
    /// Lower-case, no spaces, never changed: what `--demo-tool` and saved
    /// settings call the tool.
    pub id: &'static str,
    /// On its button.
    pub name: &'static str,
    /// An icon in `assets/icons`, without `.png`.
    pub icon: &'static str,
    /// The button's tooltip: what the tool is for, in a sentence.
    pub tip: &'static str,
    /// A key that brings the tool up from Sculpt, Transform or another
    /// hosted tool, and puts it away again. The Unity app's: T Tube, J Keys,
    /// K Split, L Measure, Y Array. Keys the app itself uses are ignored
    /// (see the README).
    pub hotkey: Option<Key>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    /// The left button, or the pen tip.
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Something the user did while the tool is up. Where the pointer is and
/// which modifiers are held is in the [`Ctx`] that comes with it.
///
/// More kinds may be added; end a `match` on it with `_ => false`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// A button went down on the viewport (never on a panel, and never with
    /// Alt, which is the camera's). Return true to take the drag: the tool
    /// then gets `Drag` for every move and `Release` at the end. Return false
    /// to pass: a left press then orbits the camera, as a drag on empty
    /// background does everywhere in the app.
    Press(Button),
    /// The pointer moved during a drag the tool took. Sent for every pointer
    /// sample, so a drawn stroke loses none of its path.
    Drag,
    /// The drag the tool took has ended.
    Release,
    /// A press the tool passed on was let go without moving: a click on
    /// something the tool did not claim. Return true if it meant something.
    Click(Button),
    /// The wheel turned over the viewport, in notches (positive away from
    /// the user). Return true if the tool used it; otherwise it zooms.
    Wheel(f32),
    /// A key went down (no Ctrl; those are the app's). Return true if it was
    /// the tool's. Esc that is not taken puts the tool away.
    KeyDown(Key),
    /// A key came up. Sent whether or not the tool took the press, so a
    /// held-key gesture can never stick.
    KeyUp(Key),
    /// Z. Return true if the tool stepped its own history back; false sends
    /// the press on to the scene's history.
    Undo,
    /// Shift+Z, likewise.
    Redo,
    /// Tile `index` of [`Tool::radial`] was picked on the hold-Space wheel.
    Radial(usize),
    /// The user is dragging a handle of the transform gizmo the tool asked
    /// for with [`Tool::gizmo`]: where its target belongs now. The placement
    /// is always worked out from where the target was when the drag began,
    /// never added to the last one, so put the target exactly there. The
    /// returned flag is ignored.
    Gizmo(Placement, GizmoPhase),
}

/// Where in a gizmo drag an [`Event::Gizmo`] falls. A tool with its own
/// history records one step per drag, at `End`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoPhase {
    /// A handle was pressed. Nothing has moved yet: the placement is the one
    /// the tool offered. Keep it (or your own state) for the undo step.
    Begin,
    /// The pointer moved.
    Move,
    /// The handle was let go, or the tool was put away mid-drag. Always
    /// sent after a `Begin`, with the placement of the last `Move`; a press
    /// that never moved ends where it began, which is not an edit.
    End,
}

/// Something of the tool's own that the app's transform gizmo (arrows, rings
/// and cubes) should sit on: a control point of an array's path, one of its
/// copies, an SSphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoTarget {
    /// Where the target is, in world space. The gizmo sits at its position,
    /// turned to its rotation.
    pub placement: Placement,
    /// Roughly how big the target is, so the handles clear it. A press
    /// inside this distance of the target's position (never less than a
    /// fingertip on screen) is not the gizmo's: it comes to the tool as an
    /// ordinary `Press`, so the target itself can still be grabbed and
    /// dragged freely. The white cube in the middle of a target that scales
    /// is the one handle in there.
    pub radius: f32,
    /// Which handles to offer.
    pub moves: bool,
    pub turns: bool,
    pub scales: bool,
}

impl GizmoTarget {
    /// A point that can only be moved.
    pub fn point(position: glam::Vec3) -> GizmoTarget {
        GizmoTarget { placement: Placement::at(position), radius: 0.0, moves: true, turns: false, scales: false }
    }

    /// Something that can be moved, turned and resized.
    pub fn body(placement: Placement, radius: f32) -> GizmoTarget {
        GizmoTarget { placement, radius, moves: true, turns: true, scales: true }
    }
}

/// One tile of the hold-Space wheel while the tool is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadialItem {
    pub label: &'static str,
    /// An icon in `assets/icons`, without `.png`.
    pub icon: &'static str,
    /// Drawn lit: the current mode, a setting that is on.
    pub on: bool,
}

/// An interactive tool, hosted by the app.
///
/// Only `info` must be written; everything else does nothing by default.
/// Every method gets a [`Ctx`]: the camera, the pointer, the scene to read,
/// and the ways to change things.
pub trait Tool {
    fn info(&self) -> Info;

    /// The tool was brought up. Pick up where the scene is now.
    fn enter(&mut self, _cx: &mut Ctx) {}

    /// The tool is being put away (another tool, Esc, or `Ctx::finish`). End
    /// any gesture in progress; keep what the user would expect to find
    /// again next time.
    fn leave(&mut self, _cx: &mut Ctx) {}

    /// The user did something while the tool is up. The meaning of the
    /// returned flag depends on the event; see [`Event`].
    fn input(&mut self, _cx: &mut Ctx, _event: Event) -> bool {
        false
    }

    /// Once a frame, before anything is drawn, for every tool whether or
    /// not it is up (`cx.active` says). Follow the pointer, collect finished
    /// jobs, notice changes to the scene, and say what to draw. Not called
    /// while a brush stroke is in progress.
    fn frame(&mut self, _cx: &mut Ctx, _draw: &mut Draw) {}

    /// The tool's controls, in the left panel in place of the brush
    /// settings while the tool is up. Build them from `theme` widgets.
    fn panel(&mut self, _ui: &mut egui::Ui, _cx: &mut Ctx) {}

    /// A card in the Scene panel that is there whichever tool is up, for the
    /// few tools with something worth having to hand all the time (the
    /// object's real size, a live array's settings). Draw a
    /// `theme::category` or nothing.
    fn card(&mut self, _ui: &mut egui::Ui, _cx: &mut Ctx) {}

    /// The tiles of the hold-Space wheel while the tool is up. None means
    /// Space does nothing.
    fn radial(&self) -> Vec<RadialItem> {
        Vec::new()
    }

    /// Whether Z (or with `redo`, Shift+Z) would step the tool's own
    /// history, for the Undo and Redo buttons.
    fn can_undo(&self, _redo: bool) -> bool {
        false
    }

    /// Something of the tool's own to put the transform gizmo on while the
    /// tool is up: asked every frame, so answer from the tool's state as it
    /// is now (`None` when nothing is selected). A press on a handle is the
    /// gizmo's and never reaches the tool as a `Press`; the drag comes back
    /// as [`Event::Gizmo`] with the target's new placement, from `Begin` to
    /// `End`. The gizmo is not drawn in the hide-the-panels view.
    fn gizmo(&self) -> Option<GizmoTarget> {
        None
    }

    /// What the tool holds for `object` that belongs in the scene's file, if
    /// anything (an array's settings and handle points). Asked of every tool
    /// for every object whenever the scene is saved. The app writes it into
    /// the object's entry under [`Info::id`], so the same name is how a file
    /// from a later version, which has more than this one knows, is read:
    /// leave out what the file does not need, and read what is there leniently.
    ///
    /// The object is named by its id here, and by its place in the object list
    /// in the file; the app keeps the two together, so ids need not be stable
    /// across a save. Keep to what is the object's own: where it stands in the
    /// scene is saved with the object, not with the tool.
    fn save_object(&self, _object: ObjectId) -> Option<Saved> {
        None
    }

    /// Objects came into the scene from a file, with what this tool saved for
    /// them. Called on every tool each time a file comes in (`objects` is
    /// empty when the file has nothing of the tool's), after the objects are
    /// in the scene and have their ids. Treat `objects` as you would a stranger's
    /// input: a damaged value is skipped, never a reason to fail.
    ///
    /// `Arrival::Replaced` means the objects took the place of the whole scene,
    /// which Undo brings back as it was, so keep what you hold for the objects
    /// that were there (ids are never reused) but end any history of your own
    /// that Z would otherwise answer before the scene's. `Arrival::Added` leaves
    /// everything as it is.
    fn load_objects(&mut self, _objects: Vec<(ObjectId, Saved)>, _arrival: Arrival) {}

    /// Puts the tool in a state worth a screenshot: `--demo-tool <id>` brings
    /// the tool up and calls this with whatever followed `<id>:` (empty if
    /// nothing did). No person is at the tablet, so do it through the same
    /// code the gestures run.
    fn demo(&mut self, _cx: &mut Ctx, _what: &str) {}
}
