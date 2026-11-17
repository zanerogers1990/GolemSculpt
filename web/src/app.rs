//! The web app: the objects, the brushes, the camera, undo, and the panels.
//!
//! The sculpting is the desktop app's own (`crate::sculpt`, `crate::mesh`,
//! compiled from the desktop's files). This file is only what drives it: it
//! turns the page's pointer into `PointerSample`s the way the desktop's
//! `app.rs` does (begin on press, samples and a `frame` every frame, `end` on
//! release), keeps the undo list, and hands the renderer what changed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui::{self, Color32, Key, Modifiers, PointerButton, Pos2, Sense, Stroke as Line};
use glam::{Vec2, Vec3};

use crate::browser::{self, Later};
use crate::camera::{self, Camera, View};
use crate::files;
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{Mesh, StrokeDelta, VisibilityDelta};
use crate::render::{self, Gpu, Upload};
use crate::scene::{Object, Props};
use crate::sculpt::{Pace, PointerSample, Sculptor};
use crate::tools::{self as tubes, Tools};

#[path = "ui.rs"]
mod ui;
// How the model is shown: the renderer's frame and the Look tab's cards.
#[path = "look.rs"]
mod look;
// Brushes, masking and the pen: alphas, the brush wheel and hold keys, the
// mask commands and gestures, radial symmetry, and the pen's every sample.
#[path = "alpha.rs"]
mod alpha;
#[path = "brush_keys.rs"]
mod brush_keys;
#[path = "masking.rs"]
mod masking;
#[path = "pen.rs"]
mod pen;
#[path = "radial.rs"]
mod radial;
// Several objects, the Objects card and the Transform tool (the gizmo).
#[path = "objects.rs"]
mod objects;
// The welcome card for a first visit, and Help (F1).
#[path = "welcome.rs"]
mod welcome;
// Sculpt layers: the Layers card and its undo steps.
#[path = "layers.rs"]
mod layers;
// Subdivision levels: the Subdivision card, Ctrl+D, Page Up and Page Down.
#[path = "levels.rs"]
mod levels;
// The whole scene as a file: Save Scene, and opening one.
#[path = "scene_file.rs"]
mod scene_file;
// Symmetry Repair (Check Symmetry, Make Symmetric, Cut & Mirror, Cleanup)
// and the Live Mirror copies of an object.
#[path = "symmetry_repair.rs"]
mod symmetry_repair;
#[path = "live_mirror.rs"]
mod live_mirror;
// FBX in and out: Open and Save FBX (the desktop's reader and writer).
#[path = "fbx_app.rs"]
mod fbx_app;
// Tooltip languages: the first visit's question and the files they need.
#[path = "languages.rs"]
mod languages;
// The keyboard shortcuts a person can change, and their card.
#[path = "shortcuts.rs"]
mod shortcuts;
// Reference pictures: Add Picture..., a photo dropped, their card.
#[path = "reference_images.rs"]
mod reference_images;
// The Pose brush's preview, vertex paint and its cards, and face groups.
#[path = "pose.rs"]
mod pose;
#[path = "paint.rs"]
mod paint;
#[path = "groups.rs"]
mod groups;
#[path = "painted_obj.rs"]
mod painted_obj;
// The Deform card, Apply Scale, Set Origin and Edit Pivot, Project Detail,
// the booleans and Join, and Quad Remesh; what they keep is `shaping`.
#[path = "shaping.rs"]
mod shaping;
#[path = "deformers.rs"]
mod deformers;
#[path = "placement.rs"]
mod placement;
#[path = "project_detail.rs"]
mod project_detail;
#[path = "combine.rs"]
mod combine;
#[path = "quad_remesh.rs"]
mod quad_remesh;
// The SSpheres tool: its gestures, keys, panel and Make Skin.
#[path = "spheres.rs"]
mod spheres;
// Finger gestures, and the layout for a phone (mobile.rs).
#[path = "mobile.rs"]
mod mobile;
// The blue pulse on an object just picked or framed (flash.rs).
#[path = "flash.rs"]
mod flash;

/// Where the scene kept in the browser carries the objects' live mirrors,
/// after the view: what comes next is each object's axes and centre.
const MIRRORS_TAG: &[u8; 4] = b"MIRR";

/// How many undo steps are kept, and how many of them may hold a whole mesh
/// (a Remesh, a Subdivide, a new shape), which cost far more memory.
const UNDO_STEPS: usize = 80;
const UNDO_MESHES: usize = 4;
/// The most memory those whole-mesh steps may take together (beyond the
/// newest, always kept): a browser tab has 4 GB in all.
const UNDO_MESH_BYTES: usize = 1 << 30;
/// The most triangles Subdivide makes: past this a browser tab runs short.
const MAX_TRIANGLES: usize = 4_000_000;
/// Seconds after the last change before the scene is put in the browser's storage.
const STORE_AFTER: f64 = 2.0;
/// The scene kept in the browser: one model (W01, before objects), or the
/// objects (W02).
const SCENE_MAGIC: &[u8; 8] = b"GOLEMW01";
const SCENE_MAGIC_2: &[u8; 8] = b"GOLEMW02";
/// W02 with each mesh's hidden faces, subdivision levels and sculpt layers.
const SCENE_MAGIC_3: &[u8; 8] = b"GOLEMW03";
/// W03 with each mesh's face groups and paint layers after its layers.
const SCENE_MAGIC_4: &[u8; 8] = b"GOLEMW04";

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    None,
    Sculpt,
    Orbit,
    Pan,
    Zoom,
    /// A tool (Tubes) has the pointer.
    Tool,
    /// A handle of the Transform tool's gizmo has it.
    Gizmo,
}

/// One undo step. Each that changes a mesh names its object, which is made
/// the active one again to undo it.
enum Step {
    Stroke(u64, StrokeDelta),
    /// The mesh before (for undo) or after (for redo) a whole-mesh change.
    Mesh(u64, Box<Mesh>),
    /// Faces hidden or shown again (the Hide tool).
    Faces(u64, VisibilityDelta),
    /// Objects' places, names or shown state before (for undo) or after.
    Props(Vec<(u64, Props)>),
    /// An object added or deleted: `held` is the object while it is out of
    /// the scene, `None` while it is in; undo and redo each put it in or
    /// take it out.
    Exists { index: usize, id: u64, held: Option<Box<Object>> },
    /// The sculpt layers, and the positions they moved, on the other side
    /// of a Layers card command (layers.rs).
    Layers(u64, crate::mesh::layers::LayersStep),
    /// The subdivision levels on the other side of Delete Lower or Delete
    /// Higher; the mesh itself is unchanged (levels.rs).
    Levels(u64, Option<Box<crate::mesh::levels::MeshLevels>>),
    /// The paint layers on the other side of a Paint Layers command (paint.rs).
    Paint(u64, crate::mesh::paint::PaintStep),
    /// The face groups (and what else the command changed: the mask, the
    /// hidden faces) on the other side of a Face Groups command (groups.rs).
    Groups(u64, crate::mesh::groups::GroupsStep),
    /// Several steps taken as one (Validate of a live mirror: the new
    /// objects, and the mirror taken off), in the order they were made.
    Many(Vec<Step>),
}

impl Step {
    /// Whether the step holds a whole mesh, which costs far more memory.
    fn heavy(&self) -> bool {
        match self {
            Step::Many(steps) => steps.iter().any(Step::heavy),
            _ => matches!(self, Step::Mesh(..) | Step::Exists { held: Some(_), .. }),
        }
    }

    /// The memory of the whole meshes the step holds, near enough.
    fn mesh_bytes(&self) -> usize {
        match self {
            Step::Many(steps) => steps.iter().map(Step::mesh_bytes).sum(),
            Step::Mesh(_, mesh) => mesh.memory_bytes(),
            Step::Exists { held: Some(object), .. } => object.mesh.as_ref().map_or(0, Mesh::memory_bytes),
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Sculpt,
    /// One of the hosted tools is up (`crate::tools`), not only Tubes.
    Tubes,
    /// The Transform tool: the gizmo on the selected objects (objects.rs).
    Transform,
    /// The SSpheres tool: a blockout of connected spheres (spheres.rs).
    Spheres,
}

/// The active object under the pointer: where in the world, and where on
/// its own mesh.
#[derive(Clone, Copy)]
struct HoverHit {
    pos: Vec3,
    local: Vec3,
    /// Which of the active object's places it is on: 0 the object, else a
    /// live mirror copy's flip mask.
    side: u8,
}

pub struct GolemApp {
    /// The active object's mesh, the one brushes and the tools work on (in
    /// its own space); the other objects keep theirs (objects.rs).
    pub(crate) mesh: Mesh,
    /// Every object, the active one among them, in the Objects card's order.
    pub(crate) objects: Vec<Object>,
    pub(crate) active: u64,
    /// The selected objects, the active one first: what the gizmo moves.
    pub(crate) selected: Vec<u64>,
    next_object: u64,
    transform: objects::Transform,
    pub(crate) sculptor: Sculptor,
    pub(crate) camera: Camera,
    undo: Vec<Step>,
    redo: Vec<Step>,
    drag: Drag,
    /// The mesh on the graphics card is not this one: send it whole.
    upload_whole: bool,
    /// The number of the whole mesh last sent, and of the one the card has.
    generation: u64,
    card_has: Arc<AtomicU64>,
    /// What the card's buffers have room for (vertices, corners), and how
    /// many of each it has been sent.
    card_room: (usize, usize),
    card_sent: (usize, usize),
    /// Changes sent in parts that the card has not yet been seen to take,
    /// with the number of the upload that first carried each (see `upload`).
    unsent: Vec<Unsent>,
    upload_seq: u64,
    card_took: Arc<AtomicU64>,
    pub(crate) view: View,
    hover: Option<HoverHit>,
    pointer_px: Option<Vec2>,
    pub(crate) status: String,
    flat_shading: bool,
    mask_tint: bool,
    shape: Primitive,
    density: usize,
    remesh_resolution: u32,
    /// Decimate's Target Triangles.
    decimate_target: f32,
    /// Cavity shading: on, and how much ridges catch light and valleys darken.
    cavity: bool,
    cavity_ridge: f32,
    cavity_valley: f32,
    subdivide_smooth: bool,
    picked: Option<Later<(String, Vec<u8>)>>,
    stored: Option<Later<Option<Vec<u8>>>>,
    changed_at: Option<f64>,
    /// The interface's own state: folded panels, tabs, icons.
    ui: ui::UiState,
    /// The matcap in use (`ui::MATCAPS`), and every matcap's picture for the card.
    matcap: usize,
    matcaps: Arc<Vec<image::RgbaImage>>,
    show_backfaces: bool,
    /// The backdrop, lens, finishes, wireframe, guides and Save Picture (look.rs).
    look: look::Look,
    pub(crate) mode: Mode,
    pub(crate) tubes: Tools,
    last_time: f64,
    /// The brush wheel and the hold keys (`brush_keys.rs`).
    brush_keys: brush_keys::BrushKeys,
    /// The quick mask gestures (`masking.rs`).
    gestures: masking::Gestures,
    /// The selection flash (flash.rs).
    pub(crate) flash: flash::Flash,
    /// The welcome card (welcome.rs).
    welcome: welcome::Welcome,
    /// A scene file picked or dropped, waiting for "Open this scene?" (scene_file.rs).
    pending_scene: Option<scene_file::Pending>,
    /// Symmetry Repair's plane and Match Tolerance (symmetry_repair.rs).
    repair: symmetry_repair::Repair,
    /// Which of the active object's places the brush is on: 0 the object
    /// itself, else the flip mask of one of its live mirror copies
    /// (live_mirror.rs). A stroke begun on a copy works the object through it.
    side: u8,
    /// The Deform card, the pivot, Project Detail, the booleans and Quad Remesh (shaping.rs).
    shaping: shaping::Shaping,
    /// The SSpheres tool and its rig (spheres.rs).
    spheres: spheres::Spheres,
    /// The fingers, and the phone layout's switches and menu (mobile.rs).
    touch: mobile::Touch,
}

impl GolemApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> GolemApp {
        // The desktop's look: its colours and spacing (its fonts are Windows', so egui's stand in).
        sculpt_toolkit::theme::install(&cc.egui_ctx);
        let ui = ui::UiState::new(&cc.egui_ctx);
        let matcaps: Arc<Vec<image::RgbaImage>> = Arc::new(ui.pictures.matcaps.iter().map(|(_, image)| image.clone()).collect());
        if let Some(state) = &cc.wgpu_render_state {
            let first = matcaps.first().cloned().unwrap_or_else(|| image::RgbaImage::from_pixel(4, 4, image::Rgba([180, 180, 180, 255])));
            let webgpu = crate::render::shadows::supported(state.adapter.get_info().backend);
            let gpu = Gpu::new(&state.device, &state.queue, state.target_format, &first, webgpu);
            state.renderer.write().callback_resources.insert(gpu);
        }

        let mut sculptor = Sculptor::default();
        sculptor.library.select_named("Clay Strips", &mut sculptor.settings);
        sculptor.settings.symmetry = [true, false, false];
        // Move as Zane has it on the desktop (his settings, 2026-11-15): the
        // surface follows 44% of the pen's travel, not all of it, with the
        // falloff a touch softer. The built-in 100% pulls a long horn out of
        // a drag that makes a broad bump on the desktop. The web keeps no
        // brush settings between visits, so this is where Move starts.
        let moving = &mut sculptor.settings.per_brush[crate::sculpt::Brush::Move.index()];
        moving.strength = 0.44;
        moving.focal_shift = -0.03;
        let mut app = GolemApp {
            mesh: primitives::build(Primitive::Sphere, 120_000, 1.0),
            objects: vec![Object::new(1, "Sphere".into(), None)],
            active: 1,
            selected: vec![1],
            next_object: 2,
            transform: Default::default(),
            sculptor,
            camera: Camera::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            drag: Drag::None,
            upload_whole: true,
            generation: 0,
            card_has: Arc::new(AtomicU64::new(0)),
            card_room: (0, 0),
            card_sent: (0, 0),
            unsent: Vec::new(),
            upload_seq: 0,
            card_took: Arc::new(AtomicU64::new(0)),
            view: Camera::default().view(Vec2::ZERO, Vec2::ONE, 1.0, 1.0),
            hover: None,
            pointer_px: None,
            status: "Draw on the model to sculpt. Drag off it to turn the view.".into(),
            flat_shading: false,
            mask_tint: true,
            shape: Primitive::Sphere,
            density: 120_000,
            remesh_resolution: 192,
            decimate_target: 50_000.0,
            cavity: browser::pref("cavity") != "off",
            cavity_ridge: 0.6,
            cavity_valley: 1.0,
            subdivide_smooth: true,
            picked: None,
            stored: Some(browser::load_scene()),
            changed_at: None,
            ui,
            matcap: browser::pref("matcap").parse().unwrap_or(0usize).min(matcaps.len().saturating_sub(1)),
            matcaps,
            show_backfaces: false,
            look: look::Look::new(),
            mode: Mode::Sculpt,
            tubes: Tools::default(),
            last_time: 0.0,
            brush_keys: Default::default(),
            gestures: Default::default(),
            flash: Default::default(),
            welcome: Default::default(),
            pending_scene: None,
            repair: Default::default(),
            side: 0,
            shaping: Default::default(),
            spheres: Default::default(),
            touch: Default::default(),
        };
        app.frame_model(false);
        browser::ready();
        app
    }

    // ---- the model ----------------------------------------------------------

    fn scene_size(&self) -> f32 {
        self.scene_bounds().size().max_element().max(1e-4)
    }

    /// Frames the active object.
    fn frame_model(&mut self, animate: bool) {
        let aspect = (self.view.size.x / self.view.size.y.max(1.0)).max(0.1);
        // With its live mirror copies: all that is on screen of it.
        let bounds = self.active_object().shown_bounds(&self.mesh);
        self.camera.frame(bounds, if aspect.is_finite() { aspect } else { 1.0 }, animate);
    }

    /// Swaps in a new mesh for the active object as one undo step.
    pub(crate) fn replace_mesh(&mut self, mesh: Mesh, what: &str) {
        self.layer_strength_done();
        let before = std::mem::replace(&mut self.mesh, mesh);
        let note = Self::rebuilt_note(&before, &self.mesh);
        self.push_undo(Step::Mesh(self.active, Box::new(before)));
        self.upload_whole = true;
        self.mark_changed();
        self.status = format!("{what}: {} triangles.{note}", group_digits(self.mesh.live_triangle_count()));
    }

    /// A tool replaced the model (`before` is what it was): one undo step.
    pub(crate) fn mesh_replaced(&mut self, before: Mesh) {
        let note = Self::rebuilt_note(&before, &self.mesh);
        if !note.is_empty() {
            self.status = format!("{}{note}", self.status.trim_end());
        }
        self.push_undo(Step::Mesh(self.active, Box::new(before)));
        self.upload_whole = true;
        self.mark_changed();
    }

    /// A change of the model's vertices or mask made outside a brush stroke.
    pub(crate) fn stroke_done(&mut self, delta: StrokeDelta) {
        self.push_undo(Step::Stroke(self.active, delta));
        self.mark_changed();
    }

    /// Faces of the model hidden or shown again by a tool: one undo step.
    pub(crate) fn faces_changed(&mut self, delta: VisibilityDelta) {
        self.push_undo(Step::Faces(self.active, delta));
        self.mark_changed();
    }

    pub(crate) fn note_changed(&mut self) {
        self.mark_changed();
    }

    fn push_undo(&mut self, step: Step) {
        self.redo.clear();
        self.undo.push(step);
        // The oldest steps go first; a step can only be undone after every
        // later one, so dropping from the front never breaks the rest.
        // Big meshes also go by their memory, so a few Remeshes at a high
        // Resolution cannot fill the tab; the newest copy is always kept.
        let over_memory = |undo: &[Step]| {
            let heavy = || undo.iter().filter(|s| s.heavy());
            heavy().count() > 1 && heavy().map(Step::mesh_bytes).sum::<usize>() > UNDO_MESH_BYTES
        };
        while self.undo.len() > UNDO_STEPS
            || self.undo.iter().filter(|s| s.heavy()).count() > UNDO_MESHES
            || over_memory(&self.undo)
        {
            self.undo.remove(0);
        }
    }

    /// How many whole-model copies undo and redo hold, and their memory.
    pub(crate) fn undo_copies(&self) -> (usize, usize) {
        let heavy = self.undo.iter().chain(&self.redo).filter(|s| s.heavy());
        heavy.fold((0, 0), |(n, bytes), s| (n + 1, bytes + s.mesh_bytes()))
    }

    /// Free Memory (Remesh card, web only): keeps the newest whole-model undo
    /// step and forgets everything older, and drops redo if it holds a copy.
    /// Only the front of the history goes, so what is left still undoes right.
    pub(crate) fn free_undo_memory(&mut self) {
        let (before, bytes_before) = self.undo_copies();
        if self.redo.iter().any(Step::heavy) {
            self.redo.clear();
        }
        if let Some(newest) = self.undo.iter().rposition(Step::heavy) {
            self.undo.drain(..newest);
        }
        let (after, bytes_after) = self.undo_copies();
        self.status = format!(
            "Freed {} by forgetting {} older undo cop{}.",
            layers::megabytes(bytes_before - bytes_after),
            before - after,
            if before - after == 1 { "y" } else { "ies" }
        );
    }

    fn undo_redo(&mut self, undo: bool) {
        if self.sculptor.is_active() {
            return;
        }
        // A Strength slider still held is its own step, closed before this one.
        self.layer_strength_done();
        // So is a deformation not yet applied (deformers.rs).
        self.deform_commit();
        // The tool that is up undoes its own steps first (a tube's point moved, say).
        if self.mode == Mode::Tubes && self.tool_undo(!undo) {
            self.status = if undo { "Undone." } else { "Redone." }.into();
            return;
        }
        let popped = if undo { self.undo.pop() } else { self.redo.pop() };
        let Some(step) = popped else {
            self.status = if undo { "Nothing to undo." } else { "Nothing to redo." }.into();
            return;
        };
        let step = match self.flip_step(step, undo) {
            Ok(step) => step,
            Err(_) => {
                self.status = "That step was on an object that is no longer there.".into();
                return;
            }
        };
        if undo {
            self.redo.push(step);
        } else {
            self.undo.push(step);
        }
        self.mark_changed();
        self.status = if undo { "Undone." } else { "Redone." }.into();
    }

    /// Undoes (or redoes) one step, and hands back the step that does the
    /// opposite. `Err` with the step untouched when its object is gone.
    fn flip_step(&mut self, step: Step, undo: bool) -> Result<Step, Step> {
        // A step that changes a mesh is undone on its own object, which
        // becomes the active one (as on the desktop).
        let on = match &step {
            Step::Stroke(id, _) | Step::Mesh(id, _) | Step::Faces(id, _) | Step::Layers(id, _) | Step::Levels(id, _) => {
                Some(*id)
            }
            Step::Paint(id, _) | Step::Groups(id, _) => Some(*id),
            _ => None,
        };
        if on.is_some_and(|id| !self.activate(id)) {
            return Err(step);
        }
        let step = match step {
            Step::Stroke(id, delta) => {
                self.mesh.apply_delta(&delta, undo);
                Step::Stroke(id, delta)
            }
            Step::Mesh(id, mut other) => {
                std::mem::swap(&mut self.mesh, &mut other);
                self.upload_whole = true;
                Step::Mesh(id, other)
            }
            Step::Faces(id, delta) => {
                self.mesh.apply_visibility(&delta, undo);
                Step::Faces(id, delta)
            }
            Step::Props(list) => Step::Props(self.swap_props(list)),
            Step::Exists { index, id, held } => {
                let held = self.toggle_exists(index, id, held);
                Step::Exists { index, id, held }
            }
            Step::Layers(id, mut step) => {
                step.swap(&mut self.mesh);
                Step::Layers(id, step)
            }
            Step::Levels(id, mut levels) => {
                std::mem::swap(&mut self.mesh.levels, &mut levels);
                Step::Levels(id, levels)
            }
            Step::Paint(id, mut step) => {
                step.swap(&mut self.mesh);
                Step::Paint(id, step)
            }
            Step::Groups(id, mut step) => {
                step.swap(&mut self.mesh);
                Step::Groups(id, step)
            }
            // Undone last first, redone first first; kept in the order made.
            Step::Many(steps) => {
                let mut done = Vec::with_capacity(steps.len());
                let ordered: Vec<Step> = if undo { steps.into_iter().rev().collect() } else { steps };
                for step in ordered {
                    match self.flip_step(step, undo) {
                        Ok(step) | Err(step) => done.push(step),
                    }
                }
                if undo {
                    done.reverse();
                }
                Step::Many(done)
            }
        };
        Ok(step)
    }

    fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.mode == Mode::Tubes
    }

    fn can_redo(&self) -> bool {
        !self.redo.is_empty() || self.mode == Mode::Tubes
    }

    fn open_picker(&mut self) {
        self.picked = Some(browser::pick_file());
    }

    fn mark_changed(&mut self) {
        self.changed_at = Some(self.last_time);
    }

    /// The chosen shape in the active object's place.
    fn new_shape(&mut self) {
        let mesh = primitives::build(self.shape, self.density, 1.0);
        self.replace_mesh(mesh, self.shape.name());
        self.frame_model(true);
    }

    /// The chosen shape as a new object, in the middle of the scene.
    fn add_shape(&mut self) {
        let mesh = primitives::build(self.shape, self.density, 1.0);
        let props = Props {
            name: self.shape.name().to_owned(),
            visible: true,
            position: Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: Vec3::ONE,
            mirror: None,
        };
        self.add_object(mesh, props, true);
        self.status = format!("Added a {}. Move it with the Transform tool.", self.shape.name().to_lowercase());
    }

    /// A file opened is a new object.
    fn open_file(&mut self, name: &str, bytes: &[u8]) {
        // A whole scene (Save Scene) asks first: it replaces what is here.
        if scene_file::is_scene(name, bytes) {
            self.scene_file_picked(name, bytes);
            return;
        }
        // A photo is a reference picture (reference_images.rs).
        if reference_images::is_picture(name) {
            self.add_reference(name, bytes.to_vec());
            return;
        }
        // An FBX may hold several objects, each its own (fbx_app.rs).
        if fbx_app::is_fbx(name) {
            self.open_fbx(name, bytes);
            return;
        }
        // An OBJ with colours or face groups keeps them (painted_obj.rs).
        let painted = name.to_ascii_lowercase().ends_with(".obj").then(|| painted_obj::read(bytes)).flatten();
        let read = match painted {
            Some(mesh) => Ok(mesh),
            None => files::read(name, bytes).map(|t| Mesh::new(t.positions, t.indices, None)),
        };
        match read {
            Ok(mesh) => {
                let tris = mesh.live_triangle_count();
                let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
                let props = Props {
                    name: stem.to_owned(),
                    visible: true,
                    position: Vec3::ZERO,
                    rotation: glam::Quat::IDENTITY,
                    scale: Vec3::ONE,
                    mirror: None,
                };
                self.add_object(mesh, props, true);
                self.frame_model(true);
                self.status = format!("Opened {name}: {} triangles, as a new object.", group_digits(tris));
            }
            Err(e) => self.status = e,
        }
    }

    /// The model without what dynamic topology's merges leave inside it
    /// (triangles shrunk to points, vertices no triangle uses), when it has
    /// any: what goes to a file, as on the desktop (`scene::tidied`).
    fn tidied(&self) -> Option<Mesh> {
        tidy(&self.mesh)
    }

    /// Saves every object shown, each where it stands, as one file (an OBJ
    /// names each object).
    fn save_file(&mut self, stl: bool) {
        let mut parts: Vec<(String, Vec<Vec3>, Vec<u32>)> = Vec::new();
        for o in self.objects.iter().filter(|o| o.visible) {
            let mesh = self.mesh_of(o);
            let tidied = tidy(mesh);
            let m = tidied.as_ref().unwrap_or(mesh);
            // The object where it stands, and its live mirror copies where they are shown.
            parts.extend(Self::placed_parts(&o.name, o, m));
        }
        if parts.is_empty() {
            self.status = "Every object is hidden: show one to save it.".into();
            return;
        }
        let (bytes, name, mime) = if stl {
            let (mut positions, mut indices) = (Vec::new(), Vec::new());
            for (_, p, i) in &parts {
                let base = positions.len() as u32;
                positions.extend_from_slice(p);
                indices.extend(i.iter().map(|&v| v + base));
            }
            (files::write_stl(&positions, &indices), "golem-sculpt.stl", "model/stl")
        } else {
            let named: Vec<(&str, &[Vec3], &[u32])> = parts.iter().map(|(n, p, i)| (n.as_str(), p.as_slice(), i.as_slice())).collect();
            // With paint or face groups, as the desktop writes them (painted_obj.rs).
            let bytes = self.painted_obj().unwrap_or_else(|| files::write_obj_objects(&named));
            (bytes, "golem-sculpt.obj", "model/obj")
        };
        browser::download(name, &bytes, mime);
        let what = if parts.len() == 1 { "1 object".to_owned() } else { format!("{} objects", parts.len()) };
        self.status = format!("Saved {name} ({what}).");
    }

    fn remesh(&mut self) {
        // A noticeably uneven Scale goes into the mesh first, so the voxels are cubes (placement.rs).
        self.deform_commit();
        self.even_scale_for_remesh();
        let began = web_time::Instant::now();
        match crate::remesh::voxel_remesh(&self.mesh, self.remesh_resolution, self.sculptor.settings.symmetry) {
            Ok(mut mesh) => {
                // The face groups and the paint go across to the new surface (groups.rs).
                groups::follow(&self.mesh, &mut mesh);
                self.replace_mesh(mesh, &format!("Remeshed in {:.1} s", began.elapsed().as_secs_f32()))
            }
            Err(e) => self.status = e,
        }
    }

    /// Decimate (the desktop's `decimate.rs`): fewer triangles, the shape
    /// kept, the mirror kept exactly, fully masked surface left alone.
    fn decimate(&mut self) {
        let tidied = self.tidied();
        let m = tidied.as_ref().unwrap_or(&self.mesh);
        let target = self.decimate_target.round() as usize;
        if m.triangle_count() <= target {
            self.status = "The model is at or under the target already.".into();
            return;
        }
        let symmetry = self.sculptor.settings.symmetry;
        let source = crate::decimate::Source::of(m).mirrored(symmetry, 1.0);
        let began = web_time::Instant::now();
        match crate::decimate::decimated(&source, target, &mut |_| true) {
            Ok(Some((mut mesh, report))) => {
                let (line, _) = crate::decimate::line("Model", &report, began.elapsed().as_secs_f64());
                groups::follow(&self.mesh, &mut mesh);
                self.replace_mesh(mesh, "Decimated");
                self.status = line;
            }
            Ok(None) => {}
            Err(e) => self.status = e,
        }
    }

    // ---- keeping the scene across a refresh ---------------------------------

    /// The scene for the browser's storage (W02): the objects, then the
    /// symmetry and the view as W01 had them.
    fn scene_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(SCENE_MAGIC_4);
        out.extend_from_slice(&(self.objects.len() as u32).to_le_bytes());
        let active = self.objects.iter().position(|o| o.id == self.active).unwrap_or(0);
        out.extend_from_slice(&(active as u32).to_le_bytes());
        for o in &self.objects {
            let name = o.name.as_bytes();
            let name = &name[..name.len().min(u16::MAX as usize)];
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(name);
            out.push(o.visible as u8);
            let p = [o.position.x, o.position.y, o.position.z, o.rotation.x, o.rotation.y, o.rotation.z, o.rotation.w, o.scale.x, o.scale.y, o.scale.z];
            out.extend_from_slice(bytemuck::cast_slice(&p));
            let mesh = self.mesh_of(o);
            let tidied = tidy(mesh);
            let m = tidied.as_ref().unwrap_or(mesh);
            out.extend_from_slice(&(m.positions.len() as u32).to_le_bytes());
            out.extend_from_slice(&(m.indices.len() as u32).to_le_bytes());
            out.extend_from_slice(bytemuck::cast_slice(&m.positions));
            out.extend_from_slice(bytemuck::cast_slice(&m.indices));
            out.extend_from_slice(bytemuck::cast_slice(&m.mask));
            // W03: the hidden faces, then the levels and layers as the desktop's file blocks.
            let hidden = m.hidden_triangles().unwrap_or(&[]);
            out.extend_from_slice(&(hidden.len() as u32).to_le_bytes());
            out.extend(hidden.iter().map(|&h| h as u8));
            let (lvls, layr) = crate::io::level_blocks(m);
            // W04: then the face groups and the paint (groups.rs, paint.rs).
            let (grps, pntl) = crate::io::paint_blocks(m);
            for block in [lvls, layr, grps, pntl] {
                out.extend_from_slice(&(block.len() as u64).to_le_bytes());
                out.extend_from_slice(&block);
            }
        }
        let s = self.sculptor.settings.symmetry;
        out.extend_from_slice(&[s[0] as u8, s[1] as u8, s[2] as u8]);
        // The view, so a restore comes back looking from where it was left.
        let c = &self.camera;
        for v in [c.yaw, c.pitch, c.distance, c.pivot.x, c.pivot.y, c.pivot.z] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(c.ortho as u8);
        // Then each object's live mirror, if any object has one: older saves end above.
        // (Written too, all off, ahead of reference pictures, which follow it.)
        let pictures = self.picture_bytes();
        if self.objects.iter().any(|o| o.mirror.is_some()) || !pictures.is_empty() {
            out.extend_from_slice(MIRRORS_TAG);
            for o in &self.objects {
                let (axes, center) = o.mirror.map_or((0, Vec3::ZERO), |m| (m.axis_mask, m.center));
                out.push(axes);
                out.extend_from_slice(bytemuck::cast_slice(&center.to_array()));
            }
        }
        out.extend_from_slice(&pictures);
        out
    }

    fn restore_scene(&mut self, bytes: &[u8]) -> Option<()> {
        let four = bytes.starts_with(SCENE_MAGIC_4);
        let (two, three, rest) = if let Some(rest) = bytes.strip_prefix(SCENE_MAGIC_4) {
            (true, true, rest)
        } else if let Some(rest) = bytes.strip_prefix(SCENE_MAGIC_3) {
            (true, true, rest)
        } else if let Some(rest) = bytes.strip_prefix(SCENE_MAGIC_2) {
            (true, false, rest)
        } else {
            (false, false, bytes.strip_prefix(SCENE_MAGIC)?)
        };
        // Meshes whose levels or layers could not be put back on them.
        let mut lost = 0;
        let mut reader = Reader { bytes: rest, at: 0 };
        let mut objects: Vec<(Props, Mesh)> = Vec::new();
        let mut active = 0;
        if two {
            let count = reader.u32()?;
            active = reader.u32()?;
            for _ in 0..count {
                let len = u16::from_le_bytes(reader.take(2)?.try_into().ok()?) as usize;
                let name = String::from_utf8_lossy(reader.take(len)?).into_owned();
                let visible = reader.take(1)?[0] != 0;
                let p: Vec<f32> = bytemuck::pod_collect_to_vec(reader.take(40)?);
                if p.iter().any(|x| !x.is_finite()) {
                    return None;
                }
                let props = Props {
                    name,
                    visible,
                    position: Vec3::new(p[0], p[1], p[2]),
                    rotation: glam::Quat::from_xyzw(p[3], p[4], p[5], p[6]).normalize(),
                    scale: Vec3::new(p[7], p[8], p[9]),
                    mirror: None,
                };
                let mesh = if three {
                    let (mesh, dropped) = reader.mesh_with_levels(four)?;
                    lost += dropped as usize;
                    mesh
                } else {
                    reader.mesh()?
                };
                objects.push((props, mesh));
            }
        } else {
            let props = Props::named("Model");
            objects.push((props, reader.mesh()?));
        }
        if objects.is_empty() {
            return None;
        }
        let symmetry = reader.take(3)?;
        // Older saves end here; newer ones carry the view.
        let view: Option<([f32; 6], bool)> = reader.take(25).map(|b| {
            let f = |k: usize| f32::from_le_bytes(b[k * 4..k * 4 + 4].try_into().unwrap());
            ([f(0), f(1), f(2), f(3), f(4), f(5)], b[24] != 0)
        });
        // The live mirrors, one for each object, where the save has them.
        let mut pictures: &[u8] = &[];
        if reader.take(MIRRORS_TAG.len()).is_some_and(|tag| tag == MIRRORS_TAG) {
            for (props, _) in objects.iter_mut() {
                let Some(b) = reader.take(13) else { break };
                let c: Vec<f32> = bytemuck::pod_collect_to_vec(&b[1..]);
                let center = Vec3::new(c[0], c[1], c[2]);
                props.mirror = if center.is_finite() { sculpt_symmetry::MirrorRepeater::new(b[0], center) } else { None };
            }
            // Then the reference pictures, if the scene has any (reference_images.rs).
            pictures = reader.bytes.get(reader.at..).unwrap_or(&[]);
        }
        self.sculptor.settings.symmetry = [symmetry[0] != 0, symmetry[1] != 0, symmetry[2] != 0];
        // The objects as kept: the active one's mesh is the app's.
        let active = active.min(objects.len() - 1);
        self.objects.clear();
        for (i, (props, mesh)) in objects.into_iter().enumerate() {
            let id = i as u64 + 1;
            let mut object = Object::new(id, String::new(), None);
            object.set_props(props);
            if i == active {
                self.mesh = mesh;
            } else {
                object.set_aside(mesh);
            }
            self.objects.push(object);
        }
        self.active = active as u64 + 1;
        self.selected = vec![self.active];
        self.next_object = self.objects.len() as u64 + 1;
        self.undo.clear();
        self.redo.clear();
        self.upload_whole = true;
        match view {
            Some((v, ortho)) if v.iter().all(|x| x.is_finite()) && v[2] > 0.0 => {
                let c = &mut self.camera;
                c.yaw = v[0];
                c.pitch = v[1];
                c.distance = v[2];
                c.pivot = Vec3::new(v[3], v[4], v[5]);
                c.ortho = ortho;
            }
            _ => self.frame_model(false),
        }
        self.status = if self.objects.len() == 1 {
            "Your last model is back. New Shape starts again.".into()
        } else {
            format!("Your last {} objects are back.", self.objects.len())
        };
        if lost > 0 {
            self.status += " Some subdivision levels or layers could not be read back, and are part of the shape now.";
        }
        self.pictures_restore(pictures);
        Some(())
    }

    fn keep_scene(&mut self, now: f64) {
        if self.drag != Drag::None {
            return;
        }
        if let Some(t) = self.changed_at {
            if now - t > STORE_AFTER {
                self.changed_at = None;
                browser::store_scene(self.scene_bytes());
            }
        }
    }

    // ---- the pointer ----------------------------------------------------------

    fn view_depth(&self) -> f32 {
        let center = self.view.origin + self.view.size * 0.5;
        let depth = self.pick(center).map_or(self.camera.distance, |(_, pos)| self.view.depth_of(pos));
        depth.min(self.camera.distance).max(1e-5)
    }

    fn sculpt_input(&mut self, ui: &egui::Ui, response: &egui::Response, ppp: f32) {
        let (events, mods, time, scroll, zoom, multi) = ui.input(|i| {
            (i.events.clone(), i.modifiers, i.time, i.smooth_scroll_delta, i.zoom_delta(), i.multi_touch())
        });
        let to_px = |p: Pos2| Vec2::new(p.x, p.y) * ppp;
        let pressure = browser::pen_pressure();
        let hovered = response.contains_pointer();
        let scene_size = self.scene_size();
        // Every pen sample since the last frame, each with its own place,
        // pressure and time (pen.rs). When the page keeps them, a stroke is
        // drawn from them rather than from egui's one move a frame.
        let queued = pen::take();
        let from_queue = queued.is_some();
        let mut queued = queued.unwrap_or_default();
        // The brush wheel or a hold key has the pointer: no press is a stroke's.
        let pointer_busy = self.brush_keys.wheel_up || self.brush_keys.gauging();
        // A second finger takes back what the first began; a tap of two or three undoes or redoes (mobile.rs).
        self.touch_frame();

        for event in &events {
            match *event {
                egui::Event::PointerButton { pos, button, pressed: true, modifiers }
                    if hovered && self.drag == Drag::None && !pointer_busy =>
                {
                    let px = to_px(pos);
                    self.pointer_px = Some(px);
                    // The press as the page saw it, with its own pressure and time.
                    let down = queued.iter().find(|s| s.down).copied();
                    let (pressure, time) = down.map_or((pressure, time), |d| (d.pressure, d.time));
                    self.press(px, button, modifiers, pressure, time);
                    if self.drag == Drag::Sculpt && pressure.is_some() {
                        self.sculptor.pen_meter.down(pressure);
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    let px = to_px(pos);
                    // S, F or D held: the move sets the brush's size, strength or focal shift.
                    if self.gauge_moved(px) {
                        continue;
                    }
                    let last = self.pointer_px.replace(px);
                    let delta = last.map_or(Vec2::ZERO, |l| px - l);
                    if self.drag == Drag::Gizmo {
                        self.transform_moved(px);
                        continue;
                    }
                    // A sphere dragged, an arrow on one, or S held (spheres.rs).
                    if self.spheres_moved(px) {
                        continue;
                    }
                    if self.mode == Mode::Tubes && self.drag != Drag::None {
                        self.tubes.pressure = pressure;
                        if self.tool_moved(px) {
                            continue;
                        }
                    }
                    match self.drag {
                        // With the page's samples to hand, they are the stroke's (below).
                        Drag::Sculpt if from_queue => {}
                        Drag::Sculpt => {
                            let sample = PointerSample { pos: px, pressure, time };
                            self.sculptor.pen_meter.moved(pressure);
                            let view = self.local_view();
                            self.sculptor.sample(&mut self.mesh, &view, sample);
                        }
                        Drag::Orbit => self.camera.orbit(delta / ppp, None),
                        Drag::Pan => {
                            let wpp = self.view.world_per_pixel(self.camera.distance);
                            self.camera.pan(delta, wpp);
                        }
                        Drag::Zoom => {
                            let factor = (1.0 + delta.y * camera::DRAG_ZOOM_PER_PX).clamp(0.5, 1.5);
                            self.camera.zoom(factor, self.view_depth(), scene_size, None);
                        }
                        Drag::Tool | Drag::Gizmo | Drag::None => {}
                    }
                }
                egui::Event::PointerButton { pressed: false, .. } => {
                    // The samples from before the lift are the stroke's last.
                    self.feed_pen(&mut queued);
                    self.release();
                }
                egui::Event::PointerGone => {
                    self.pointer_px = None;
                }
                _ => {}
            }
        }
        self.feed_pen(&mut queued);
        // A press that ended outside the page never sends its release.
        if self.drag != Drag::None && !ui.input(|i| i.pointer.any_down()) {
            self.release();
        }

        let tool_wheel = self.mode == Mode::Tubes && hovered && self.drag == Drag::None && scroll.y != 0.0;
        if tool_wheel && self.tool_wheel(scroll.y / 50.0) {
            // The tool used the wheel (sizing a tube's point).
        } else if hovered && self.spheres_wheel(&events) {
            // Over a sphere the wheel resizes it (spheres.rs).
        } else if hovered && self.drag == Drag::None {
            // Wheel and pinch zoom toward what is under the pointer.
            let anchor = self.hover.map(|h| h.pos).or_else(|| self.pointer_px.and_then(|px| self.pick(px)).map(|(_, pos)| pos));
            let mut factor = (-scroll.y * 0.0025).exp();
            if let Some(touch) = multi.filter(|t| t.num_touches >= 2) {
                factor = 1.0 / touch.zoom_delta.max(1e-3);
                let wpp = self.view.world_per_pixel(self.camera.distance);
                self.camera.pan(Vec2::new(touch.translation_delta.x, touch.translation_delta.y) * ppp, wpp);
            } else if zoom != 1.0 {
                factor = 1.0 / zoom;
            }
            if (factor - 1.0).abs() > 1e-4 {
                self.camera.zoom(factor, self.view_depth(), scene_size, anchor);
            }
            if scroll.x != 0.0 && !mods.shift {
                self.camera.orbit(Vec2::new(-scroll.x, 0.0) * 0.5, None);
            }
        }

        let view = self.local_view();
        if self.drag == Drag::Sculpt {
            let dt = (time - self.last_time).clamp(0.0, 0.1) as f32;
            self.sculptor.frame(&mut self.mesh, &view, dt);
        }

        self.hover = match (self.drag, self.pointer_px) {
            (Drag::None, Some(px)) if hovered && !matches!(self.mode, Mode::Transform | Mode::Spheres) => self.hit(px),
            _ => None,
        };
        // The brush ring sits on whichever side is under the pointer.
        if let (Some(h), Drag::None) = (self.hover, self.drag) {
            self.side = h.side;
        }
        let view = self.local_view();
        // The brush's size on the surface, in the active object's own units.
        let per_pixel = match self.hover {
            Some(h) => view.world_per_pixel(view.depth_of(h.local).max(1e-6)),
            None => {
                let mean = glam::Mat3::from_mat4(self.model()).determinant().abs().cbrt().max(1e-6);
                self.view.world_per_pixel(self.camera.distance.max(1e-6)) / mean
            }
        };
        self.sculptor.sync_size(&self.mesh, per_pixel);
    }

    fn press(&mut self, px: Vec2, button: PointerButton, mods: Modifiers, pressure: Option<f32>, time: f64) {
        self.layer_strength_done();
        self.drag = match button {
            PointerButton::Middle => Drag::Pan,
            PointerButton::Primary if mods.alt && mods.ctrl => Drag::Zoom,
            PointerButton::Primary if mods.alt && mods.shift => Drag::Pan,
            PointerButton::Primary if mods.alt => Drag::Orbit,
            // A click for the Face Groups card: Ctrl + Shift + click on a group, or one the card armed (groups.rs).
            PointerButton::Primary if self.groups_takes_press(mods) => {
                self.groups_pressed(px);
                Drag::None
            }
            PointerButton::Primary | PointerButton::Secondary if self.mode == Mode::Transform => {
                self.transform_press(px, button, mods)
            }
            PointerButton::Primary | PointerButton::Secondary if self.mode == Mode::Spheres => {
                self.spheres_press(px, button, mods)
            }
            PointerButton::Primary | PointerButton::Secondary if self.mode == Mode::Tubes => {
                self.tubes.pressure = pressure;
                self.tubes.mods = tubes::mods_of(mods);
                if self.tool_press(px, button == PointerButton::Secondary) { Drag::Tool } else { Drag::Orbit }
            }
            PointerButton::Primary | PointerButton::Secondary => {
                // The phone layout's Sub and Smooth switches count as Ctrl and Shift held (mobile.rs).
                let invert = mods.ctrl || mods.command || button == PointerButton::Secondary || self.touch.invert;
                let smooth = self.smooth_held(mods.shift);
                let sample = PointerSample { pos: px, pressure, time };
                // On the object or one of its live mirror copies: the stroke works it through that one.
                self.side = self.hit(px).map_or(0, |h| h.side);
                let view = self.local_view();
                let shown = self.active_object().visible;
                // A layer that records but does not show takes no stroke, nor
                // does Erase Layer with nothing to rub out (layers.rs).
                let on_active = shown && self.pick(px).is_some_and(|(id, _)| id == self.active);
                let refused = if on_active { self.layer_stroke_refusal(smooth) } else { None };
                // The Paint brush's Sample mode is the eyedropper, a Paint stroke needs a layer to paint
                // on (made if there is none), and Pose by face groups needs groups (paint.rs, pose.rs).
                let held = refused.is_none()
                    && on_active
                    && (self.paint_samples_press(px, smooth)
                        || self.paint_refuses_stroke(smooth)
                        || self.pose_refuses_stroke(smooth));
                if let Some(why) = refused {
                    self.status = why;
                    Drag::None
                } else if held {
                    Drag::None
                } else if shown && !self.press_is_other_objects(px) && self.begin_stroke(&view, sample, invert, smooth) {
                    self.sculptor.pace = Pace::LIVE;
                    Drag::Sculpt
                } else if button == PointerButton::Primary && mods.shift {
                    // A Shift tap here clears the mask while mask painting (masking.rs).
                    self.empty_press(px, time, true);
                    Drag::Pan
                } else {
                    // A double tap here inverts the mask or frames the model (masking.rs).
                    if button == PointerButton::Primary {
                        self.empty_press(px, time, false);
                    }
                    Drag::Orbit
                }
            }
            _ => Drag::None,
        };
        if self.drag == Drag::Orbit {
            // Turn about the model, not wherever the pivot was left.
            let center = self.view.origin + self.view.size * 0.5;
            if let Some((_, pos)) = self.pick(center) {
                self.camera.seat_pivot(pos);
            }
        }
    }

    fn release(&mut self) {
        match self.drag {
            Drag::Sculpt => {
                let view = self.local_view();
                if let Some(delta) = self.sculptor.end(&mut self.mesh, &view) {
                    self.push_undo(Step::Stroke(self.active, delta));
                    self.mark_changed();
                }
                self.sculptor.pace = Pace::Unpaced;
            }
            Drag::Gizmo => self.transform_released(),
            _ => {}
        }
        // A sphere let go of, or a click on empty space (spheres.rs).
        self.spheres_released();
        if self.mode == Mode::Transform {
            // A click on the object with Edit Pivot on puts the pivot there (placement.rs).
            self.pivot_released(self.pointer_px, self.transform_ppp());
        }
        if self.mode == Mode::Tubes {
            self.tool_released();
        }
        if matches!(self.drag, Drag::Orbit | Drag::Pan) {
            self.empty_release(self.pointer_px);
        }
        self.sculptor.pen_meter.lifted();
        self.drag = Drag::None;
    }

    /// Gives a stroke the pen samples the page kept (pen.rs), oldest first;
    /// with no stroke under way they are dropped.
    fn feed_pen(&mut self, queued: &mut Vec<pen::PenSample>) {
        if self.drag != Drag::Sculpt {
            queued.clear();
            return;
        }
        let view = self.local_view();
        for s in queued.drain(..).filter(|s| !s.down) {
            self.sculptor.pen_meter.moved(s.pressure);
            let sample = PointerSample { pos: s.pos, pressure: s.pressure, time: s.time };
            self.sculptor.sample(&mut self.mesh, &view, sample);
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        // The brush wheel and the hold keys' readout, whoever has the keyboard (brush_keys.rs).
        self.brush_wheel(ctx);
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if self.mode == Mode::Spheres {
            // 1-4, S held, F F, A, X, Delete, Esc; the brush keys are put away (spheres.rs).
            self.spheres_keys(ctx);
        } else {
            // Space, the number keys, and S, F and D held (brush_keys.rs).
            self.brush_keys(ctx);
            // P for the Paint brush, I for the eyedropper (paint.rs).
            self.paint_keys(ctx);
        }
        if self.mode == Mode::Tubes {
            self.tool_keys(ctx);
        }
        // Enter applies a deformation, Esc cancels it (deformers.rs).
        if self.mode == Mode::Transform && self.deform_keys(ctx) {
            ctx.request_repaint();
        }
        if self.mode == Mode::Transform {
            let (escape, delete) = ctx.input_mut(|i| {
                (i.consume_key(Modifiers::NONE, Key::Escape), i.consume_key(Modifiers::NONE, Key::Delete))
            });
            if escape && !self.transform_cancel() && self.drag == Drag::None {
                self.mode = Mode::Sculpt;
            }
            if delete && self.drag == Drag::None {
                self.delete_object(self.active);
            }
        }
        let shift_cmd = Modifiers::COMMAND | Modifiers::SHIFT;
        let (undo, redo) = ctx.input_mut(|i| {
            let redo = i.consume_key(shift_cmd, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y);
            (i.consume_key(Modifiers::COMMAND, Key::Z), redo)
        });
        if (undo || redo) && self.spheres_undo(redo) {
            // The spheres' own history while their tool is up (spheres.rs).
        } else if redo {
            self.undo_redo(false);
        } else if undo {
            self.undo_redo(true);
        }
        if self.mode == Mode::Sculpt && self.drag == Drag::None && !self.sculptor.is_active() {
            self.level_keys(ctx);
        }
        let viewport_h = self.view.size.y;
        let mut picked_tool = None;
        let mut toggle_transform = false;
        // Each tool's desktop key: T Tube, B Muscle, E Strap, Y Array, N Mask, K Split, L Measure.
        let hotkeys = self.tubes.hotkeys();
        ctx.input_mut(|i| {
            // F is Strength held, and frames the model tapped twice (brush_keys.rs).
            if i.consume_key(Modifiers::NONE, Key::F) && self.mode != Mode::Sculpt {
                self.frame_model(true);
            }
            if i.consume_key(Modifiers::NONE, Key::X) {
                let s = &mut self.sculptor.settings.symmetry;
                s[0] = !s[0];
            }
            if self.drag == Drag::None {
                if i.consume_key(Modifiers::NONE, Key::W) {
                    toggle_transform = true;
                }
                if i.consume_key(Modifiers::NONE, Key::M) && self.mode == Mode::Sculpt {
                    let s = &mut self.sculptor.settings;
                    s.mask_mode = !s.mask_mode;
                }
                for &(key, tool) in &hotkeys {
                    if i.consume_key(Modifiers::NONE, key) {
                        picked_tool = Some(tool);
                    }
                }
            }
            let s = &mut self.sculptor.settings;
            if i.consume_key(Modifiers::NONE, Key::OpenBracket) {
                s.set_size(s.size() / 1.15, viewport_h);
            }
            if i.consume_key(Modifiers::NONE, Key::CloseBracket) {
                s.set_size(s.size() * 1.15, viewport_h);
            }
        });
        if let Some(tool) = picked_tool {
            self.pick_tool(tool);
        }
        if toggle_transform {
            self.toggle_transform();
        }
    }

    /// The keys a tool listens for: Delete a point, Enter to merge, Esc to
    /// cancel, S held to size, U for Measure's unit, the digits (back to
    /// sculpting from a region tool), the tool's own key (a region tool's
    /// next shape) and, while it drags, Space (slide the shape). A key the
    /// tool takes is the tool's alone. Ctrl shortcuts stay the app's.
    fn tool_keys(&mut self, ctx: &egui::Context) {
        let own = self.tubes.hotkey();
        let dragging = self.drag == Drag::Tool;
        let keys: Vec<(Key, bool, Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match *e {
                    egui::Event::Key { key, pressed, repeat: false, modifiers, .. } if !modifiers.command => {
                        let wanted = matches!(key, Key::Delete | Key::Backspace | Key::Enter | Key::Escape | Key::S | Key::U)
                            || tubes::is_digit(key)
                            || Some(key) == own
                            || (key == Key::Space && dragging);
                        wanted.then_some((key, pressed, modifiers))
                    }
                    _ => None,
                })
                .collect()
        });
        for (key, down, modifiers) in keys {
            let used = self.tool_key(key, down);
            if used && down {
                ctx.input_mut(|i| i.consume_key(modifiers, key));
            }
            if !used && down && key == Key::Escape && self.drag == Drag::None {
                self.mode = Mode::Sculpt;
            }
        }
    }

    // ---- drawing --------------------------------------------------------------

    /// What changed since the card last had the mesh, ready to send.
    ///
    /// eframe runs the app's logic once for every press and release, and a
    /// pass whose upload is never painted (two of them before one paint, say:
    /// a quick double tap) throws its upload away. The changes it carried
    /// were already taken from the mesh, so they are kept here as `unsent`
    /// and sent again with every upload until the card is seen to take one
    /// that had them. Sending a range twice is harmless: it always carries
    /// the mesh as it is now.
    fn upload(&mut self) -> Option<Arc<Upload>> {
        self.upload_seq += 1;
        let took = self.card_took.load(Ordering::Relaxed);
        self.unsent.retain(|u| u.seq > took);
        // Cavity reads the mesh's own curvature, kept only while it is on.
        self.mesh.track_curvature(self.wants_curvature());
        let dirty = self.mesh.take_dirty();
        let lost = self.card_has.load(Ordering::Relaxed) != self.generation;
        let (vertices, corners) = (self.mesh.positions.len(), self.mesh.indices.len());
        let fits = vertices <= self.card_room.0 && corners <= self.card_room.1;
        // Hidden faces are left out of the triangles sent, so the card's
        // triangles no longer line up with the mesh's: a hide or show, or a
        // change of triangles while some are hidden, sends them all again.
        let visibility = self.mesh.visibility_version();
        let faces_moved = visibility != self.tubes.sent_visibility
            || (self.mesh.any_hidden() && (corners != self.card_sent.1 || !dirty.triangles.is_empty()));
        if self.upload_whole || lost || !fits || faces_moved {
            self.upload_whole = false;
            self.tubes.sent_visibility = visibility;
            self.generation += 1;
            // With dynamic topology on, room to grow, so a stroke that adds
            // detail sends only what it added.
            let room = |n: usize| if self.sculptor.dyntopo.enabled { n + n / 2 + 4096 } else { n };
            self.card_room = (room(vertices), room(corners));
            self.card_sent = (vertices, corners);
            // A whole mesh carries every change there is.
            self.unsent.clear();
            let m = &self.mesh;
            let shown = m.visible_indices();
            return Some(Arc::new(Upload::Whole {
                generation: self.generation,
                positions: bytemuck::cast_slice(&m.positions).to_vec(),
                normals: bytemuck::cast_slice(&m.normals).to_vec(),
                mask: bytemuck::cast_slice(&m.mask).to_vec(),
                curv: bytemuck::cast_slice(&m.curv).to_vec(),
                indices: bytemuck::cast_slice(&shown).to_vec(),
                index_count: shown.len() as u32,
                vertex_room: self.card_room.0 as u64,
                index_room: self.card_room.1 as u64,
                // The paint's colour and the face groups' tint (paint.rs, groups.rs).
                paint: paint::colour_bytes(m, 0..m.positions.len()),
                groups: groups::colours_to_send(m, true).unwrap_or_default(),
            }));
        }
        // New vertices go up whole, whatever the dirty ranges say of them.
        let grown = |ranges: &[std::ops::Range<u32>]| {
            let mut ranges = ranges.to_vec();
            if vertices > self.card_sent.0 {
                ranges.push(self.card_sent.0 as u32..vertices as u32);
            }
            ranges
        };
        let (positions, normals, mask) = (grown(&dirty.positions), grown(&dirty.normals), grown(&dirty.mask));
        let curv = if self.wants_curvature() { grown(&dirty.curv) } else { Vec::new() };
        let corners_changed = corners != self.card_sent.1;
        // The face groups' tint goes whole, when it changed (groups.rs).
        let groups_now = groups::colours_to_send(&self.mesh, false);
        let paint = if self.mesh.has_paint() { grown(&dirty.paint) } else { Vec::new() };
        let new = Unsent {
            seq: self.upload_seq,
            positions,
            normals,
            mask,
            curv,
            triangles: dirty.triangles,
            paint,
            groups: groups_now.is_some(),
        };
        if !new.is_empty() || corners_changed {
            self.unsent.push(new);
        }
        if self.unsent.is_empty() {
            return None;
        }
        let all = |pick: fn(&Unsent) -> &Vec<std::ops::Range<u32>>| -> Vec<std::ops::Range<u32>> {
            self.unsent.iter().flat_map(|u| pick(u).iter().cloned()).collect()
        };
        let (positions, normals, mask, curv, triangles) =
            (all(|u| &u.positions), all(|u| &u.normals), all(|u| &u.mask), all(|u| &u.curv), all(|u| &u.triangles));
        let paint_ranges = all(|u| &u.paint);
        let groups_due = self.unsent.iter().any(|u| u.groups);
        fn parts<T: bytemuck::Pod>(data: &[T], ranges: &[std::ops::Range<u32>], per: usize) -> Vec<(u64, Vec<u8>)> {
            let size = std::mem::size_of::<T>();
            ranges
                .iter()
                .filter_map(|r| {
                    let (start, end) = (r.start as usize * per, (r.end as usize * per).min(data.len()));
                    let part = data.get(start..end).filter(|p| !p.is_empty())?;
                    Some(((start * size) as u64, bytemuck::cast_slice(part).to_vec()))
                })
                .collect()
        }
        self.card_sent = (vertices, corners);
        let m = &self.mesh;
        Some(Arc::new(Upload::Parts {
            generation: self.generation,
            positions: parts(&m.positions, &positions, 1),
            normals: parts(&m.normals, &normals, 1),
            mask: parts(&m.mask, &mask, 1),
            curv: parts(&m.curv, &curv, 1),
            // Triangle ranges, three corners each.
            indices: parts(&m.indices, &triangles, 3),
            // Always the count as it is now: the upload that changed it may be one that was lost.
            index_count: Some(corners as u32),
            paint: paint_ranges
                .iter()
                .map(|r| (r.start as u64 * 4, paint::colour_bytes(m, r.start as usize..r.end as usize)))
                .filter(|(_, bytes)| !bytes.is_empty())
                .collect(),
            groups: if groups_due { groups_now.or_else(|| groups::colours_to_send(m, true)) } else { None },
        }))
    }

    /// The brush ring, as the desktop draws it (`app.rs` `cursor` and
    /// `ui/overlay.rs` `cursor`): a flat ring facing the screen at the
    /// pointer, with a dark halo, green when the brush builds, red when it
    /// digs (Ctrl, the right button, or Sub), blue and dashed for Smooth,
    /// orange for the mask; a fainter inner ring where the brush is at half
    /// weight; a dot in the middle. During a stroke it is the size the
    /// stroke works at, where the brush is. S, F and D held draw their own
    /// (`brush_keys.rs`).
    fn draw_cursor(&self, ui: &egui::Ui, hovered: bool, ppp: f32) {
        use crate::sculpt::Brush;
        const ADD: Color32 = Color32::from_rgb(51, 255, 102);
        const SUBTRACT: Color32 = Color32::from_rgb(255, 76, 76);
        const SMOOTH: Color32 = Color32::from_rgb(76, 166, 255);
        const MASK: Color32 = Color32::from_rgb(242, 166, 38);

        let stroking = self.sculptor.stroke_cursor();
        let navigating = matches!(self.drag, Drag::Orbit | Drag::Pan | Drag::Zoom);
        let shown = stroking.is_some()
            || (self.mode == Mode::Sculpt && hovered && self.drag == Drag::None && self.pointer_px.is_some());
        if !shown || navigating || self.brush_keys.wheel_up || self.brush_keys.gauging() {
            return;
        }
        // The brush's alpha, faintly inside the ring (alpha.rs).
        let painter = ui.painter();
        self.paint_alpha_preview(painter, ppp);

        let (mods, right) = ui.input(|i| (i.modifiers, i.pointer.secondary_down()));
        let s = &self.sculptor.settings;
        let brush = self.sculptor.effective_brush(self.smooth_held(mods.shift));
        // With Shift held the ring is the brush Shift smooths with.
        let shift = self.smooth_held(mods.shift) && !s.mask_mode;
        let (bs, _) = s.in_use(brush, shift);
        let mut negative = !bs.positive && brush.has_polarity();
        if (mods.ctrl || mods.command || right) && (brush.has_polarity() || s.mask_mode) {
            negative = !negative;
        }
        let mut tint = if s.mask_mode {
            if negative { SUBTRACT } else { MASK }
        } else if brush == Brush::Smooth {
            SMOOTH
        } else if negative {
            SUBTRACT
        } else {
            ADD
        };

        let mut center = self.pointer_px.unwrap_or_default();
        let mut radius = s.radius_px;
        let mut tether = None;
        if let Some((rope, world_radius, sign)) = stroking {
            // During a stroke the world size is frozen: show it where the brush is.
            if rope.distance(center) > 1.0 && self.pointer_px.is_some() {
                tether = Some(center);
            }
            center = rope;
            let view = self.local_view();
            let depth = self.sculptor.last_hit.map_or(self.camera.distance, |h| view.depth_of(h));
            radius = world_radius / view.world_per_pixel(depth.max(1e-6));
            if sign < 0.0 && brush.has_polarity() {
                tint = SUBTRACT;
            }
        }
        let to_pt = |px: Vec2| Pos2::new(px.x / ppp, px.y / ppp);
        let c = to_pt(center);
        let r = (radius / ppp).max(7.0);
        let halo = Line::new(4.0, Color32::from_black_alpha(110));
        let ring = Line::new(1.75, tint);
        if brush == Brush::Smooth && !s.mask_mode {
            let dashes = 14;
            for i in 0..dashes {
                let a0 = i as f32 / dashes as f32 * std::f32::consts::TAU;
                let a1 = a0 + 0.6 / dashes as f32 * std::f32::consts::TAU;
                let arc: Vec<Pos2> =
                    (0..=6).map(|k| { let a = a0 + (a1 - a0) * k as f32 / 6.0; c + egui::vec2(a.cos(), a.sin()) * r }).collect();
                painter.line(arc.clone(), halo);
                painter.line(arc, ring);
            }
        } else {
            painter.circle_stroke(c, r, halo);
            painter.circle_stroke(c, r, ring);
        }
        let inner = if s.mask_mode {
            s.mask_hardness + (1.0 - s.mask_hardness) * 0.5
        } else {
            s.half_weight_radius_in_use(brush, shift)
        };
        if inner > 0.0 && inner < 0.99 {
            painter.circle_stroke(c, (r * inner).max(4.0), Line::new(1.5, tint.gamma_multiply(0.6)));
        }
        painter.circle_filled(c, 2.0, tint);
        if let Some(raw) = tether {
            let raw = to_pt(raw);
            painter.line_segment([c, raw], Line::new(1.5, tint.gamma_multiply(0.5)));
            painter.circle_filled(raw, 2.5, tint.gamma_multiply(0.5));
        }
    }
}

/// Ranges of the mesh sent in parts (vertices, and triangles for `triangles`).
struct Unsent {
    seq: u64,
    positions: Vec<std::ops::Range<u32>>,
    normals: Vec<std::ops::Range<u32>>,
    mask: Vec<std::ops::Range<u32>>,
    curv: Vec<std::ops::Range<u32>>,
    triangles: Vec<std::ops::Range<u32>>,
    /// Vertices whose paint changed, and whether the face groups' tint did.
    paint: Vec<std::ops::Range<u32>>,
    groups: bool,
}

impl Unsent {
    fn is_empty(&self) -> bool {
        self.positions.is_empty()
            && self.normals.is_empty()
            && self.mask.is_empty()
            && self.curv.is_empty()
            && self.triangles.is_empty()
            && self.paint.is_empty()
            && !self.groups
    }
}

/// `mesh` without what dynamic topology's merges leave inside it (triangles
/// shrunk to points, vertices no triangle uses), when it has any: what goes
/// to a file or the browser's storage, as on the desktop (`scene::tidied`).
fn tidy(mesh: &Mesh) -> Option<Mesh> {
    mesh.has_tombstones().then(|| copy_of(mesh))
}

/// A copy of `mesh`, numbered afresh, with its hidden faces, face groups,
/// subdivision levels and sculpt layers carried through the renumbering
/// (the desktop's `scene::copy_of`).
pub(crate) fn copy_of(mesh: &Mesh) -> Mesh {
    let hidden = mesh.hidden_triangles().unwrap_or(&[]);
    let (mut copy, numbering) = Mesh::new_numbered(mesh.positions.clone(), mesh.indices.clone(), Some(mesh.mask.clone()), hidden);
    copy.levels = mesh.levels.as_ref().and_then(|levels| levels.renumbered(&copy, &numbering));
    copy.layers = mesh.layers_renumbered(&numbering.vertex);
    copy.set_group_ids(crate::mesh::groups::renumbered(mesh.group_ids(), &numbering.triangle));
    // The paint goes through the renumbering too (paint.rs).
    copy.paint = mesh.paint_renumbered(&numbering.vertex, copy.vertex_count());
    copy
}

/// Reads the scene kept in the browser, part by part.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let part = self.bytes.get(self.at..self.at.checked_add(len)?);
        self.at += len;
        part
    }

    fn u32(&mut self) -> Option<usize> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?) as usize)
    }

    /// One mesh: its counts, positions, triangles and mask.
    fn mesh(&mut self) -> Option<Mesh> {
        let n = self.u32()?;
        let i = self.u32()?;
        let positions: Vec<Vec3> = bytemuck::pod_collect_to_vec(self.take(n.checked_mul(12)?)?);
        let indices: Vec<u32> = bytemuck::pod_collect_to_vec(self.take(i.checked_mul(4)?)?);
        let mask: Vec<f32> = bytemuck::pod_collect_to_vec(self.take(n * 4)?);
        if indices.is_empty() || indices.iter().any(|&v| v as usize >= n) {
            return None;
        }
        Some(Mesh::new(positions, indices, Some(mask)))
    }

    /// One mesh as W03 keeps it: as `mesh`, then its hidden faces, levels
    /// and layers (and with `four`, as W04, its face groups and paint). The
    /// second value: some of those could not be put back.
    fn mesh_with_levels(&mut self, four: bool) -> Option<(Mesh, bool)> {
        let n = self.u32()?;
        let i = self.u32()?;
        let positions: Vec<Vec3> = bytemuck::pod_collect_to_vec(self.take(n.checked_mul(12)?)?);
        let indices: Vec<u32> = bytemuck::pod_collect_to_vec(self.take(i.checked_mul(4)?)?);
        let mask: Vec<f32> = bytemuck::pod_collect_to_vec(self.take(n * 4)?);
        if indices.is_empty() || indices.iter().any(|&v| v as usize >= n) {
            return None;
        }
        let h = self.u32()?;
        let hidden: Vec<bool> = self.take(h)?.iter().map(|&b| b != 0).collect();
        let lvls = self.block()?;
        let layr = self.block()?;
        let (grps, pntl) = if four { (self.block()?, self.block()?) } else { (&[][..], &[][..]) };
        Some(crate::io::build_mesh_painted(positions, indices, mask, hidden, lvls, layr, grps, pntl))
    }

    /// A length (u64) and that many bytes.
    fn block(&mut self) -> Option<&'a [u8]> {
        let len = u64::from_le_bytes(self.take(8)?.try_into().ok()?);
        self.take(usize::try_from(len).ok()?)
    }
}

fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl eframe::App for GolemApp {
    /// Each key pressed becomes what the key map has it on (shortcuts.rs).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // A pinch is one press, not one a finger (mobile.rs).
        if self.touch.filter(raw_input) {
            ctx.request_repaint();
        }
        self.shortcut_keys(ctx, raw_input);
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        let now = ctx.input(|i| i.time);

        // A scene kept from the last visit, once the browser has found it.
        if let Some(slot) = &self.stored {
            let found = slot.borrow_mut().take();
            if let Some(found) = found {
                self.stored = None;
                if let Some(bytes) = found {
                    if self.restore_scene(&bytes).is_none() {
                        self.status = "The model kept from last time could not be read; starting fresh.".into();
                    }
                }
            }
        }
        // A file from the picker, or dropped on the page.
        let dropped = ctx.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        if let Some(file) = dropped.into_iter().next() {
            self.picked = Some(browser::read_dropped(file));
        }
        let picked = self.picked.as_ref().and_then(|slot| slot.borrow_mut().take());
        if let Some((name, bytes)) = picked {
            self.picked = None;
            self.open_file(&name, &bytes);
        }
        if self.picked.is_some() || self.stored.is_some() {
            // The browser hands the file over between frames: look again soon.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // The cards over the page, as they stand before this frame's input.
        let cards_were = (self.welcome.open, self.pending_scene.is_some());
        // "Open this scene?" first, so its Enter and Escape are its own.
        self.scene_question(ctx);
        self.keys(ctx);
        self.shortcut_commands();

        // The panels float over the viewport, as on the desktop. They come
        // first, so what is changed on them is drawn in this same frame.
        self.interface(root);
        // The first visit's language question comes before the welcome card.
        if !self.languages(ctx) {
            self.welcome(ctx);
        }
        // A card that closed (or opened) needs a frame drawn without (with)
        // it at once: egui finds what a click lands on from the last frame
        // drawn, so otherwise the next click lands on the card's invisible
        // backdrop, and a press on a tab or button seems to do nothing.
        if (self.welcome.open, self.pending_scene.is_some()) != cards_were {
            ctx.request_repaint();
        }
        // A Strength slider the card stopped drawing while held is let go.
        self.layers_watch();
        // An Opacity slider let go unseen; the picked group's count; paint or groups left behind.
        self.paint_watch();
        self.groups_watch();

        egui::CentralPanel::no_frame().show(root, |ui| {
            let ppp = ctx.pixels_per_point();
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
            let origin = Vec2::new(rect.min.x, rect.min.y) * ppp;
            let size = Vec2::new(rect.width(), rect.height()) * ppp;
            let dt = (now - self.last_time).clamp(0.0, 0.1) as f32;
            let animating = self.camera.update(dt);
            let scene_size = self.scene_size();
            self.view = self.camera.view(origin, size, self.view_depth(), scene_size);

            let hovering = response.contains_pointer() && (self.drag == Drag::None || self.tool_holds_drag());
            let t = &mut self.tubes;
            (t.ppp, t.time, t.dt, t.hovering) = (ppp, now, dt, hovering);
            t.mods = tubes::mods_of(ui.input(|i| i.modifiers));
            if let Some(p) = response.hover_pos() {
                t.pointer = Vec2::new(p.x, p.y) * ppp;
            }
            self.sculpt_input(ui, &response, ppp);
            // The pointer may have moved the camera: draw from where it is now.
            self.view = self.camera.view(origin, size, self.view_depth(), scene_size);
            self.tubes_frame(self.mode == Mode::Tubes);
            // The spheres follow the pointer and keep their skin current (spheres.rs).
            let alt = ui.input(|i| i.modifiers.alt);
            self.spheres_frame(response.contains_pointer(), alt, now);

            // A masked gizmo drag reshapes the surface once a frame, before it is sent.
            self.transform_apply();
            // The Deform card's session, the pivot, Quad Remesh at work (shaping.rs).
            self.shaping_frame(ctx, now);
            // The globals, what changed of the mesh, the runs in view (look.rs).
            let viewport = self.viewport(ctx, size);
            ui.painter().add(render::callback(rect, viewport));
            self.draw_cursor(ui, response.contains_pointer(), ppp);
            // Where the Pose brush will bend the limb (pose.rs).
            self.draw_pose(ctx);
            self.draw_transform(ui.painter(), ppp);
            self.draw_spheres_gizmo(ui.painter(), ppp);
            tubes::paint_overlay(ui.painter(), &self.tubes.draw, self.view.origin, ppp);

            if self.drag != Drag::None || animating || self.sculptor.is_active() || self.mode != Mode::Sculpt {
                ctx.request_repaint();
            }
        });

        self.keep_scene(now);
        if self.changed_at.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(STORE_AFTER + 0.2));
        }
        self.last_time = now;
    }
}
