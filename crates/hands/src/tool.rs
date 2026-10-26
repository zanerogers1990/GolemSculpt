//! The Hand & Foot tool: shape a hand or a foot with sliders and poses, see it live beside the model,
//! place it with the gizmo, and Accept it as an ordinary object to sculpt.
//!
//! The settings are a scaffold, like the Lathe's profile: not in the scene's file, with an undo
//! history of their own while the tool is up. Accept is where the shape becomes real geometry, one
//! step on the scene's history. The settings are kept, so the next hand starts from the last one.
//!
//! | Input | Action |
//! | --- | --- |
//! | Gizmo | Move, turn and size the hand or foot |
//! | Enter | Accept: make it an object |
//! | Ctrl+Z / Ctrl+Shift+Z | Undo / redo a change of setting or place |
//! | Esc | Put the tool away; the settings are kept |

use glam::{Quat, Vec2, Vec3};
use sculpt_geom::{Aabb, TriMesh};
use sculpt_toolkit::{
    Color32, Command, Ctx, Draw, Event, GizmoPhase, GizmoTarget, Info, Job, Key, MeshData, NewObject, Placement, Preview,
    PreviewData, RadialItem, Style, Tool, View,
};
use sculpt_volume::Surface;

use crate::{Body, FootShape, HandShape};

/// Lattice cells across one hand length (wrist to fingertip), or one foot length, for the live
/// preview, and for a quicker one while a slider is moving if the full one is slow.
pub const PREVIEW_DETAIL: f32 = 110.0;
const DRAFT_DETAIL: f32 = 70.0;
/// A preview that took longer than this is built at draft detail while a slider moves.
const DRAFT_MILLISECONDS: f64 = 25.0;
/// Changes closer together than this are one undo step: a slider drag is one step.
const COALESCE_SECONDS: f64 = 0.6;
const MAX_UNDO: usize = 200;

/// Detail for Accept: lattice cells across one length.
pub const MIN_DETAIL: f32 = 100.0;
pub const MAX_DETAIL: f32 = 500.0;
const DEFAULT_DETAIL: f32 = 260.0;

/// A new hand is this long against the size of the scene's main object, and a foot this long. A
/// foot is about a third longer than a hand.
const HAND_SIZE: f32 = 0.55;
const FOOT_SIZE: f32 = 0.75;

/// Lit clay, as the scene shows a fresh object.
const CLAY: Color32 = Color32::from_rgb(184, 184, 189);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hand,
    Foot,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Hand => "Hand",
            Kind::Foot => "Foot",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
    /// A right one where the gizmo is, and a left one mirrored across the world's X = 0 plane.
    Pair,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Right => "Right",
            Side::Left => "Left",
            Side::Pair => "Pair",
        }
    }
}

/// Everything Ctrl+Z steps through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setup {
    pub kind: Kind,
    pub side: Side,
    pub hand: HandShape,
    pub foot: FootShape,
    /// Where the hand stands: its wrist joint, turned and sized. A hand is 1 long at scale 1.
    pub hand_at: Placement,
    /// Where the foot stands: its ankle joint.
    pub foot_at: Placement,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            kind: Kind::Hand,
            side: Side::Right,
            hand: HandShape::default(),
            foot: FootShape::default(),
            hand_at: Placement::IDENTITY,
            foot_at: Placement::IDENTITY,
        }
    }
}

impl Setup {
    /// The right-handed body the settings make.
    pub fn body(&self) -> Body {
        match self.kind {
            Kind::Hand => self.hand.body(),
            Kind::Foot => self.foot.body(),
        }
    }

    pub fn placement(&self) -> Placement {
        match self.kind {
            Kind::Hand => self.hand_at,
            Kind::Foot => self.foot_at,
        }
    }

    fn placement_mut(&mut self) -> &mut Placement {
        match self.kind {
            Kind::Hand => &mut self.hand_at,
            Kind::Foot => &mut self.foot_at,
        }
    }
}

/// What a preview was built from, to know when to build it again.
struct Shown {
    body: Body,
    draft: bool,
}

/// Objects Accept made, ready to commit.
struct Made {
    objects: Vec<(String, MeshData, Placement)>,
    triangles: usize,
}

pub struct HandTool {
    setup: Setup,
    /// Cells across one length for Accept. A setting of the tool, not an undo step.
    detail: f32,
    /// Whether the shape has been put beside the scene yet; Accept leaves it where the new object
    /// is, so the next visit puts it beside the scene again.
    seated: bool,
    undo: Vec<Setup>,
    redo: Vec<Setup>,
    /// When the setup last changed, so that a slider drag is one step and builds drafts.
    changed_at: f64,
    preview: Preview,
    /// The same mesh reflected, for a left one.
    twin: Preview,
    shown: Option<Shown>,
    preview_milliseconds: f64,
    preview_triangles: usize,
    error: Option<String>,
    job: Option<Job<Made>>,
}

impl Default for HandTool {
    fn default() -> HandTool {
        HandTool::new()
    }
}

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn Tool> {
    Box::new(HandTool::new())
}

/// The size of the scene's main object (the first in the list), which a new hand is sized from.
fn reference_size(cx: &Ctx) -> f32 {
    let scene = cx.scene();
    let Some(&main) = scene.objects().first() else { return 1.0 };
    let bounds = scene.bounds(main);
    if bounds.is_empty() {
        return 1.0;
    }
    let half = bounds.size() * 0.5;
    ((half.x + half.y + half.z) / 3.0 * 2.0).clamp(0.05, 20.0)
}

/// The box round everything visible in the scene.
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

/// Whether the box lies well inside the view: clear of the screen's edges, where the panels are.
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

/// A placement reflected across the world's X = 0 plane. With the mesh reflected too, the result is
/// the mirror image of the original in the world.
pub fn mirror_placement(p: Placement) -> Placement {
    let q = p.rotation;
    Placement { position: Vec3::new(-p.position.x, p.position.y, p.position.z), rotation: Quat::from_xyzw(q.x, -q.y, -q.z, q.w), scale: p.scale }
}

/// A mesh reflected across x = 0, wound so it still faces out.
pub fn mirror_mesh(mesh: &TriMesh) -> TriMesh {
    let positions = mesh.positions.iter().map(|p| Vec3::new(-p.x, p.y, p.z)).collect();
    let indices = mesh.indices.chunks_exact(3).flat_map(|t| [t[0], t[2], t[1]]).collect();
    TriMesh::new(positions, indices)
}

fn preview_data(surface: &Surface, mirrored: bool) -> PreviewData {
    if !mirrored {
        return PreviewData { positions: surface.positions.clone(), normals: surface.normals.clone(), indices: surface.indices.clone() };
    }
    let flip = |v: &Vec3| Vec3::new(-v.x, v.y, v.z);
    PreviewData {
        positions: surface.positions.iter().map(flip).collect(),
        normals: surface.normals.iter().map(flip).collect(),
        indices: surface.indices.chunks_exact(3).flat_map(|t| [t[0], t[2], t[1]]).collect(),
    }
}

/// The whole size of a placement, never zero or reflected.
fn sane_scale(scale: Vec3) -> Vec3 {
    let fix = |v: f32| if v.is_finite() { v.abs().clamp(1e-3, 1e4) } else { 1.0 };
    Vec3::new(fix(scale.x), fix(scale.y), fix(scale.z))
}

impl HandTool {
    pub fn new() -> HandTool {
        HandTool {
            setup: Setup::default(),
            detail: DEFAULT_DETAIL,
            seated: false,
            undo: Vec::new(),
            redo: Vec::new(),
            changed_at: f64::NEG_INFINITY,
            preview: Preview::new(),
            twin: Preview::new(),
            shown: None,
            preview_milliseconds: 0.0,
            preview_triangles: 0,
            error: None,
            job: None,
        }
    }

    pub fn setup(&self) -> &Setup {
        &self.setup
    }

    /// Replaces the settings without an undo step (the panel records its own).
    pub(crate) fn set_setup(&mut self, setup: Setup) {
        self.setup = setup;
    }

    pub fn detail(&self) -> f32 {
        self.detail
    }

    pub fn set_detail(&mut self, detail: f32) {
        self.detail = if detail.is_finite() { detail.clamp(MIN_DETAIL, MAX_DETAIL) } else { DEFAULT_DETAIL };
    }

    pub fn is_making(&self) -> bool {
        self.job.is_some()
    }

    /// Roughly how many triangles Accept makes, from the preview's count; per object.
    pub fn estimate_triangles(&self) -> usize {
        let ratio = self.detail / PREVIEW_DETAIL;
        (self.preview_triangles as f32 * ratio * ratio) as usize
    }

    // ------------------------------------------------------------------------------- changes

    /// Records the setup as it was before a change, as an undo step. Changes in quick succession (a
    /// slider being dragged) make one step; `fresh` starts a new one regardless (a button).
    pub(crate) fn changed(&mut self, before: Setup, now: f64, fresh: bool) {
        if before == self.setup {
            return;
        }
        if fresh || now - self.changed_at > COALESCE_SECONDS || self.undo.is_empty() {
            self.undo.push(before);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.changed_at = if fresh { f64::NEG_INFINITY } else { now };
    }

    /// Changes the setup as one undo step of its own.
    pub fn edit(&mut self, cx: &Ctx, change: impl FnOnce(&mut Setup)) {
        let before = self.setup;
        change(&mut self.setup);
        self.changed(before, cx.time, true);
    }

    fn step(&mut self, redo: bool) -> bool {
        let (from, to) = if redo { (&mut self.redo, &mut self.undo) } else { (&mut self.undo, &mut self.redo) };
        let Some(setup) = from.pop() else { return false };
        to.push(self.setup);
        self.setup = setup;
        self.changed_at = f64::NEG_INFINITY;
        true
    }

    /// Puts the hand and the foot beside the scene, sized to it, and brings them into view.
    fn seat(&mut self, cx: &mut Ctx) {
        let reference = reference_size(cx);
        let scene = scene_bounds(cx);
        let (centre, reach) = if scene.is_empty() { (Vec3::ZERO, 0.0) } else { (scene.center(), scene.size().x * 0.5) };
        // A right hand goes on the figure's right, which faces the camera from -X; a left one on +X.
        let toward = if self.setup.side == Side::Left { 1.0 } else { -1.0 };
        let hand = reference * HAND_SIZE;
        let foot = reference * FOOT_SIZE;
        let beside = |size: f32| centre.x + toward * (reach + size * 0.45);
        let floor = if scene.is_empty() { 0.0 } else { scene.min.y };
        self.setup.hand_at = Placement {
            position: Vec3::new(beside(hand), centre.y - hand * 0.45, centre.z),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(hand),
        };
        self.setup.foot_at = Placement {
            position: Vec3::new(beside(foot), floor + 0.22 * foot, centre.z - foot * 0.25),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(foot),
        };
        self.seated = true;
        self.undo.clear();
        self.redo.clear();
        let shape = self.world_bounds();
        if !comfortably_in_view(&cx.view, shape) {
            cx.frame_view(Aabb::union(shape, scene));
        }
    }

    /// The world box round what Accept would make, from the preview's own box.
    fn world_bounds(&self) -> Aabb {
        let local = self.setup.body().field().bounds();
        let corners = |placement: Placement, mirrored: bool| {
            let matrix = placement.matrix();
            (0..8).map(move |c| {
                let pick = |axis: usize| if c >> axis & 1 == 0 { local.min[axis] } else { local.max[axis] };
                let x = if mirrored { -pick(0) } else { pick(0) };
                matrix.transform_point3(Vec3::new(x, pick(1), pick(2)))
            })
        };
        let placement = self.setup.placement();
        let mut points: Vec<Vec3> = corners(placement, self.setup.side == Side::Left).collect();
        if self.setup.side == Side::Pair {
            points.extend(corners(mirror_placement(placement), true));
        }
        Aabb::of(&points)
    }

    // ------------------------------------------------------------------------------- preview

    /// Builds the preview again if the settings changed since it was built.
    fn refresh(&mut self, cx: &Ctx) {
        let body = self.setup.body();
        let moving = cx.time - self.changed_at < 0.25;
        let draft = moving && self.preview_milliseconds > DRAFT_MILLISECONDS;
        // A draft stands while the slider moves; once it stops, the full preview replaces it.
        if self.shown.as_ref().is_some_and(|shown| shown.body == body && (!shown.draft || moving)) {
            return;
        }
        let start = std::time::Instant::now();
        let detail = if draft { DRAFT_DETAIL } else { PREVIEW_DETAIL };
        match body.surface(1.0 / detail) {
            Ok(surface) => {
                self.preview.set_parts(preview_data(&surface, false));
                self.twin.set_parts(preview_data(&surface, true));
                if !draft {
                    self.preview_triangles = surface.indices.len() / 3;
                }
                self.error = None;
            }
            Err(e) => {
                self.preview.clear();
                self.twin.clear();
                self.error = Some(e);
            }
        }
        if !draft {
            self.preview_milliseconds = start.elapsed().as_secs_f64() * 1000.0;
        }
        self.shown = Some(Shown { body, draft });
    }

    // ------------------------------------------------------------------------------- accept

    /// Starts building the objects at full detail, off the interface thread.
    pub fn accept(&mut self, cx: &mut Ctx) {
        if self.job.is_some() {
            return;
        }
        let setup = self.setup;
        let body = setup.body();
        let cell = 1.0 / self.detail;
        let placement = setup.placement();
        let scale = sane_scale(placement.scale);
        let kind = setup.kind.name();
        self.job = Some(cx.spawn(&format!("Make {kind}"), move |job| {
            job.progress("Shaping");
            let mut mesh = body.mesh(cell)?;
            if job.cancelled() {
                return Err("Cancelled.".into());
            }
            // The size goes into the mesh, so the object is sculpted at scale 1.
            mesh.positions.iter_mut().for_each(|p| *p *= scale);
            let at = Placement { scale: Vec3::ONE, ..placement };
            let triangles = mesh.triangle_count();
            job.progress("Preparing to sculpt");
            let objects = match setup.side {
                Side::Right => vec![(format!("Right {kind}"), job.bake(mesh, None), at)],
                Side::Left => vec![(format!("Left {kind}"), job.bake(mirror_mesh(&mesh), None), at)],
                Side::Pair => {
                    let left = mirror_mesh(&mesh);
                    vec![
                        (format!("Right {kind}"), job.bake(mesh, None), at),
                        (format!("Left {kind}"), job.bake(left, None), mirror_placement(at)),
                    ]
                }
            };
            Ok(Made { objects, triangles })
        }));
    }

    fn land(&mut self, cx: &mut Ctx, result: Result<Made, String>) {
        let made = match result {
            Ok(made) => made,
            Err(e) => {
                cx.status(e, false);
                return;
            }
        };
        let count = made.objects.len();
        let commands = made
            .objects
            .into_iter()
            .enumerate()
            .map(|(i, (name, mesh, at))| {
                let object = NewObject::new(name, mesh).placed(at);
                Command::Add(if i == 0 { object } else { object.unselected() })
            })
            .collect();
        let kind = self.setup.kind.name();
        let label = if count == 2 { format!("Make {kind} Pair") } else { format!("Make {kind}") };
        let receipt = cx.commit(&label, commands);
        if receipt.added.is_empty() {
            cx.status(format!("The {} could not be made into an object.", kind.to_lowercase()), false);
            return;
        }
        let what = if count == 2 { format!("a pair of {}", if kind == "Foot" { "feet" } else { "hands" }) } else { format!("a {}", kind.to_lowercase()) };
        let each = if count == 2 { " each" } else { "" };
        cx.status(format!("Made {what} ({} triangles{each}). Ctrl+Z takes it away again.", made.triangles), true);
        // The shape stands where the new object is now: next time, it is put beside the scene again,
        // and Ctrl+Z is the scene's, to take the new object away.
        self.seated = false;
        self.undo.clear();
        self.redo.clear();
        self.shown = None;
        cx.finish();
    }
}

impl Tool for HandTool {
    fn info(&self) -> Info {
        Info {
            id: "hands",
            name: "Hand & Foot",
            icon: "tool_hands",
            tip: "Build a hand or a foot from sliders and poses, then accept it as a base to sculpt.",
            hotkey: None,
        }
    }

    fn enter(&mut self, cx: &mut Ctx) {
        if !self.seated {
            self.seat(cx);
        }
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::KeyDown(Key::Enter) => {
                self.accept(cx);
                true
            }
            Event::Undo => self.step(false),
            Event::Redo => self.step(true),
            Event::Radial(0) => {
                self.accept(cx);
                true
            }
            Event::Radial(1) => {
                self.edit(cx, |s| s.kind = Kind::Hand);
                true
            }
            Event::Radial(2) => {
                self.edit(cx, |s| s.kind = Kind::Foot);
                true
            }
            Event::Radial(3) => {
                self.edit(cx, |s| s.side = if s.side == Side::Pair { Side::Right } else { Side::Pair });
                true
            }
            Event::Gizmo(placement, phase) => {
                if phase == GizmoPhase::Begin {
                    self.undo.push(self.setup);
                    self.redo.clear();
                }
                *self.setup.placement_mut() = Placement { scale: sane_scale(placement.scale), ..placement };
                if phase == GizmoPhase::End {
                    // A press let go without moving changed nothing: no step.
                    if self.undo.last() == Some(&self.setup) {
                        self.undo.pop();
                    }
                    self.changed_at = f64::NEG_INFINITY;
                }
                true
            }
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        // A build started here lands even if the tool was put away meanwhile.
        if let Some(result) = self.job.as_mut().and_then(Job::poll) {
            self.job = None;
            self.land(cx, result);
        }
        if !cx.active {
            return;
        }
        self.refresh(cx);
        let placement = self.setup.placement();
        let style = Style::solid(CLAY);
        match self.setup.side {
            Side::Right => draw.instances(&self.preview, style, &[placement.matrix()]),
            Side::Left => draw.instances(&self.twin, style, &[placement.matrix()]),
            Side::Pair => {
                draw.instances(&self.preview, style, &[placement.matrix()]);
                draw.instances(&self.twin, style, &[mirror_placement(placement).matrix()]);
            }
        }
        if cx.time - self.changed_at < 0.3 {
            cx.repaint();
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        self.show_panel(ui, cx);
    }

    fn radial(&self) -> Vec<RadialItem> {
        vec![
            RadialItem { label: "Accept", icon: "check", on: false },
            RadialItem { label: "Hand", icon: "tool_hands", on: self.setup.kind == Kind::Hand },
            RadialItem { label: "Foot", icon: "tool_hands", on: self.setup.kind == Kind::Foot },
            RadialItem { label: "Pair", icon: "mirror", on: self.setup.side == Side::Pair },
        ]
    }

    fn can_undo(&self, redo: bool) -> bool {
        if redo { !self.redo.is_empty() } else { !self.undo.is_empty() }
    }

    /// The gizmo sits on the wrist (or the ankle) of the one being shaped.
    fn gizmo(&self) -> Option<GizmoTarget> {
        let placement = self.setup.placement();
        Some(GizmoTarget::body(placement, placement.scale.max_element() * 0.3))
    }

    /// A hand (or `foot`), posed (`fist`), styled (`cartoon`, `creature`), as a `pair`, or `made`
    /// into an object; several joined by `+`, as `foot+pair+made`.
    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        if !self.seated {
            self.seat(cx);
        }
        for word in what.split('+') {
            match word {
                "foot" => self.setup.kind = Kind::Foot,
                "left" => self.setup.side = Side::Left,
                "pair" => self.setup.side = Side::Pair,
                "fist" => crate::HandPose::Fist.apply(&mut self.setup.hand),
                "relaxed" => crate::HandPose::Relaxed.apply(&mut self.setup.hand),
                "spread" => crate::HandPose::Spread.apply(&mut self.setup.hand),
                "pointed" => crate::FootPose::Pointed.apply(&mut self.setup.foot),
                "cartoon" => {
                    crate::HandStyle::Cartoon.apply(&mut self.setup.hand);
                    crate::FootStyle::Cartoon.apply(&mut self.setup.foot);
                }
                "creature" => {
                    crate::HandStyle::Creature.apply(&mut self.setup.hand);
                    crate::FootStyle::Creature.apply(&mut self.setup.foot);
                }
                _ => {}
            }
        }
        let shape = self.world_bounds();
        cx.frame_view(Aabb::union(shape, scene_bounds(cx)));
        if what.split('+').any(|w| w == "made") {
            self.accept(cx);
            if let Some(job) = self.job.take() {
                let result = job.wait();
                self.land(cx, result);
            }
        }
    }
}
