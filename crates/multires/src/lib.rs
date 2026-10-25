//! Subdivision levels: a mesh subdivided, sculpted at any level, with the
//! finer detail kept.
//!
//! This is the way of working ZBrush and Nomad are built round. Shape the big
//! forms on a coarse mesh, subdivide, add finer detail, and still be able to
//! step back down to a coarse level, change a big form (bend an arm, fatten a
//! belly) and step back up with the fine detail intact and carried along.
//!
//! Nothing here knows about the app or the sculpting mesh. Functions take
//! slices and return `Vec`s, so they can be tested and timed on their own.
//!
//! * [`Levels`] is the thing to use: a mesh held at several levels. Start
//!   with `levels` for how it works and why it keeps what it keeps.
//! * `loop_scheme` and `catmull_clark` are the two ways of subdividing: Loop
//!   for a mesh of triangles, Catmull-Clark for one with polygon faces. Their
//!   headers give the rules, the numbering, and what happens at rims, at edges
//!   where more than two faces meet, and at stray vertices.
//! * [`Turn`] is how a surface turned at a point between two shapes, for
//!   whatever has to turn with it (the levels' own detail, and the app's
//!   sculpt layers).
//! * `refine` is what the two share, and `detail` is how sculpted detail
//!   follows an edit made lower down; `dirty` does the same from what the
//!   edit changed, so that a dab low down is not paid for at every vertex.
//! * [`subdivide`] is one subdivision on its own, with no levels kept.
//! * [`reconstruct`] goes the other way: the levels below a quad mesh that
//!   was made by subdividing (`unsubdivide`).
//!
//! # What a caller can rely on
//!
//! * **Numbering.** The vertices of a level keep their numbers in the level
//!   above, and new vertices are appended by a fixed rule. Every finer face
//!   knows the coarser face it came from. So anything kept per vertex or per
//!   face can follow a change of level.
//! * **The same answer every time.** Results do not depend on how the work
//!   was shared between threads, to the bit.
//! * **Nothing is lost by looking.** Stepping down and back up without
//!   editing gives back the finer level bit for bit.
//! * **Symmetry.** A mesh that mirrors onto itself subdivides to one that
//!   does, to within rounding: sums over neighbours are in double precision.
//! * **No surprises on bad meshes.** Open rims are handled properly. Edges
//!   with more than two faces, vertices no face uses and triangles with two
//!   equal corners neither panic nor produce a not-a-number; data that does
//!   not fit together is refused with a message when it is handed in.
//!
//! The mesh the app sculpts is not numbered the way a level is: building it
//! reorders vertices and triangles for its spatial index. Carrying positions
//! and per-face data across that renumbering is the app's side of the join.

mod catmull_clark;
mod detail;
mod dirty;
mod levels;
mod loop_scheme;
mod mesh;
mod refine;
mod turn;
mod unsubdivide;

#[cfg(test)]
mod tests;

pub use levels::{Levels, LevelsParts, Rider, Scheme, StepUndo, Stepped, face_values_down, face_values_up};
pub use mesh::{LevelMesh, Polys};
pub use refine::{Channel, Split};
pub use turn::{Fan, Turn};
pub use unsubdivide::{Coarser, Rebuilt, reconstruct, unsubdivide};

/// What [`subdivide`] makes.
#[derive(Clone, Debug, PartialEq)]
pub struct Subdivided {
    pub mesh: LevelMesh,
    /// For every triangle of `mesh`, the triangle it came from.
    pub parent_triangles: Vec<u32>,
    /// For every polygon face of `mesh`, the face it came from; `None` for a
    /// triangle mesh.
    pub parent_polys: Option<Vec<u32>>,
}

/// One subdivision of a mesh, with no levels kept: Loop for a mesh of
/// triangles, Catmull-Clark for one with polygon faces. The first
/// `mesh.positions.len()` vertices of the result are the mesh's own, in the
/// same order.
pub fn subdivide(mesh: &LevelMesh, split: Split) -> Result<Subdivided, String> {
    let mut levels = Levels::new(mesh.positions.len(), mesh.triangles.clone(), mesh.polys.clone())?;
    let fine = levels.subdivide(&mesh.positions, &mesh.mask, split)?;
    Ok(Subdivided { mesh: fine, parent_triangles: levels.parent_triangles(1)?, parent_polys: levels.parent_polys(1)? })
}
