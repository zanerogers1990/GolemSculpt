//! Trimming and splitting the selected object along a drawn shape: the half
//! of the two cutting tools that touches the scene.
//!
//! A cut is made in three steps, so the slow middle one can run anywhere:
//! [`Snapshot::take`] copies what the cut needs out of the scene, [`run`]
//! does the geometry (`sculpt_cut`) on those copies, and [`land`] puts the
//! result back as one undo step. On a small mesh the three run in a row on
//! the interface thread; on a large one the middle runs as a background job,
//! which also builds the sculpting structures there (`bake`), so the commit
//! stays instant at any size.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_cut::{CutView, ScreenRegion, SplitPass, split, trim_passes};
use sculpt_geom::{SymmetryOp, TriMesh};
use sculpt_toolkit::{Command, Ctx, MeshData, MeshId, NewObject, ObjectId, Placement, View};

use crate::screen::{cut_views, symmetry};

/// Which cut to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    /// Cut away what the shape covers, or with `remove_covered` false crop
    /// down to it.
    Trim { remove_covered: bool },
    /// Keep both sides: the covered one becomes a new object.
    Split,
}

impl Cut {
    pub fn label(self) -> &'static str {
        match self {
            Cut::Trim { .. } => "Trim",
            Cut::Split => "Split",
        }
    }
}

/// Everything a cut needs, copied out of the scene: a job cannot borrow from
/// it, and the scene may move on while a long cut runs.
pub struct Snapshot {
    object: ObjectId,
    /// The geometry the cut was worked out on. If the object shows another
    /// by the time the cut lands, the result no longer fits it.
    mesh: MeshId,
    placement: Placement,
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    /// The mask, when anything is masked: it is carried across the cut.
    mask: Option<Vec<f32>>,
    /// The camera at each place the mesh is drawn (see `screen::cut_views`).
    views: Vec<CutView>,
    symmetry: Vec<SymmetryOp>,
}

impl Snapshot {
    /// The selected object as seen through `view`, with the brushes' mirror
    /// axes and radial symmetry as the symmetry the cut repeats under.
    pub fn take(cx: &Ctx, view: &View) -> Result<Snapshot, String> {
        let scene = cx.scene();
        let object = scene.active();
        let (Some(info), Some(mesh)) = (scene.info(object), scene.mesh(object)) else {
            return Err("No object selected.".to_owned());
        };
        if mesh.positions.is_empty() || mesh.indices.len() < 3 {
            return Err("No geometry.".to_owned());
        }
        let masked = mesh.mask.len() == mesh.positions.len() && mesh.mask.par_iter().any(|&m| m > 0.0);
        Ok(Snapshot {
            object,
            mesh: info.mesh,
            placement: info.placement,
            positions: mesh.positions.to_vec(),
            indices: mesh.indices.to_vec(),
            mask: masked.then(|| mesh.mask.to_vec()),
            views: cut_views(scene, object, view),
            symmetry: symmetry(cx),
        })
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// One side of a cut as plain triangles, with its mask if one was carried.
type Side = (TriMesh, Option<Vec<f32>>);

/// What a cut made, before it is turned into scene geometry.
pub struct Pieces {
    kept: Side,
    /// The covered side of a split.
    piece: Option<Side>,
    /// A trim's whole report, or what a split adds to its own when the cut
    /// could not be fully closed.
    note: Option<String>,
    /// Every cut loop closed.
    clean: bool,
}

/// Makes the cut. Pure geometry on the snapshot's copies; safe on any thread.
/// Fails with a sentence for the user when the shape cuts nothing.
pub fn run(snapshot: &Snapshot, region: &ScreenRegion, cut: Cut) -> Result<Pieces, String> {
    let mask = snapshot.mask.as_deref();
    let carried = |m: Vec<f32>| mask.is_some().then_some(m);
    match cut {
        Cut::Trim { remove_covered } => {
            let done = trim_passes(
                &snapshot.positions,
                &snapshot.indices,
                mask,
                &snapshot.views,
                &snapshot.symmetry,
                region,
                remove_covered,
            )?;
            let (note, clean) = (Some(done.message()), done.open_loops == 0);
            Ok(Pieces { kept: (done.mesh, carried(done.mask)), piece: None, note, clean })
        }
        Cut::Split => {
            let passes = SplitPass::all(&snapshot.views, &snapshot.symmetry);
            let done = split(&snapshot.positions, &snapshot.indices, mask, &passes, region)?;
            let (note, clean) = (done.warning(), done.open_loops == 0);
            let kept = (done.outside.mesh, carried(done.outside.mask));
            let piece = (done.inside.mesh, carried(done.inside.mask));
            Ok(Pieces { kept, piece: Some(piece), note, clean })
        }
    }
}

impl Pieces {
    /// Turns the pieces into geometry for the scene with `bake`: the job's
    /// own baker off the interface thread, or [`plain`] on it.
    pub fn bake(self, snapshot: &Snapshot, bake: impl Fn(TriMesh, Option<Vec<f32>>) -> MeshData) -> Outcome {
        Outcome {
            object: snapshot.object,
            mesh: snapshot.mesh,
            placement: snapshot.placement,
            kept: bake(self.kept.0, self.kept.1),
            piece: self.piece.map(|(mesh, mask)| bake(mesh, mask)),
            note: self.note,
            clean: self.clean,
        }
    }
}

/// Geometry left as plain triangles: the app builds its sculpting structures
/// when the command is carried out. For meshes small enough not to notice.
pub fn plain(mesh: TriMesh, mask: Option<Vec<f32>>) -> MeshData {
    MeshData::Plain { mesh, mask }
}

/// A finished cut, ready to commit.
pub struct Outcome {
    object: ObjectId,
    mesh: MeshId,
    placement: Placement,
    kept: MeshData,
    piece: Option<MeshData>,
    note: Option<String>,
    clean: bool,
}

/// What became of a finished cut.
pub struct Landed {
    /// What to tell the user.
    pub message: String,
    /// Whether that is good news: the cut was made and every loop closed.
    pub ok: bool,
    /// The new object a split made.
    pub piece: Option<ObjectId>,
}

impl Landed {
    fn failed(message: &str) -> Landed {
        Landed { message: message.to_owned(), ok: false, piece: None }
    }
}

/// Puts a finished cut in the scene as one undo step.
///
/// A trim replaces the object's geometry. A split replaces it with the side
/// the shape did not cover and adds the covered side as a new object in the
/// same place, so one Z brings the whole object back. The cut object stays
/// selected, ready for the next cut.
pub fn land(cx: &mut Ctx, outcome: Outcome) -> Landed {
    let Outcome { object, mesh, placement, kept, piece, note, clean } = outcome;
    let name = match cx.scene().info(object) {
        Some(info) if info.mesh == mesh => info.name.to_owned(),
        // A cut that took seconds may come back to an object that was
        // deleted, remeshed or cut again in the meantime.
        _ => return Landed::failed("The object changed while it was being cut - nothing was done."),
    };
    let Some(piece) = piece else {
        let receipt = cx.commit("Trim", vec![Command::Replace { object, mesh: kept }]);
        if receipt.failed > 0 {
            return Landed::failed("The trimmed mesh could not be put in the scene.");
        }
        return Landed { message: note.unwrap_or_else(|| "Trimmed.".to_owned()), ok: clean, piece: None };
    };
    // The part is a piece of the object and is sculpted the way it is: it
    // starts with the object's own symmetry settings, as in the Unity app.
    let part = NewObject::new(format!("{name} Part"), piece).placed(placement).unselected().with_symmetry_of(object);
    let receipt = cx.commit("Split", vec![Command::Replace { object, mesh: kept }, Command::Add(part)]);
    let added = receipt.added.first().copied();
    let part_name = added.and_then(|id| cx.scene().info(id)).map(|info| info.name.to_owned());
    let Some(part_name) = part_name.filter(|_| receipt.failed == 0) else {
        return Landed::failed("The split could not be put in the scene. Undo (Z) takes back what was.");
    };
    let mut message = format!("Split into {name} and {part_name}.");
    if let Some(note) = note {
        message.push(' ');
        message.push_str(&note);
    }
    cx.toast("Split");
    Landed { message, ok: clean, piece: added }
}
