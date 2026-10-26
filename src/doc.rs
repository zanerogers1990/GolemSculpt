//! The open document and the options the UI edits, plus the commands the UI
//! can ask the app to carry out.

use std::path::PathBuf;

use crate::camera::Camera;
use crate::history::History;
use crate::look::Look;
use crate::mesh::Mesh;
use crate::mesh::primitives::Primitive;
use crate::scene::Scene;
use crate::sculpt::Sculptor;
use crate::ssphere::SSpheres;

pub struct Options {
    pub matcap_on: bool,
    pub matcap: usize,
    pub flat_shading: bool,
    /// Draw the inside of the surface too, where a hole or a hidden face
    /// lets it be seen. Off, faces turned away from the camera are left out,
    /// which halves the work of drawing a closed model.
    pub show_backfaces: bool,
    /// Draw the edges of every face over the model.
    pub wireframe: bool,
    pub cavity: bool,
    pub cavity_ridge: f32,
    pub cavity_valley: f32,
    pub base_color: [f32; 3],
    pub smoothness: f32,
    pub gradient_background: bool,
    pub background: [f32; 3],
    pub background_top: [f32; 3],
    pub symmetry_guides: bool,
    /// The Symmetry card's Match Tolerance: how far apart two vertices may be
    /// and still count as a mirrored pair, as a multiple of what the mesh
    /// itself suggests (see app/symmetry_repair.rs). Not kept between runs.
    pub symmetry_tolerance: f32,
    /// Orbit and zoom around the last sculpted point.
    pub stroke_pivot: bool,
    pub tooltips: bool,
    pub ui_scale: f32,
    /// Voxels along the longest side for a voxel remesh.
    pub remesh_resolution: f32,
    /// The Remesh card's Smooth, 0 to `remesh::MAX_SMOOTH` in whole steps:
    /// how much a remesh irons out (see `remesh::smoothed`).
    pub remesh_smooth: f32,
    /// Quads a quad remesh aims for.
    pub quad_target: f32,
    /// Triangles Decimate brings the selected object down to.
    pub decimate_target: f32,
    /// What the Remesh card shows of a Decimate preview; the app sets it
    /// every frame (see ui/decimate.rs). Not kept between runs.
    pub decimate_preview: crate::ui::decimate::Preview,
    /// Subdivide only cuts faces up, moving nothing, instead of smoothing.
    /// Not kept between runs: smooth is what is nearly always wanted.
    pub subdivide_flat: bool,
    /// The object Project Detail takes detail from; `None` (or one that has
    /// gone) for the one overlapping the active object most (app/project.rs).
    /// Not kept between runs.
    pub project_from: Option<u64>,
    /// How far Project Detail may move a point, as a share of the active
    /// object's size. Not kept between runs.
    pub project_reach: f32,
    /// Triangle count for new shapes.
    pub new_shape_tris: f32,
    /// Tab: hide every panel.
    pub clean_view: bool,
    /// The Sculpt panel (left) and the Scene panel (right) folded away to a
    /// tab at the edge of the screen. Unlike the clean view this hides
    /// nothing else and switches nothing off.
    pub folded: [bool; 2],
    /// Which tab of each panel is showing.
    pub tabs: crate::ui::tabs::Showing,
    /// The interface's colours: a place in `ui::theme::THEMES`.
    pub theme: usize,
    /// Which way the transform gizmo's arrows and rings point.
    pub gizmo_axes: crate::gizmo::Orientation,
    /// Finishes, lighting, environment and post effects.
    pub look: Look,
    /// How hard the renderer works (see render/quality.rs). Kept in the
    /// settings file, not the scene.
    pub quality: crate::render::quality::Quality,
    /// The Face Groups card: whether group colours show, and the group picked (see ui/groups.rs).
    pub groups: crate::ui::groups::Card,
    /// The Deform card: the deformer picked and its settings (see ui/deform.rs). Not kept between runs.
    pub deform: crate::ui::deform::Card,
    /// The scene's material palette for painting, kept in the file (see mesh/paint/palette.rs).
    pub paint: crate::mesh::paint::palette::ScenePaint,
    /// Export FBX also writes each painted object's material weights into
    /// its UV sets, for game shaders (see io/fbx/paint.rs). Not kept between runs.
    pub fbx_material_weights: bool,
    /// The material library and the custom shaders, which the Materials card
    /// edits (see materials/mod.rs). Not part of the scene.
    pub materials: crate::materials::State,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            matcap_on: true,
            matcap: 0,
            flat_shading: false,
            show_backfaces: false,
            wireframe: false,
            cavity: true,
            cavity_ridge: 1.0,
            cavity_valley: 1.0,
            base_color: [0.65, 0.65, 0.68],
            smoothness: 0.4,
            gradient_background: false,
            background: [0.10, 0.11, 0.14],
            background_top: [0.35, 0.42, 0.55],
            symmetry_guides: true,
            symmetry_tolerance: crate::app::symmetry_repair::DEFAULT_TOLERANCE,
            stroke_pivot: true,
            tooltips: true,
            ui_scale: 1.0,
            remesh_resolution: 128.0,
            remesh_smooth: crate::remesh::DEFAULT_SMOOTH,
            quad_target: 5000.0,
            decimate_target: 200_000.0,
            decimate_preview: Default::default(),
            subdivide_flat: false,
            new_shape_tris: 200_000.0,
            clean_view: false,
            folded: [false; 2],
            tabs: Default::default(),
            project_from: None,
            project_reach: crate::project::DEFAULT_REACH,
            theme: 0,
            gizmo_axes: Default::default(),
            look: Look::default(),
            quality: Default::default(),
            groups: Default::default(),
            deform: Default::default(),
            paint: Default::default(),
            fbx_material_weights: false,
            materials: Default::default(),
        }
    }
}

#[derive(Default)]
pub struct Stats {
    pub frame_ms: f32,
    pub sculpt_ms: f32,
    pub upload_kb: f32,
    pub adapter: String,
}

pub struct Doc {
    /// The mesh of the active object. The other objects' meshes are in
    /// `scene`.
    pub mesh: Mesh,
    pub scene: Scene,
    /// Set when the mesh object was replaced, so the renderer rebuilds its
    /// buffers instead of patching them.
    pub mesh_replaced: bool,
    pub history: History,
    pub sculptor: Sculptor,
    pub camera: Camera,
    pub opts: Options,
    pub path: Option<PathBuf>,
    pub stats: Stats,
    /// What the pointer does in the viewport.
    pub tool: Tool,
    /// The SSphere blockout. It lives beside the scene, not in it, until
    /// Make Skin turns it into an object.
    pub spheres: SSpheres,
    /// The tools written against `sculpt_toolkit`, and what the app keeps for them.
    pub tools: crate::app::tools::Tools,
    /// The turntable: its spin, its settings and how a loop recording is going.
    pub turntable: crate::turntable::Turntable,
    /// The timelapse recorder's settings and status.
    pub timelapse: crate::timelapse::Timelapse,
    /// Render Picture's settings and how a render is going (still.rs).
    pub still: crate::still::Still,
}

impl Doc {
    /// Whether Z has anything to step back: the spheres' own history while
    /// that tool is up, then the scene's.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
            || (self.tool == Tool::SSpheres && self.spheres.can_undo())
            || (self.tool == Tool::Plugin && self.tools.can_undo())
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
            || (self.tool == Tool::SSpheres && self.spheres.can_redo())
            || (self.tool == Tool::Plugin && self.tools.can_redo())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Sculpt,
    /// Move, turn and scale whole objects with the gizmo.
    Transform,
    SSpheres,
    /// One of the hosted tools (`Doc::tools` says which); see app/tools.rs.
    Plugin,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MaskOp {
    Invert,
    Clear,
    Blur,
    Sharpen,
    Grow,
    Shrink,
}

/// How a chosen `.sclp` file is brought in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenHow {
    /// In place of the scene.
    Replace,
    /// The same, once the scene that is open has been saved.
    SaveThenReplace,
    /// Its objects join the scene; nothing else of the scene changes.
    Add,
}

/// What to do with the copy of the scene a crash left behind (see
/// app/recovery.rs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Recover {
    /// Bring back the newest copy.
    Newest,
    /// Bring back the last regular copy, not the one made as the app stopped.
    Earlier,
    /// Leave it out; its files go once this run has saved or closed.
    Discard,
}

/// What the Layers card can ask for. A layer is named by its number
/// (`mesh::layers::LayerRow::id`), which no other layer of the mesh ever has.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerAction {
    /// A new, empty layer at the bottom of the list, recording: under this
    /// name, or "Layer 1" or the next such name that is free.
    New(Option<String>),
    Rename(u32, String),
    /// Record strokes on this layer, or with `None` on none: strokes then
    /// change the model underneath.
    Record(Option<u32>),
    /// The Strength slider is held at this many percent. Sent every frame it
    /// is held; the whole drag is one undo step, closed by `StrengthDone`.
    Strength(u32, u8),
    /// The Strength slider was let go.
    StrengthDone,
    /// The eye: show the layer in the shape, or leave it out.
    Show(u32, bool),
    /// Take the layer and what it added away.
    Delete(u32),
    /// Make what the layer adds, as it shows now, part of the model, and
    /// forget the layer.
    Bake(u32),
    BakeAll,
}

/// What the Paint Layers card, the Paint card and the Masking card's paint
/// commands can ask for (see app/paint.rs). A layer is named by its number
/// (`mesh::paint::PaintRow::id`), which no other layer of the mesh ever has.
/// Each is one undo step.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintAction {
    /// A new, empty layer above the active one, and active.
    New,
    Delete(u32),
    Rename(u32, String),
    /// Strokes and fills go to this layer from now on.
    Select(u32),
    /// Move the layer to this place in the stack, counted from the bottom.
    Move(u32, usize),
    Duplicate(u32),
    MergeDown(u32),
    FlattenAll,
    /// The layer's settings as the card has them now: the eye, the lock,
    /// the blend modes, the channel toggles, the mask source.
    Row(crate::mesh::paint::PaintRow),
    /// The Opacity slider is held at this. Sent every frame it is held; the
    /// whole drag is one undo step, closed by `OpacityDone`.
    Opacity(u32, u8),
    OpacityDone,
    /// Fill the active layer with the Paint card's colour (`colour`) and the
    /// scene's material to paint with (`material`), over `area`.
    Fill { area: crate::mesh::paint::FillArea, colour: bool, material: bool },
    /// Fill over the face group picked on the model (the Face Groups card).
    FillPickedGroup { colour: bool, material: bool },
    /// Empty the layer of its paint.
    Clear(u32),
    /// Take the layer's cavity snapshot again from the surface as it is.
    RecomputeCavity(u32),
    /// Mask the layer by the face group picked on the model.
    MaskByPickedGroup(u32),
    /// The eyedropper at this window position, in pixels.
    PickAt(glam::Vec2),
    /// Masking card: mask where the paint is near the Paint card's colour.
    MaskByColour,
    /// Masking card: mask where the scene's material to paint with is painted.
    MaskByMaterial,
    /// Masking card: mask where the active layer has paint.
    MaskByLayer,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Undo,
    Redo,
    /// Add a new shape to the scene.
    New(Primitive),
    SelectObject(u64),
    /// Add an object to the selection, leaving the active one as it is.
    SelectAlso(u64),
    /// Merge the selected objects into the active one; `remesh` rebuilds the
    /// result as one surface at the Remesh resolution.
    Join { remesh: bool },
    /// Cut the active object with the other selected objects, weld them
    /// into it, or keep what they all share, at this voxel resolution;
    /// `delete` removes the others afterwards instead of hiding them.
    Boolean { op: sculpt_volume::BooleanOp, resolution: u32, delete: bool },
    /// Put the active mesh back to the shape it had before it was sculpted.
    ResetShape,
    ToggleVisible(u64),
    RenameObject(u64, String),
    DeleteObject(u64),
    /// Move an object to this row of the list.
    MoveObject(u64, usize),
    /// Independent copy of the active object.
    CloneObject,
    /// Linked copy of the active object, sharing its mesh.
    InstanceObject,
    /// Give the active instance its own mesh.
    MakeUnique,
    /// The New Scene button, or Ctrl + N: a fresh scene in place of this one,
    /// asking first if that would cost work (see app/new_scene.rs).
    NewScene,
    /// The answer to that question: go ahead, saving the scene first if
    /// `save` says so.
    NewSceneAs { save: bool },
    Open,
    Save,
    SaveAs,
    Import,
    /// Open one of the recent files (a tile of the File card).
    OpenRecent(PathBuf),
    /// Bring a chosen `.sclp` file in, the way its question was answered.
    OpenAs(PathBuf, OpenHow),
    /// The Exit button: what the window's close button does, which asks
    /// first if closing would cost work.
    Exit,
    /// The answer to that question: close the app, saving the scene first
    /// if `save` says so.
    Quit { save: bool },
    /// The answer to the question about a copy a crash left behind.
    Recover(Recover),
    /// The File tab's Older Versions: the earlier saves of the scene and the
    /// recovery copies kept for a week (see app/older.rs).
    OlderVersions,
    /// One of those: its objects added to the scene (`add`), or the whole of
    /// it in place of the scene, with no file of its own.
    OpenOlder { file: PathBuf, add: bool },
    /// Show a folder in Explorer.
    ShowFolder(PathBuf),
    ExportObj,
    /// Export PLY: the active object, with its paint (see app/exports.rs).
    ExportPly,
    ExportFbx,
    /// Send the selected objects to Blender, opening it if it is not open
    /// (see app/blender.rs).
    SendToBlender,
    /// Export STL for Printing: checks the object first, and asks before
    /// writing one that is not a closed surface.
    ExportStl,
    /// The answer "Export Anyway" to that question: straight to the save box.
    ExportStlAnyway,
    /// Say whether the selected object is a closed surface a printer can use.
    CheckForPrinting,
    Remesh,
    QuadRemesh,
    /// The Decimate button: bring the active object down to the Target
    /// Triangles of the Remesh card, asking first if its radial symmetry
    /// would no longer be exact (see app/decimate.rs). With the preview
    /// open, it keeps what the preview shows.
    Decimate,
    /// The answer "Decimate Anyway" to that question: straight to the work.
    DecimateAnyway,
    /// The Preview button under Decimate: show the active object decimated,
    /// following Target Triangles, without changing it yet.
    DecimatePreview,
    /// Keep what the preview shows, as one undo step (Enter).
    DecimateApply,
    /// Close the preview and leave the model as it was (Esc).
    DecimateCancel,
    /// Add a subdivision level above the active object's finest and go to
    /// it (see app/levels.rs).
    Subdivide,
    /// Find the subdivision levels below the active object, a quad model
    /// subdivided elsewhere, and keep it on the finest (see
    /// app/levels/reconstruct.rs).
    ReconstructSubdiv,
    /// Go to this subdivision level of the active object; 0 is the coarsest.
    Level(usize),
    /// Go this many levels finer, or coarser if negative.
    LevelBy(i32),
    /// Delete the active object's subdivision levels below the one it is on
    /// Put the detail of another object onto the active one (see
    /// app/project.rs).
    ProjectDetail,
    /// (`lower`), or above it.
    DeleteLevels { lower: bool },
    /// A command of the Layers card, for the active object's sculpt layers
    /// (see app/layers.rs).
    Layer(LayerAction),
    /// A command of the Face Groups card, for the active object's face
    /// groups (see app/groups.rs).
    Groups(crate::ui::groups::Ask),
    /// A command of the Paint Layers card or the Paint card, for the active
    /// object's paint (see app/paint.rs).
    Paint(PaintAction),
    /// Cut the active object at one of its own planes and rebuild the other
    /// side as the mirror image of the side kept: the positive side when
    /// `from_positive` (see app/cut_mirror.rs).
    CutAndMirror { axis: sculpt_symmetry::Axis, from_positive: bool },
    /// Say on the status line how much of the active object pairs up across
    /// one of its own planes. Changes nothing (see app/symmetry_repair.rs).
    CheckSymmetry { axis: sculpt_symmetry::Axis },
    /// Move one side of the active object onto the mirror image of the
    /// other, vertex for vertex: the positive side is the one copied when
    /// `from_positive`. Refused, in words, where that would tear the model.
    MakeSymmetric { axis: sculpt_symmetry::Axis, from_positive: bool },
    /// Put the active object's near-centreline vertices exactly on the plane
    /// and merge the doubled ones a seam of two joined halves leaves there.
    SymmetryCleanup { axis: sculpt_symmetry::Axis },
    Mask(MaskOp),
    /// Mask the active object's recesses or its peaks, this much of them
    /// (0 to 1): a mask made from the surface's own shape (sculpt/cavity.rs).
    MaskShape(crate::sculpt::cavity::Shape, f32),
    /// Hide these triangles of the active object, or with `hide` false show
    /// them again: what a box or lasso hide gesture asks for.
    HideFaces { triangles: Vec<u32>, hide: bool },
    /// Show every hidden face of the active object again.
    ShowAllFaces,
    /// Swap what is hidden and what shows on the active object.
    InvertVisible,
    Matcap(usize),
    /// Pick a picture, copy it into the matcap folder and use it.
    ImportMatcap,
    /// Look through the matcap folder again for pictures added by hand.
    RescanMatcaps,
    /// Pick a picture, copy it into the alpha folder and give it to the
    /// brush in hand (see app/alpha.rs).
    ImportAlpha,
    /// Something asked of the brush library: duplicate, rename, delete,
    /// Import Brush and the rest (see app/brushes.rs).
    Brush(crate::sculpt::library::Ask),
    /// Something asked of the material library or the scene's palette (see
    /// app/materials.rs).
    Material(crate::materials::Ask),
    /// Look through the alpha folder again for pictures added by hand.
    RescanAlphas,
    /// Pick an HDRI picture to light the scene with.
    LoadHdri,
    ClearHdri,
    /// Pick a .cube colour table to grade the picture by, or drop it.
    LoadLut,
    ClearLut,
    Frame,
    /// Turn to an axis view (yaw, pitch in degrees) and switch to orthographic.
    Snap(f32, f32),
    /// The Ortho / Persp button: switch between the two, keeping the model
    /// the size it is on screen (see app/turn.rs).
    ToggleOrtho,
    /// A drag on the view gizmo's hand, in pixels: pan the view.
    PanView([f32; 2]),
    /// A drag on the view gizmo's magnifier, pixels down: zoom the view.
    ZoomView(f32),
    /// The view gizmo's Home: back to the view a scene starts with.
    HomeView,
    /// The History list (beside Undo and Redo): this many presses of Undo,
    /// or of Redo, at once.
    UndoSteps(usize),
    RedoSteps(usize),
    /// A file dragged onto the window and let go: a sculpt is opened, a
    /// model imported (see app/drop.rs).
    DropFile(PathBuf),
    /// Turn the view by a drag of this many pixels on the view gizmo;
    /// `begins` on the first of each drag (see app/turn.rs).
    Turn { by: [f32; 2], begins: bool },
    Tool(Tool),
    /// Put the active object's turn and size back to none.
    ResetTransform,
    /// Move the active object's centre to the middle of its mesh, leaving
    /// the object where it is (see app/origin.rs).
    SetOriginToMesh,
    /// Turn the SSphere rig's skin into a new object.
    MakeSkin,
    /// A command of the Mirror section of the Objects card: live copies of the
    /// active object across the world's planes.
    Mirror(crate::scene::MirrorCommand),
    /// Bring the SSphere tool up with a first sphere in the middle of the view.
    NewRig,
    /// A command of the Deform card: Bend, Twist and the other deformers the
    /// gizmo drives (see app/deform.rs).
    Deform(crate::ui::deform::Ask),
}
