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
    /// Orbit and zoom around the last sculpted point.
    pub stroke_pivot: bool,
    pub tooltips: bool,
    pub ui_scale: f32,
    /// Voxels along the longest side for a voxel remesh.
    pub remesh_resolution: f32,
    /// Quads a quad remesh aims for.
    pub quad_target: f32,
    /// Triangles Decimate brings the selected object down to.
    pub decimate_target: f32,
    /// Triangle count for new shapes.
    pub new_shape_tris: f32,
    /// Tab: hide every panel.
    pub clean_view: bool,
    /// The Sculpt panel (left) and the Scene panel (right) folded away to a
    /// tab at the edge of the screen. Unlike the clean view this hides
    /// nothing else and switches nothing off.
    pub folded: [bool; 2],
    /// Which way the transform gizmo's arrows and rings point.
    pub gizmo_axes: crate::gizmo::Orientation,
    /// Finishes, lighting, environment and post effects.
    pub look: Look,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            matcap_on: true,
            matcap: 0,
            flat_shading: false,
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
            stroke_pivot: true,
            tooltips: true,
            ui_scale: 1.0,
            remesh_resolution: 128.0,
            quad_target: 5000.0,
            decimate_target: 200_000.0,
            new_shape_tris: 200_000.0,
            clean_view: false,
            folded: [false; 2],
            gizmo_axes: Default::default(),
            look: Look::default(),
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
    ExportObj,
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
    /// Triangles of the Remesh card, asking first if its symmetry would no
    /// longer be exact (see app/decimate.rs).
    Decimate,
    /// The answer "Decimate Anyway" to that question: straight to the work.
    DecimateAnyway,
    /// Cut the active object at one of its own planes and rebuild the other
    /// side as the mirror image of the side kept: the positive side when
    /// `from_positive` (see app/cut_mirror.rs).
    CutAndMirror { axis: sculpt_symmetry::Axis, from_positive: bool },
    Mask(MaskOp),
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
    /// Pick an HDRI picture to light the scene with.
    LoadHdri,
    ClearHdri,
    Frame,
    /// Turn to an axis view (yaw, pitch in degrees) and switch to orthographic.
    Snap(f32, f32),
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
}
