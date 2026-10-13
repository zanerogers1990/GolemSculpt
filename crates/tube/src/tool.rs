//! The tool's state: the live tubes, the settings the panel edits, the
//! tool's own undo history, and what is drawn each frame.
//!
//! Gestures are in `input`, Merge and Keep Separate in `finish`, the panel in
//! `panel`.

use egui::Color32;
use glam::Vec2;
use sculpt_toolkit::{Ctx, Draw, Shape, Style, View};

use crate::finish::Finishing;
use crate::input::{Drag, Pointer};
use crate::live::{Frame, LiveTube, Symmetry, TubeState};
use crate::types::{CapStyle, Kind, TubeSettings};

const MAX_UNDO_STEPS: usize = 128;
/// Streamed edits (a slider, the wheel) closer together than this are one
/// undo step.
pub(crate) const COALESCE_SECONDS: f64 = 0.4;

const CLAY: Color32 = Color32::from_rgb(184, 184, 189);
const CURVE: Color32 = Color32::from_rgba_premultiplied(242, 155, 53, 242);
const OTHER_CURVE: Color32 = Color32::from_rgba_premultiplied(126, 126, 133, 140);
const COPY_CURVE: Color32 = Color32::from_rgba_premultiplied(77, 49, 17, 77);
const HANDLE: Color32 = Color32::from_rgb(247, 247, 247);
const HANDLE_OUTLINE: Color32 = Color32::from_rgba_premultiplied(23, 23, 28, 230);
const SHARP: Color32 = Color32::from_rgb(5, 5, 8);
const SHARP_OUTLINE: Color32 = Color32::from_rgb(247, 247, 247);
const SELECTED: Color32 = Color32::from_rgb(255, 163, 56);
const DIM_HANDLE: Color32 = Color32::from_rgba_premultiplied(111, 111, 111, 115);
const INSERT: Color32 = Color32::from_rgb(128, 242, 140);
const STROKE: Color32 = Color32::from_rgba_premultiplied(230, 207, 80, 230);
const CURSOR: Color32 = Color32::from_rgba_premultiplied(140, 140, 140, 140);
const SIZE_RING: Color32 = Color32::from_rgba_premultiplied(204, 130, 45, 204);

/// The tubes as they were before an edit.
pub(crate) struct Snapshot {
    tubes: Vec<TubeState>,
    active: Option<usize>,
    pub label: &'static str,
}

/// An undo step that has been opened but not yet pushed: it is pushed only
/// if something changed.
struct PendingEdit {
    before: Snapshot,
    version: u64,
    /// When a streamed edit closes by itself; `None` for an edit a gesture
    /// closes.
    commit_at: Option<f64>,
}

pub struct TubeTool {
    pub(crate) tubes: Vec<LiveTube>,
    pub(crate) active: Option<usize>,
    pub(crate) selected_point: Option<usize>,
    /// What the next stroke starts from: the last settings the panel showed.
    pub(crate) defaults: TubeSettings,
    defaults_sized: bool,
    next_id: u64,
    /// `Ctx::time` of the call in progress.
    pub(crate) now: f64,

    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    pending: Option<PendingEdit>,
    /// Bumped by every edit to the tubes; what an open undo step compares.
    edit_version: u64,

    pub(crate) pointer: Pointer,
    pub(crate) finish: Finishing,

    /// A scripted picture is being taken of the shapes alone: no handles.
    pub(crate) bare: bool,
    /// The last message, for the panel.
    pub(crate) status: String,
    /// A message not yet passed to the app's status line.
    outgoing: Option<(String, bool)>,
    /// The scene has just been handed an undo or a redo this tool did not take: the models may have
    /// changed shape under the muscles.
    pub(crate) scene_stepped: bool,
}

impl Default for TubeTool {
    fn default() -> TubeTool {
        TubeTool {
            tubes: Vec::new(),
            active: None,
            selected_point: None,
            defaults: TubeSettings::default(),
            defaults_sized: false,
            next_id: 1,
            now: 0.0,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: None,
            edit_version: 0,
            pointer: Pointer::default(),
            finish: Finishing::default(),
            bare: false,
            status: String::new(),
            outgoing: None,
            scene_stepped: false,
        }
    }
}

fn approximately(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

/// How many pixels a world-space length at `world` spans on screen.
pub(crate) fn pixels_for(view: &View, world: glam::Vec3, length: f32) -> f32 {
    length / view.world_per_pixel(view.depth_of(world)).max(1e-12)
}

fn circle(draw: &mut Draw, centre: Vec2, radius: f32, width: f32, color: Color32, dash: Option<(f32, f32)>) {
    if !(radius > 0.5 && radius.is_finite()) {
        return;
    }
    let segments = ((radius * 0.5) as usize).clamp(24, 96);
    let points = (0..segments)
        .map(|i| {
            let angle = i as f32 / segments as f32 * std::f32::consts::TAU;
            centre + Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect();
    draw.overlay.shapes.push(Shape::Line { points, width, color, closed: true, dash });
}

impl TubeTool {
    /// The Muscle tool: the same tool, drawing muscles. Its shapes are its own; the Tube tool's are
    /// the Tube tool's.
    pub fn muscle() -> TubeTool {
        TubeTool { defaults: TubeSettings::default().muscle(), ..TubeTool::default() }
    }

    /// What this tool draws.
    pub fn kind(&self) -> Kind {
        self.defaults.kind
    }

    // ---- reading -------------------------------------------------------------

    pub fn tube_count(&self) -> usize {
        self.tubes.len()
    }

    /// Live tubes drawn on an object: the ones Merge can take.
    pub fn mergeable_count(&self) -> usize {
        self.tubes.iter().filter(|t| t.state.anchor.is_some()).count()
    }

    /// Live tubes that `loss` would cost. Closing the app costs them all. A
    /// scene opened in place of this one costs the tubes drawn on an object:
    /// an object that is gone takes its tubes with it (`refresh`). A tube
    /// drawn on nothing stands in the world and is still there afterwards.
    pub fn unmerged(&self, loss: sculpt_toolkit::Loss) -> usize {
        match loss {
            sculpt_toolkit::Loss::Closing => self.tube_count(),
            sculpt_toolkit::Loss::SceneReplaced => self.mergeable_count(),
        }
    }

    pub fn point_count(&self, tube: usize) -> usize {
        self.tubes.get(tube).map_or(0, |t| t.state.points.len())
    }

    /// How many symmetric copies tube `tube` was last built as.
    pub fn copy_count(&self, tube: usize) -> usize {
        self.tubes.get(tube).map_or(0, |t| t.copies.len())
    }

    /// The points of tube `tube`, in its anchor's own space.
    pub fn points(&self, tube: usize) -> &[crate::TubePoint] {
        self.tubes.get(tube).map_or(&[], |t| &t.state.points)
    }

    pub fn active_tube(&self) -> Option<usize> {
        self.active.filter(|&i| i < self.tubes.len())
    }

    pub fn selected_point(&self) -> Option<usize> {
        let tube = self.active_tube()?;
        self.selected_point.filter(|&p| p < self.tubes[tube].state.points.len())
    }

    pub fn selected_is_sharp(&self) -> bool {
        self.active_tube().zip(self.selected_point()).is_some_and(|(t, p)| self.tubes[t].state.points[p].sharp)
    }

    /// Where the handle of a point is in the world (the lifted centre line),
    /// for copy `copy` of its tube as last built.
    pub fn handle_world(&self, tube: usize, point: usize, copy: usize) -> Option<glam::Vec3> {
        self.tubes.get(tube)?.copies.get(copy)?.handle_world.get(point).copied()
    }

    /// Whether a merge is running in the background.
    pub fn is_merging(&self) -> bool {
        self.finish.is_busy()
    }

    // ---- messages ------------------------------------------------------------

    pub(crate) fn say(&mut self, text: impl Into<String>) {
        self.tell(text, true);
    }

    pub(crate) fn tell(&mut self, text: impl Into<String>, ok: bool) {
        let text = text.into();
        self.status.clone_from(&text);
        self.outgoing = Some((text, ok));
    }

    /// Passes the last message on to the app's status line.
    pub(crate) fn flush(&mut self, cx: &mut Ctx) {
        if let Some((text, ok)) = self.outgoing.take() {
            cx.status(text, ok);
        }
    }

    // ---- settings ------------------------------------------------------------
    //
    // What the panel edits: the selected tube's settings when a tube is
    // selected (each change one coalesced undo step), and always the defaults
    // the next stroke starts from, so a tube that was just tuned is what the
    // next one looks like.

    pub fn settings(&self) -> TubeSettings {
        self.active_tube().map_or(self.defaults, |i| self.tubes[i].state.settings)
    }

    fn edit_settings(&mut self, label: &'static str, change: impl Fn(&mut TubeSettings)) {
        change(&mut self.defaults);
        let Some(active) = self.active_tube() else { return };
        self.begin_edit(label, COALESCE_SECONDS);
        change(&mut self.tubes[active].state.settings);
        self.touch();
    }

    pub fn set_radius(&mut self, value: f32) {
        let v = value.clamp(TubeSettings::MIN_RADIUS, TubeSettings::MAX_RADIUS);
        if !approximately(v, self.settings().radius) {
            self.edit_settings("Radius", |s| s.radius = v);
        }
    }

    pub fn set_taper(&mut self, value: f32) {
        let v = value.clamp(0.0, 1.0);
        if !approximately(v, self.settings().taper) {
            self.edit_settings("Taper", |s| s.taper = v);
        }
    }

    pub fn set_flip_taper(&mut self, value: bool) {
        if value != self.settings().flip_taper {
            self.edit_settings("Flip Taper", |s| s.flip_taper = value);
            self.commit_edit();
        }
    }

    pub fn set_caps(&mut self, value: CapStyle) {
        if value != self.settings().caps {
            self.edit_settings("Caps", |s| s.caps = value);
            self.commit_edit();
        }
    }

    pub fn set_smoothness(&mut self, value: f32) {
        let v = value.clamp(0.0, 1.0);
        if !approximately(v, self.settings().smoothness) {
            self.edit_settings("Smoothness", |s| s.smoothness = v);
        }
    }

    pub fn set_swell(&mut self, value: f32) {
        let v = value.clamp(0.0, 1.0);
        if !approximately(v, self.settings().swell) {
            self.edit_settings("Swell", |s| s.swell = v);
        }
    }

    pub fn set_belly(&mut self, value: f32) {
        let v = value.clamp(0.1, 0.9);
        if !approximately(v, self.settings().belly) {
            self.edit_settings("Belly", |s| s.belly = v);
        }
    }

    pub fn set_flatten(&mut self, value: f32) {
        let v = value.clamp(TubeSettings::MIN_FLATTEN, 1.0);
        if !approximately(v, self.settings().flatten) {
            self.edit_settings("Thickness", |s| s.flatten = v);
        }
    }

    pub fn set_depth(&mut self, value: f32) {
        let v = value.clamp(0.0, TubeSettings::MAX_DEPTH);
        if !approximately(v, self.settings().depth) {
            self.edit_settings("Depth", |s| s.depth = v);
        }
    }

    /// Fillet size where a merged tube meets the model, as a multiple of the
    /// tube's radius.
    pub fn set_blend(&mut self, value: f32) {
        let v = value.clamp(0.0, TubeSettings::MAX_BLEND);
        if !approximately(v, self.settings().blend) {
            self.edit_settings("Blend", |s| s.blend = v);
        }
    }

    /// Placement for new strokes and for dragged points: on the surface of
    /// the model, or floating on a plane facing the view. A tool setting
    /// rather than a per-tube one: each point remembers how it was placed.
    pub fn on_surface(&self) -> bool {
        self.defaults.on_surface
    }

    pub fn set_on_surface(&mut self, value: bool) {
        self.defaults.on_surface = value;
    }

    /// The selected point's thickness, as a multiple of the tube's radius.
    pub fn selected_point_size(&self) -> f32 {
        self.active_tube().zip(self.selected_point()).map_or(1.0, |(t, p)| self.tubes[t].state.points[p].size)
    }

    pub fn set_selected_point_size(&mut self, value: f32) {
        let Some((tube, point)) = self.active_tube().zip(self.selected_point()) else { return };
        let v = value.clamp(TubeSettings::MIN_SIZE, TubeSettings::MAX_SIZE);
        if approximately(v, self.tubes[tube].state.points[point].size) {
            return;
        }
        self.begin_edit("Point Size", COALESCE_SECONDS);
        self.tubes[tube].state.points[point].size = v;
        self.touch();
    }

    pub(crate) fn touch(&mut self) {
        self.edit_version += 1;
    }

    // ---- lifecycle -----------------------------------------------------------

    pub(crate) fn entered(&mut self, cx: &mut Ctx) {
        self.now = cx.time;
        if !self.defaults_sized {
            self.size_defaults_to_scene(cx);
        }
        // Whatever was done to the models while the tool was away, the muscles lie on them as they
        // are now.
        self.tubes.iter_mut().for_each(LiveTube::lay_again);
        if self.tubes.is_empty() {
            self.say(self.invitation());
        }
        self.flush(cx);
    }

    pub(crate) fn left(&mut self, cx: &mut Ctx) {
        self.now = cx.time;
        self.cancel_gesture();
        self.commit_edit();
        self.pointer.clear_hover();
        self.flush(cx);
    }

    /// What to do first, for the status line and the panel.
    pub(crate) fn invitation(&self) -> &'static str {
        match (self.kind(), self.on_surface()) {
            (Kind::Muscle, true) => "Drag across the model to lay a muscle on it.",
            (Kind::Muscle, false) => "Drag from the model to draw a muscle floating in front of it.",
            (Kind::Tube, true) => "Drag across the model to lay a tube on its surface.",
            (Kind::Tube, false) => "Drag from the model to grow a tube out of it.",
        }
    }

    /// The name of an undo step that is about a whole shape.
    pub(crate) fn label(&self, tube: &'static str, muscle: &'static str) -> &'static str {
        if self.kind() == Kind::Muscle { muscle } else { tube }
    }

    /// A starting radius in proportion to what is in the scene, so the first
    /// tube is neither a thread nor a trunk.
    fn size_defaults_to_scene(&mut self, cx: &Ctx) {
        let bounds = cx.scene().bounds(cx.scene().active());
        if bounds.is_empty() {
            return;
        }
        self.defaults_sized = true;
        let half = bounds.size() * 0.5;
        // A muscle is one of many on a body, where a tube is a limb of it.
        let share = if self.kind() == Kind::Muscle { 0.09 } else { 0.12 };
        let radius = (half.x + half.y + half.z) / 3.0 * share;
        self.defaults.radius = radius.clamp(TubeSettings::MIN_RADIUS, TubeSettings::MAX_RADIUS);
    }

    pub(crate) fn new_tube(&mut self, anchor: Option<sculpt_toolkit::ObjectId>) -> LiveTube {
        self.next_id += 1;
        LiveTube::new(self.next_id - 1, anchor, self.defaults)
    }

    // ---- building ------------------------------------------------------------

    /// Takes away every tube `gone` picks out, ending any gesture first.
    fn drop_tubes(&mut self, gone: impl Fn(&LiveTube) -> bool) {
        if !self.tubes.iter().any(&gone) {
            return;
        }
        self.cancel_gesture();
        let mut index = 0;
        while index < self.tubes.len() {
            if gone(&self.tubes[index]) {
                self.remove_tube_at(index);
            } else {
                index += 1;
            }
        }
        self.touch();
    }

    /// Another scene has taken this one's place (an Open, a New Scene). Every
    /// object a tube was drawn on has gone, and those tubes go with them, now,
    /// whether or not the tool is up. `refresh` finds them gone on the tool's
    /// next frame, but only while it is up: put away, no frame looks, and the
    /// tubes would outlive their objects, named by the question about closing
    /// a scene they are nowhere in, and back with Undo of the Open on the
    /// days the tool happened to be away. What the question before the Open
    /// said (`unmerged`) has to be what happens. The tool's own undo steps
    /// are of those tubes, so they end here and the next Z is the scene's. A
    /// tube drawn in mid-air stands in the world and stays.
    pub(crate) fn scene_replaced(&mut self) {
        self.cancel_gesture();
        self.drop_tubes(|tube| tube.state.anchor.is_some());
        self.clear_history();
    }

    /// Brings every tube's copies (and, with `previews`, its preview mesh)
    /// up to date with its points, its settings, where its anchor stands
    /// and the symmetry its anchor is sculpted under, which is each object's
    /// own: two tubes on two objects can be under two symmetries. A tube
    /// drawn on nothing has none. A tube whose anchor has been deleted has
    /// nowhere to live and goes with it.
    pub(crate) fn refresh(&mut self, cx: &Ctx, previews: bool) {
        let scene = cx.scene();
        self.drop_tubes(|tube| tube.state.anchor.is_some_and(|a| !scene.exists(a)));
        let frame_of = |tube: &LiveTube| {
            let info = tube.state.anchor.and_then(|a| scene.info(a));
            info.map_or(Frame::WORLD, |info| Frame { placement: info.placement })
        };
        let symmetry_of =
            |tube: &LiveTube| tube.state.anchor.map_or(Symmetry::NONE, |anchor| Symmetry::of(cx.symmetry_of(anchor)));
        for tube in self.tubes.iter_mut().chain(self.pointer.draft.as_mut()) {
            let anchor = tube.state.anchor;
            let cast = |ray: &sculpt_toolkit::Ray| anchor.and_then(|a| scene.raycast_object(a, ray));
            let cast: Option<crate::live::Cast> = if anchor.is_some() { Some(&cast) } else { None };
            let ground = anchor.and_then(|a| scene.info(a)).map(|info| (info.mesh, info.vertices));
            tube.build(frame_of(tube), symmetry_of(tube), cast, ground);
            if previews {
                tube.build_preview();
            }
        }
    }

    // ---- operations ----------------------------------------------------------

    pub fn toggle_selected_sharp(&mut self) {
        match self.active_tube().zip(self.selected_point()) {
            Some((tube, point)) => self.toggle_sharp(tube, point),
            None => self.say(format!("Click a {} point to select it first.", self.kind().one())),
        }
    }

    pub(crate) fn toggle_sharp(&mut self, tube: usize, point: usize) {
        self.begin_edit("Toggle Sharp", 0.0);
        let p = &mut self.tubes[tube].state.points[point];
        p.sharp = !p.sharp;
        let sharp = p.sharp;
        self.touch();
        self.commit_edit();
        let turns = format!("Sharp point: the {} turns a corner here.", self.kind().one());
        self.say(if sharp { turns } else { "Smooth point.".to_owned() });
    }

    pub fn delete_selected_point(&mut self) {
        match self.active_tube().zip(self.selected_point()) {
            Some((tube, point)) => self.delete_point(tube, point),
            None => self.say(format!("Click a {} point to select it first.", self.kind().one())),
        }
    }

    /// Removes a point; a tube left with a single point goes with it.
    pub(crate) fn delete_point(&mut self, tube: usize, point: usize) {
        self.cancel_gesture();
        if point >= self.point_count(tube) {
            return;
        }
        self.begin_edit("Delete Point", 0.0);
        self.tubes[tube].state.points.remove(point);
        if self.tubes[tube].state.points.len() < 2 {
            self.remove_tube_at(tube);
            self.say(format!("{} deleted.", self.kind().title()));
        } else {
            self.active = Some(tube);
            self.selected_point = None;
        }
        self.touch();
        self.commit_edit();
    }

    pub fn delete_active_tube(&mut self) {
        let Some(active) = self.active_tube() else {
            self.say(format!("Click a {} to select it first.", self.kind().one()));
            return;
        };
        self.cancel_gesture();
        self.begin_edit(self.label("Delete Tube", "Delete Muscle"), 0.0);
        self.remove_tube_at(active);
        self.touch();
        self.commit_edit();
        self.say(format!("{} deleted.", self.kind().title()));
    }

    pub(crate) fn remove_tube_at(&mut self, index: usize) {
        self.tubes.remove(index);
        self.active = match self.active {
            Some(a) if a == index => None,
            Some(a) if a > index => Some(a - 1),
            other => other,
        };
        self.selected_point = None;
    }

    pub(crate) fn select_tube(&mut self, tube: usize, point: Option<usize>) {
        self.active = Some(tube);
        self.selected_point = point;
    }

    // ---- the tool's own undo -------------------------------------------------
    //
    // Tube edits keep their own history while the tool is up: forty point
    // drags on the scene's history would bury the model edits either side.
    // Z answers here while there is something to step through and falls
    // through to the scene's history otherwise, which is how Z straight
    // after a merge un-merges.

    fn has_open_edit(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| p.version != self.edit_version)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.has_open_edit()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What Z would take back, for the panel.
    pub fn next_undo_label(&self) -> Option<&'static str> {
        match &self.pending {
            Some(p) if p.version != self.edit_version => Some(p.before.label),
            _ => self.undo.last().map(|s| s.label),
        }
    }

    fn capture(&self, label: &'static str) -> Snapshot {
        Snapshot { tubes: self.tubes.iter().map(|t| t.state.clone()).collect(), active: self.active, label }
    }

    fn apply(&mut self, step: Snapshot) {
        self.tubes = step.tubes.into_iter().map(LiveTube::from_state).collect();
        self.active = step.active.filter(|&a| a < self.tubes.len());
        self.selected_point = None;
        self.touch();
    }

    /// Opens an undo step, lazily: snapshotted now, pushed by `commit_edit`
    /// only if something changed. With `coalesce` above zero the edit is a
    /// streamed one (a slider, the wheel): it joins an open stream of the
    /// same label and closes by itself that many seconds after the last of
    /// them.
    pub(crate) fn begin_edit(&mut self, label: &'static str, coalesce: f64) {
        if let Some(open) = &mut self.pending {
            let open_is_streamed = open.commit_at.is_some();
            let same_stream = open_is_streamed && coalesce > 0.0 && open.before.label == label;
            if same_stream || (!open_is_streamed && coalesce <= 0.0) {
                if coalesce > 0.0 {
                    open.commit_at = Some(self.now + coalesce);
                }
                return;
            }
            self.commit_edit();
        }
        self.pending = Some(PendingEdit {
            before: self.capture(label),
            version: self.edit_version,
            commit_at: (coalesce > 0.0).then_some(self.now + coalesce),
        });
    }

    pub(crate) fn commit_edit(&mut self) {
        let Some(open) = self.pending.take() else { return };
        if open.version != self.edit_version {
            self.undo.push(open.before);
            if self.undo.len() > MAX_UNDO_STEPS {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    pub(crate) fn cancel_edit(&mut self) {
        let Some(open) = self.pending.take() else { return };
        if open.version != self.edit_version {
            self.apply(open.before);
        }
    }

    fn tick_pending_edit(&mut self) {
        let due = self.pending.as_ref().and_then(|p| p.commit_at).is_some_and(|at| self.now >= at);
        if due && matches!(self.pointer.drag, Drag::None) {
            self.commit_edit();
        }
    }

    pub fn undo(&mut self) -> bool {
        self.step(false)
    }

    pub fn redo(&mut self) -> bool {
        self.step(true)
    }

    fn step(&mut self, redo: bool) -> bool {
        self.cancel_gesture();
        self.commit_edit();
        let Some(step) = (if redo { self.redo.pop() } else { self.undo.pop() }) else { return false };
        let label = step.label;
        let here = self.capture(label);
        if redo { self.undo.push(here) } else { self.redo.push(here) }
        self.apply(step);
        self.say(format!("{}: {label}", if redo { "Redid" } else { "Undid" }));
        true
    }

    pub(crate) fn clear_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
    }

    // ---- every frame ---------------------------------------------------------

    pub(crate) fn each_frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        self.now = cx.time;
        // A merge keeps running if the user switches tool, and an undone
        // finish hands its tubes back whichever tool is up.
        self.poll_job(cx);
        self.watch_finishes(cx);
        if !cx.active {
            self.flush(cx);
            return;
        }
        self.tick_pending_edit();
        self.follow_resize(cx.pointer);
        // By the frame after a step of the scene's history, the scene has taken it.
        if std::mem::take(&mut self.scene_stepped) {
            self.tubes.iter_mut().for_each(LiveTube::lay_again);
        }
        self.refresh(cx, true);
        for tube in self.tubes.iter().chain(self.pointer.draft.as_ref()) {
            draw.mesh(&tube.preview, Style::solid(CLAY));
        }
        if cx.clean_view || self.bare {
            self.pointer.clear_hover();
        } else {
            self.update_hover(cx);
            self.draw_overlay(cx, draw);
        }
        // A streamed edit closes on a timer: keep frames coming until it has.
        if self.pending.as_ref().is_some_and(|p| p.commit_at.is_some()) {
            cx.repaint();
        }
        self.flush(cx);
    }

    fn draw_overlay(&self, cx: &Ctx, draw: &mut Draw) {
        let view = &cx.view;
        for (index, tube) in self.tubes.iter().enumerate() {
            self.draw_tube(view, draw, index, tube);
        }
        let stroke = &self.pointer.stroke;
        if matches!(self.pointer.drag, Drag::Draw) && stroke.world.len() > 1 {
            draw.overlay.polyline_3d(view, &stroke.world, 1.5, STROKE);
        }
        let idle = matches!(self.pointer.drag, Drag::None);
        if let Some(hit) = self.pointer.hover_curve.filter(|hit| idle && Some(hit.tube) == self.active) {
            if let Some(px) = view.project(hit.world) {
                draw.overlay.dot(px, 4.0, INSERT, HANDLE_OUTLINE);
            }
        }
        // The radius handle of the selected point: drag it to size the tube
        // there.
        if let Some(ring) = self.ring_handle(view).filter(|_| idle || matches!(self.pointer.drag, Drag::Ring { .. })) {
            let lit = self.pointer.hover_ring || matches!(self.pointer.drag, Drag::Ring { .. });
            draw.overlay.line(ring.centre, ring.at, 1.0, SIZE_RING);
            draw.overlay.square(ring.at, if lit { 5.5 } else { 4.5 }, if lit { HANDLE } else { SELECTED }, HANDLE_OUTLINE);
        }
        self.draw_cursor(cx, draw);
    }

    fn draw_tube(&self, view: &View, draw: &mut Draw, index: usize, tube: &LiveTube) {
        let active = Some(index) == self.active;
        let idle = matches!(self.pointer.drag, Drag::None);
        let sizing = match self.pointer.drag {
            Drag::Resize { tube: Some(t), point: Some(p), .. } | Drag::Ring { tube: t, point: p, .. } if t == index => Some(p),
            _ => None,
        };
        // The tube itself last, so its handles are on top of its copies'.
        for (k, copy) in tube.copies.iter().enumerate().rev() {
            let canonical = k == 0;
            let color = if !canonical {
                COPY_CURVE
            } else if active {
                CURVE
            } else {
                OTHER_CURVE
            };
            draw.overlay.polyline_3d(view, &copy.curve_world, if canonical && active { 2.0 } else { 1.2 }, color);

            for (i, (&world, point)) in copy.handle_world.iter().zip(&tube.state.points).enumerate() {
                let Some(px) = view.project(world) else { continue };
                let hovered = idle
                    && self.pointer.hover_handle.is_some_and(|h| h.tube == index && h.copy == k && h.point == i);
                let selected = active && canonical && Some(i) == self.selected_point;
                let radius = if !canonical {
                    3.5
                } else if !active {
                    4.0
                } else if hovered || selected {
                    6.5
                } else {
                    5.0
                };
                let fill = if selected {
                    SELECTED
                } else if point.sharp {
                    SHARP
                } else if canonical && active {
                    HANDLE
                } else {
                    DIM_HANDLE
                };
                let outline = if point.sharp && !selected { SHARP_OUTLINE } else { HANDLE_OUTLINE };
                // A sharp point is a square, the usual mark for a corner.
                if point.sharp {
                    draw.overlay.square(px, radius, fill, outline);
                } else {
                    draw.overlay.dot(px, radius, fill, outline);
                }
                // The tube's thickness at the point being pointed at or sized.
                if hovered || selected || (canonical && sizing == Some(i)) {
                    if let Some(control) = copy.controls.get(i) {
                        circle(draw, px, pixels_for(view, world, control.radius), 1.5, SIZE_RING, None);
                    }
                }
            }
        }
    }

    /// The radius the next stroke would draw, at the pointer, sized for the
    /// depth under it, so thickness can be judged before drawing. Dashed
    /// when strokes float free of the surface.
    fn draw_cursor(&self, cx: &Ctx, draw: &mut Draw) {
        let pointer = &self.pointer;
        let busy = !matches!(pointer.drag, Drag::None) || pointer.hover_handle.is_some() || pointer.hover_curve.is_some();
        if busy || pointer.hover_ring || self.finish.is_busy() {
            return;
        }
        let Some((px, depth_point)) = pointer.cursor else { return };
        let radius = pixels_for(&cx.view, depth_point, self.defaults.radius);
        let dash = (!self.on_surface()).then_some((4.0, 3.0));
        circle(draw, px, radius, 1.2, CURSOR, dash);
    }
}
