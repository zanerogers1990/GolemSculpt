//! The Lathe tool: shape a 2D profile in the viewport and see it turned into a solid live, like pulling
//! clay up on a wheel, then Create it as an ordinary sculptable object.
//!
//! The profile is edited ON the preview's own silhouette: the handles sit in the plane through the
//! axis that faces the camera, on both sides, so dragging a handle drags the outline you are looking
//! at. The view can still orbit - the handles follow round, because a solid of revolution has the same
//! silhouette from every side.
//!
//! Like the SSphere rig, the profile is a scaffold rather than a scene object: it is not saved, and it
//! keeps its own undo history ([`crate::history`]). Create is where the work becomes real geometry, and
//! Create is one step on the scene's history. Undoing it hands the scaffold back.
//!
//! | Input | Action |
//! | --- | --- |
//! | Drag a handle | Move that point (Shift: only along the dominant direction) |
//! | Drag the curve | Insert a point there, and keep dragging it |
//! | Click empty space | Extend the profile from its nearer end |
//! | Ctrl + drag | Sketch a whole new profile in one stroke |
//! | Double-click a handle | Toggle it between smooth and corner |
//! | Right-click a handle | Delete it (Delete / Backspace deletes the selected one) |
//! | Drag the axis | Move the lathe |
//! | Esc | Cancel the drag, else deselect, else leave |
//! | Enter | Create the mesh |
//!
//! A drag on empty background orbits, as everywhere in the app; this is the one change from the Unity
//! tool, where a press beside the curve added a point and went on dragging it. Here the point is added by
//! the click, and a drag that starts on nothing is the camera's.

mod demo;
mod input;
mod overlay;
mod panel;
#[cfg(test)]
mod tests;

use std::time::Instant;

use glam::{Vec2, Vec3};
use sculpt_geom::Aabb;
use sculpt_toolkit::{
    Button, Color32, Command, Ctx, Draw, Event, Info, Job, Key, MeshData, NewObject, ObjectId, Placement, Preview,
    PreviewData, Style, Tool, View,
};

use crate::history::{COALESCE_SECONDS, History};
use crate::plane::{MAX_SCALE, MIN_SCALE, Plane};
use crate::{LatheBuild, LatheBuilder, LathePreset, LatheProfile, LatheSettings, round_segments};

/// Pick distances and thresholds, in interface points.
const HANDLE_PICK: f32 = 11.0;
const CURVE_PICK: f32 = 8.0;
const AXIS_PICK: f32 = 6.0;
/// An END point dropped this close to the axis, on screen, lands on it and closes that end.
const AXIS_SNAP: f32 = 10.0;
const DRAG_THRESHOLD: f32 = 3.0;
const DOUBLE_CLICK_SECONDS: f64 = 0.3;
const SKETCH_SPACING: f32 = 4.0;
/// A sketch whose ends meet within this many points closes into a loop.
const SKETCH_CLOSE: f32 = 18.0;

/// A rebuild this slow while dragging switches the drag to half the radial detail, so the handle keeps
/// up with the cursor. The full preview is rebuilt the moment the drag ends.
const DRAFT_MILLISECONDS: f64 = 12.0;

/// The preview's colour: lit clay, much as the scene's own material shows a fresh object.
const CLAY: Color32 = Color32::from_rgb(184, 184, 189);

/// One gesture in progress. A drag is computed ABSOLUTELY from the press (the point's start position and
/// where on it you grabbed), never by accumulating per-event deltas, so nothing creeps and Esc can put it
/// back exactly.
#[derive(Clone, Debug, Default)]
enum Drag {
    #[default]
    None,
    /// A control point, grabbed on `side` of the axis.
    Point { index: usize, side: f32, press: Vec2, start: Vec2, grab: Vec2, moved: bool },
    /// The whole lathe, by its axis.
    Axis { press: Vec2, start: Vec3, anchor: Vec3, moved: bool },
    /// A stroke in signed profile coordinates, for Ctrl + drag.
    Sketch { points: Vec<Vec2>, last: Vec2 },
}

/// Where a new point would go if the press landed on the curve.
#[derive(Clone, Copy, Debug)]
struct Insert {
    /// Index in the point list.
    index: usize,
    /// On the curve, in profile space.
    position: Vec2,
    side: f32,
    /// The hit was on a cap: the old end point, where the cap met the wall, stays a corner.
    sharpen: Option<usize>,
}

/// What is under the pointer, for the highlight.
#[derive(Clone, Copy, Debug, Default)]
struct Hover {
    point: Option<(usize, f32)>,
    insert: Option<Insert>,
    axis: bool,
}

/// What the last preview build left behind, for the panel and for deciding when to build again.
#[derive(Default)]
struct Built {
    /// The profile version the preview shows.
    version: Option<u64>,
    /// A setting changed since.
    dirty: bool,
    /// The preview was built at half detail because a drag was outrunning the builder.
    draft: bool,
    last_milliseconds: f64,
    has_mesh: bool,
    /// Why there is no mesh, in words for the artist.
    error: Option<String>,
    /// Triangles Create will make, at full detail (the preview may be lighter).
    triangles: usize,
}

/// A finished Create, waiting to be committed.
struct Made {
    data: MeshData,
    position: Vec3,
    triangles: usize,
    watertight: bool,
}

/// The object Create made, to notice when it is undone.
#[derive(Clone, Copy)]
struct Created {
    id: ObjectId,
    alive: bool,
}

pub struct Lathe {
    profile: LatheProfile,
    plane: Plane,
    settings: LatheSettings,
    history: History,
    selected: Option<usize>,
    /// The first visit seats a starting shape beside the scene; later ones pick up where it was left.
    placed: bool,
    /// A Create just left the shape where the new object now stands: move it aside on the next visit.
    reseat: bool,
    /// How big the scene's main object is, which presets and the axis are sized from.
    reference: f32,
    drag: Drag,
    hover: Hover,
    /// The handle last pressed and when, for the double click.
    last_click: Option<(usize, f64)>,
    builder: LatheBuilder,
    build: LatheBuild,
    preview: Preview,
    built: Built,
    job: Option<Job<Made>>,
    created: Option<Created>,
}

impl Default for Lathe {
    fn default() -> Lathe {
        Lathe::new()
    }
}

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn Tool> {
    Box::new(Lathe::new())
}

/// The scene's main object (the first in the list) sets the size of a new shape, so it is neither a
/// speck nor a giant next to the model.
fn reference_height(cx: &Ctx) -> f32 {
    let scene = cx.scene();
    let Some(&main) = scene.objects().first() else { return 1.0 };
    let bounds = scene.bounds(main);
    if bounds.is_empty() {
        return 1.0;
    }
    let half = bounds.size() * 0.5;
    ((half.x + half.y + half.z) / 3.0 * 2.0).clamp(0.05, 20.0)
}

/// The box around everything visible in the scene.
fn scene_bounds(cx: &Ctx) -> Aabb {
    let scene = cx.scene();
    scene
        .objects()
        .into_iter()
        .filter(|&id| scene.info(id).is_some_and(|info| info.visible))
        .map(|id| scene.bounds(id))
        .filter(|bounds| !bounds.is_empty())
        .fold(Aabb::EMPTY, Aabb::union)
}

/// Whether the box lies well inside the view: clear of the screen's edges, which is where the panels are.
fn comfortably_in_view(view: &View, bounds: Aabb) -> bool {
    if bounds.is_empty() {
        return true;
    }
    let (low, high) = (view.size * Vec2::new(0.22, 0.08), view.size * Vec2::new(0.78, 0.92));
    (0..8).all(|corner| {
        let pick = |axis: usize| if corner >> axis & 1 == 0 { bounds.min[axis] } else { bounds.max[axis] };
        view.project(Vec3::new(pick(0), pick(1), pick(2))).is_some_and(|px| px.cmpge(low).all() && px.cmple(high).all())
    })
}

/// A build this slow while a drag is going on means the next one should be lighter.
fn wants_draft(dragging: bool, last_milliseconds: f64) -> bool {
    dragging && last_milliseconds > DRAFT_MILLISECONDS
}

impl Lathe {
    pub fn new() -> Lathe {
        Lathe {
            profile: LatheProfile::new(),
            plane: Plane::new(Vec3::ZERO),
            settings: LatheSettings::default(),
            history: History::default(),
            selected: None,
            placed: false,
            reseat: false,
            reference: 1.0,
            drag: Drag::None,
            hover: Hover::default(),
            last_click: None,
            builder: LatheBuilder::new(),
            build: LatheBuild::default(),
            preview: Preview::new(),
            built: Built::default(),
            job: None,
            created: None,
        }
    }

    // ------------------------------------------------------------------------------- reading

    pub fn profile(&self) -> &LatheProfile {
        &self.profile
    }

    pub fn settings(&self) -> LatheSettings {
        self.settings
    }

    /// Where the lathe stands and how big it is.
    pub fn plane(&self) -> &Plane {
        &self.plane
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The live preview mesh, in the profile's own space (the plane's matrix carries it into the world).
    pub fn preview(&self) -> &Preview {
        &self.preview
    }

    /// Triangles Create will make at the full detail setting.
    pub fn create_triangles(&self) -> usize {
        self.built.triangles
    }

    /// The object Create last made, if it is still in the scene.
    pub fn created(&self) -> Option<ObjectId> {
        self.created.filter(|c| c.alive).map(|c| c.id)
    }

    /// World-space radius of the widest point.
    pub fn radius(&self) -> f32 {
        self.profile.max_radius() * self.plane.scale
    }

    /// World-space height.
    pub fn height(&self) -> f32 {
        self.profile.height() * self.plane.scale
    }

    fn dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// Halfway up the profile, in profile units: where the lathe sits at the scene's middle height.
    fn middle_height(&self) -> f32 {
        if self.profile.is_empty() { 0.0 } else { (self.profile.min_height() + self.profile.max_height()) * 0.5 }
    }

    /// The axis as drawn: a little past the shape at both ends, in profile height.
    fn axis_span(&self) -> (f32, f32) {
        let scale = self.plane.scale;
        let (low, high) =
            if self.profile.is_empty() { (0.0, self.reference / scale) } else { (self.profile.min_height(), self.profile.max_height()) };
        let pad = ((high - low) * 0.15).max(self.reference * 0.1 / scale);
        (low - pad, high + pad)
    }

    /// A world box around the shape, for framing the camera on it.
    fn shape_bounds(&self) -> Aabb {
        let scale = self.plane.scale;
        let radius = self.radius();
        let (low, high) = (self.profile.min_height() * scale, self.profile.max_height() * scale);
        let root = self.plane.root;
        Aabb::new(root + Vec3::new(-radius, low, -radius), root + Vec3::new(radius, high, radius))
    }

    // ------------------------------------------------------------------------------- history

    fn begin_edit(&mut self, now: f64, label: &str) {
        self.history.begin(now, label, 0.0, &self.profile, &self.plane);
    }

    /// A streamed edit (a slider): one step however many moves it takes.
    fn begin_stream(&mut self, now: f64, label: &str) {
        self.history.begin(now, label, COALESCE_SECONDS, &self.profile, &self.plane);
    }

    fn commit_edit(&mut self) {
        self.history.commit(&self.profile, &self.plane);
    }

    /// Ends any gesture, keeping what it did. Safe to call when nothing is in progress: every operation
    /// calls it first, so a panel click mid-drag can never leave a drag hanging.
    fn end_drag(&mut self) {
        if matches!(self.drag, Drag::None) {
            return;
        }
        self.drag = Drag::None;
        self.commit_edit();
    }

    /// Esc: puts the profile back exactly as it was at the press, including taking away a point the
    /// press itself added.
    fn cancel_drag(&mut self, cx: &mut Ctx) {
        if matches!(self.drag, Drag::None) {
            return;
        }
        self.drag = Drag::None;
        if let Some(pose) = self.history.cancel(&self.profile, &self.plane) {
            pose.restore(&mut self.profile, &mut self.plane);
        }
        self.forget_missing_selection();
        cx.status("Cancelled.", true);
    }

    fn forget_missing_selection(&mut self) {
        if self.selected.is_some_and(|i| i >= self.profile.len()) {
            self.selected = None;
        }
    }

    fn step(&mut self, cx: &mut Ctx, redo: bool) -> bool {
        self.end_drag();
        let stepped = if redo {
            self.history.redo(&self.profile, &self.plane)
        } else {
            self.history.undo(&self.profile, &self.plane)
        };
        let Some((pose, label)) = stepped else { return false };
        pose.restore(&mut self.profile, &mut self.plane);
        self.forget_missing_selection();
        cx.status(format!("{}: {label}", if redo { "Redid" } else { "Undid" }), true);
        true
    }

    // ----------------------------------------------------------------------------- operations

    /// Puts a preset's points in the profile, sized to the scene. Not an undo step: it is how the shape
    /// starts.
    fn load_preset(&mut self, preset: LathePreset) {
        let (points, closed_loop) = preset.points(self.reference / self.plane.scale);
        self.profile.set_points(&points, closed_loop);
        self.selected = None;
    }

    fn apply_preset(&mut self, cx: &mut Ctx, preset: LathePreset) {
        self.end_drag();
        self.begin_edit(cx.time, &format!("Preset: {}", preset.name()));
        self.load_preset(preset);
        self.commit_edit();
        // A wide preset can stand under a panel: bring the shape and the scene into view if so.
        if !comfortably_in_view(&cx.view, self.shape_bounds()) {
            cx.frame_view(Aabb::union(self.shape_bounds(), scene_bounds(cx)));
        }
        let name = preset.name();
        cx.status(
            if self.profile.closed_loop() {
                format!("{name}: a closed loop, revolved into a ring.")
            } else {
                format!("{name}: drag the handles to reshape it.")
            },
            true,
        );
    }

    /// Empties the profile so the next clicks draw a new one from scratch. Undoable.
    fn clear_profile(&mut self, cx: &mut Ctx) {
        self.end_drag();
        self.begin_edit(cx.time, "Clear Profile");
        self.profile.clear();
        self.selected = None;
        self.commit_edit();
        cx.status("Cleared. Click beside the dashed axis to place points bottom to top, or Ctrl-drag to sketch.", true);
    }

    fn set_closed_loop(&mut self, cx: &mut Ctx, closed: bool) {
        if closed == self.profile.closed_loop() {
            return;
        }
        if closed && self.profile.len() < 3 {
            cx.status("A closed loop needs at least 3 points.", false);
            return;
        }
        self.end_drag();
        self.begin_edit(cx.time, if closed { "Close Loop" } else { "Open Loop" });
        self.profile.set_closed_loop(closed);
        self.selected = None;
        self.commit_edit();
    }

    fn delete_point(&mut self, cx: &mut Ctx, index: usize) {
        if index >= self.profile.len() {
            return;
        }
        self.end_drag();
        self.begin_edit(cx.time, "Delete Point");
        self.profile.remove(index);
        if self.profile.closed_loop() && self.profile.len() < 3 {
            self.profile.set_closed_loop(false);
        }
        self.selected = None;
        self.commit_edit();
    }

    fn delete_selected(&mut self, cx: &mut Ctx) {
        match self.selected {
            Some(index) => self.delete_point(cx, index),
            None => cx.status("Click a handle to select a point first.", false),
        }
    }

    /// Corner against smooth. A corner is how a flat base meets a wall, or a lid meets a rim.
    fn toggle_sharp(&mut self, cx: &mut Ctx, index: usize) {
        if index >= self.profile.len() {
            return;
        }
        self.end_drag();
        self.begin_edit(cx.time, "Toggle Corner");
        let sharp = !self.profile[index].sharp;
        self.profile.set_sharp(index, sharp);
        self.commit_edit();
        cx.status(if sharp { "Corner point - the curve turns sharply here." } else { "Smooth point." }, true);
    }

    fn toggle_selected_sharp(&mut self, cx: &mut Ctx) {
        match self.selected {
            Some(index) => self.toggle_sharp(cx, index),
            None => cx.status("Click a handle to select a point first.", false),
        }
    }

    /// Stretches the profile away from (or toward) the axis until its widest point is `radius` across.
    fn set_radius(&mut self, cx: &Ctx, radius: f32) {
        let current = self.radius();
        if current < 1e-5 || radius <= 0.0 || (radius - current).abs() <= current * 1e-5 {
            return;
        }
        self.begin_stream(cx.time, "Radius");
        self.profile.scale_radius(radius / current);
    }

    /// Stretches the profile along the axis, about its base, until it is `height` tall.
    fn set_height(&mut self, cx: &Ctx, height: f32) {
        let current = self.height();
        if current < 1e-5 || height <= 0.0 || (height - current).abs() <= current * 1e-5 {
            return;
        }
        self.begin_stream(cx.time, "Height");
        self.profile.scale_height(height / current);
    }

    /// The overall size of the shape, about the lathe's own origin.
    fn set_scale(&mut self, cx: &Ctx, scale: f32) {
        let scale = scale.clamp(MIN_SCALE, MAX_SCALE);
        if (scale - self.plane.scale).abs() <= self.plane.scale * 1e-5 {
            return;
        }
        self.begin_stream(cx.time, "Scale");
        self.plane.scale = scale;
    }

    /// Rounds a detail request to what the builder uses.
    fn set_segments(&mut self, segments: usize) {
        let segments = round_segments(segments);
        if segments != self.settings.radial_segments {
            self.settings.radial_segments = segments;
            self.built.dirty = true;
        }
    }

    fn set_cap_ends(&mut self, on: bool) {
        if on != self.settings.cap_ends {
            self.settings.cap_ends = on;
            self.built.dirty = true;
        }
    }

    fn set_even_triangles(&mut self, on: bool) {
        if on != self.settings.even_triangles {
            self.settings.even_triangles = on;
            self.built.dirty = true;
        }
    }

    /// Moves the axis to the middle of the view, keeping the shape and its distance from the camera: to
    /// build it where it will be used.
    fn move_axis_to_view_centre(&mut self, cx: &mut Ctx) {
        self.end_drag();
        let middle = self.middle_height() * self.plane.scale;
        let heart = self.plane.root + Vec3::Y * middle;
        let Some(target) = cx.view.on_facing_plane(cx.view.size * 0.5, heart) else { return };
        self.begin_edit(cx.time, "Move Axis");
        self.plane.root = target - Vec3::Y * middle;
        self.commit_edit();
        cx.status("Axis moved to the centre of the view.", true);
    }

    /// Seats the shape beside whatever is in the scene, level with its middle, and brings both into
    /// view. Beside, not at the view's centre: the scene's starting object sits there, and a vase built
    /// inside it would be invisible.
    fn place_beside_scene(&mut self, cx: &mut Ctx) {
        let scene = scene_bounds(cx);
        let middle = self.middle_height() * self.plane.scale;
        if !scene.is_empty() {
            let mut right = cx.view.right;
            right.y = 0.0;
            let right = if right.length_squared() > 1e-6 { right.normalize() } else { Vec3::X };
            // The scene's half-width along `right`, then clear it by the lathe's own radius (presets are
            // at most ~0.7 x height wide) and a gap.
            let half = scene.size() * 0.5;
            let scene_half = (half.x * right.x).abs() + (half.z * right.z).abs();
            let mut centre = scene.center() + right * (scene_half + self.reference * 0.75);
            centre.y = scene.center().y;
            self.plane.root = centre - Vec3::Y * middle;
        }
        cx.frame_view(Aabb::union(self.shape_bounds(), scene));
    }

    // ---------------------------------------------------------------------------------- mesh

    /// Rebuilds the preview when the profile or the detail settings changed. While a handle is being
    /// dragged and a full rebuild has been slow, builds at half the radial detail instead:
    /// responsiveness first, then the real thing on release.
    fn refresh_mesh(&mut self) {
        let draft = wants_draft(matches!(self.drag, Drag::Point { .. }), self.built.last_milliseconds);
        let stale = self.built.version != Some(self.profile.version()) || self.built.dirty || (self.built.draft && !draft);
        if !stale {
            return;
        }
        let started = Instant::now();
        let settings = self.settings.for_preview(draft);
        match self.builder.build(&self.profile, &settings, &mut self.build) {
            Ok(()) => {
                // Only the profile is tessellated for the count, so it stays cheap at any detail.
                self.built.triangles = if settings == self.settings {
                    self.build.triangle_count()
                } else {
                    self.builder.count(&self.profile, &self.settings).map_or(0, |counts| counts.triangles)
                };
                // The preview takes the build's arrays rather than copying them; the outline stays for
                // the overlay.
                self.preview.set_parts(PreviewData {
                    positions: std::mem::take(&mut self.build.mesh.positions),
                    normals: std::mem::take(&mut self.build.normals),
                    indices: std::mem::take(&mut self.build.mesh.indices),
                });
                self.built.has_mesh = true;
                self.built.error = None;
            }
            Err(error) => {
                self.preview.clear();
                self.built.has_mesh = false;
                self.built.triangles = 0;
                self.built.error = Some(error);
            }
        }
        if !draft {
            self.built.last_milliseconds = started.elapsed().as_secs_f64() * 1000.0;
        }
        self.built.version = Some(self.profile.version());
        self.built.draft = draft;
        self.built.dirty = false;
    }

    // ---------------------------------------------------------------------------------- create

    /// Starts turning the shape into a real object at full detail, off the interface thread: the
    /// build is quick, but fitting the sculpting structures to a mesh of a million triangles is not.
    /// The profile is kept, so the next Create can be a variation.
    fn create(&mut self, cx: &mut Ctx) {
        self.end_drag();
        if self.job.is_some() {
            return;
        }
        if let Err(error) = self.builder.count(&self.profile, &self.settings) {
            cx.status(error, false);
            return;
        }
        // The job cannot borrow from the tool: it gets copies.
        let (profile, settings, plane) = (self.profile.clone(), self.settings, self.plane);
        self.job = Some(cx.spawn("Lathe", move |job| {
            job.progress("Revolving the profile");
            let mut build = LatheBuild::default();
            LatheBuilder::new().build(&profile, &settings, &mut build)?;
            if job.cancelled() {
                return Err("Cancelled.".to_owned());
            }
            // Scale baked into the vertices, so the new object has a unit transform (what export and
            // the gizmo expect), and re-origined to its vertical centre on the axis, so its pivot and
            // its mirror plane sit in the middle of it.
            let centre = build.centre_and_scale(plane.scale);
            let (triangles, watertight) = (build.triangle_count(), build.watertight);
            job.progress(format!("Preparing {triangles} triangles"));
            let data = job.bake(build.into_mesh(), None);
            Ok(Made { data, position: plane.root + Vec3::Y * (centre * plane.scale), triangles, watertight })
        }));
    }

    /// Collects a finished Create, whether or not the tool is still up.
    fn land(&mut self, cx: &mut Ctx) {
        let Some(result) = self.job.as_mut().and_then(Job::poll) else { return };
        self.job = None;
        self.land_result(cx, result);
    }

    /// The finished mesh becomes an object: one undo step, and straight into sculpting it.
    fn land_result(&mut self, cx: &mut Ctx, result: Result<Made, String>) {
        let made = match result {
            Ok(made) => made,
            Err(error) => return cx.status(error, false),
        };
        let object = NewObject::new("Lathe", made.data).placed(Placement::at(made.position));
        let receipt = cx.commit("Create Lathe", vec![Command::Add(object)]);
        let Some(&id) = receipt.added.first() else {
            return cx.status("The lathe could not be added to the scene.", false);
        };
        self.created = Some(Created { id, alive: true });
        self.reseat = true;
        let name = cx.scene().info(id).map_or_else(|| "Lathe".to_owned(), |info| info.name.to_owned());
        let closure = if made.watertight { "closed solid" } else { "open surface" };
        cx.status(
            format!("Created {name} ({} tris, {closure}). The profile is kept for another.", panel::grouped(made.triangles)),
            true,
        );
        cx.finish();
    }

    /// Notices that Z took the created object away again, and hands the scaffold back: the profile is
    /// kept, so the tool simply comes back up. A redo that returns the object puts the tool away again.
    fn watch_created(&mut self, cx: &mut Ctx) {
        let Some(created) = &mut self.created else { return };
        let exists = cx.scene().exists(created.id);
        if created.alive && !exists {
            created.alive = false;
            cx.start();
            cx.status("Create undone: the profile is back to shape.", true);
        } else if !created.alive && exists {
            created.alive = true;
            cx.finish();
        }
    }
}

impl Tool for Lathe {
    fn info(&self) -> Info {
        Info {
            id: "lathe",
            name: "Lathe",
            icon: "tool_lathe",
            tip: "Shapes a turned solid - a vase, bowl, bottle, knob or ring - by dragging its outline, like clay on a wheel.",
            hotkey: None,
        }
    }

    fn enter(&mut self, cx: &mut Ctx) {
        self.reference = reference_height(cx);
        if !self.placed {
            // A first visit gets a starting shape.
            self.placed = true;
            self.load_preset(LathePreset::Vase);
            self.place_beside_scene(cx);
            cx.status("Vase: drag the handles to reshape it.", true);
        } else if std::mem::take(&mut self.reseat) && self.created().is_some() {
            self.place_beside_scene(cx);
        }
        // After the move: the plane faces the camera from where the axis now stands.
        self.plane.face(&cx.view);
        self.built.dirty = true;
        self.refresh_mesh();
    }

    fn leave(&mut self, _cx: &mut Ctx) {
        self.end_drag();
        self.hover = Hover::default();
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        // The plane follows the camera between drags, and is held still during one.
        if !self.dragging() {
            self.plane.face(&cx.view);
        }
        match event {
            Event::Press(Button::Left) => self.press(cx),
            Event::Press(Button::Right) => self.right_press(cx),
            Event::Drag => {
                self.drag_to(cx, cx.pointer, cx.mods.shift);
                true
            }
            Event::Release => {
                self.release(cx);
                true
            }
            Event::Click(Button::Left) => self.click(cx),
            Event::KeyDown(Key::Escape) => self.escape(cx),
            Event::KeyDown(Key::Delete | Key::Backspace) => self.delete_key(cx),
            Event::KeyDown(Key::Enter) => {
                self.create(cx);
                true
            }
            Event::Undo => self.step(cx, false),
            Event::Redo => self.step(cx, true),
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        // A job started here and an object made by it are watched whether or not the tool is up.
        self.land(cx);
        self.watch_created(cx);
        if !cx.active {
            return;
        }
        self.history.tick(cx.time, self.dragging(), &self.profile, &self.plane);
        if !self.dragging() {
            self.plane.face(&cx.view);
        }
        self.reference = reference_height(cx);
        self.hover = self.find_hover(cx);
        self.refresh_mesh();
        draw.instances(&self.preview, Style::solid(CLAY), &[self.plane.matrix()]);
        if !cx.clean_view {
            self.draw_overlay(cx, &mut draw.overlay);
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        self.show_panel(ui, cx);
    }

    fn can_undo(&self, redo: bool) -> bool {
        if redo { self.history.can_redo() } else { self.history.can_undo(&self.profile, &self.plane) }
    }

    /// The lathe in states worth a look. With nothing after the colon: the starting vase. `edit`: a
    /// point of it pulled out and left selected. `bowl`, `ring`: those presets. `sketch`: a goblet
    /// drawn in one Ctrl-stroke. `created`: Create done, the object standing beside the model.
    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        self.run_demo(cx, what);
    }
}
