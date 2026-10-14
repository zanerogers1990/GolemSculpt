//! The level stack: one mesh held at several levels of subdivision.
//!
//! What is kept, and why so little:
//!
//! * The faces of the coarsest level, as the app gave them.
//! * For every level that has a finer one above it, the tables the finer one
//!   is made from (edges, neighbours, which edges are creases) and how it was
//!   split. The finer level's faces follow from these by rule, so they are
//!   written out when a level is handed over and not stored.
//! * While the app is working below the finest level, the finest level's
//!   positions and mask, exactly as they were handed in. Nothing else: no
//!   detail vectors, no positions per level. A step that changes neither
//!   passes them on as they are, shared with every clone of the stack the
//!   app keeps for undo, rather than copied.
//!
//! That last array is every level at once. Vertices keep their numbers going
//! up, and a coarse vertex is where the fine vertex of the same number is
//! (see below), so level `k` is the first `vertex_count(k)` entries of it.
//! And a level's detail, what sculpting added over its base surface, is the
//! difference between two things that can always be worked out again from
//! those entries, so it is worked out when it is needed (a coarse level was
//! edited and the edit has to be carried up: [`crate::detail`]) and never
//! stored. While the app is at the finest level even that array is not kept:
//! the app's own mesh is it.
//!
//! # Stepping down
//!
//! A coarse vertex takes the position of the fine vertex with its number.
//! The other choice is to fit the coarse level so that subdividing it lands
//! closest to the fine one. That reads better in a paper than under a pen: the
//! fitted cage sits off the surface (outside it where the form is convex), so
//! the level a sculptor steps down to would not be the shape they were just
//! looking at, and a brush would land on a surface that is not the sculpt.
//! Taking the vertices as they are is what ZBrush does; it costs nothing, is
//! exact, and stepping straight back up changes nothing at all.
//!
//! # Stepping up
//!
//! If the coarse level was not touched, the finer positions are handed back
//! as they were kept, bit for bit. If it was edited, the edit is found by
//! comparing what comes back against what went out, and carried up one level
//! at a time: at each level the base surface is made from the level below as
//! it was and as it is now, and each vertex the change reaches is put at the
//! new base plus its old detail, turned with the surface. Vertices the change
//! does not reach are not recomputed, nor, for a small edit, even looked at:
//! each level starts from the vertices that changed on the one below
//! ([`crate::dirty`]).
//!
//! One consequence to know about. The base of a level made smooth is the
//! smooth rule's, which pulls a kept vertex toward its neighbours. So a single
//! coarse vertex dragged out by itself comes back up as a smooth swelling
//! about half as high, not a spike of the full height, and stepping down
//! again shows the coarse vertex on that swelling. Anything broader than the
//! coarse mesh's own spacing (a limb bent, a belly fattened) is carried at
//! very nearly full size. Because of this the app must take the mesh a step
//! hands back, even when the step is to the level it is already on.
//!
//! A level made [`Split::Flat`] has the flat split as its base instead, so
//! what was built with flat faces keeps them when a coarser level is edited:
//! a corner follows its coarse vertex all the way and the middle of an edge
//! goes halfway between its ends.
//!
//! # The mask and other channels
//!
//! Down, a channel is its first entries. Up, kept vertices keep their values
//! and new ones take the mean of the vertices they lie between; an edit made
//! lower down is carried up as a change, on top of what the finer level
//! already had.

mod parts;
mod undo;
mod values;

pub use parts::LevelsParts;
pub use undo::{StepUndo, Stepped};
pub use values::{Rider, face_values_down, face_values_up};

use std::borrow::Cow;
use std::sync::Arc;

use glam::Vec3;
use rayon::prelude::*;

use crate::catmull_clark::{self, QuadTables};
use crate::dirty::{self, Way};
use crate::loop_scheme::LoopTables;
use crate::mesh::{Faces, LevelMesh, Polys};
use crate::refine::{Channel, MOST_VERTICES, PER_TASK, Refiner, Split, place, raise};

/// Which rules a stack subdivides by, settled by the mesh it was started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// All triangles: Loop subdivision.
    Loop,
    /// Polygon faces: Catmull-Clark subdivision.
    CatmullClark,
}

/// A mesh at several levels of subdivision. Level 0 is the coarsest.
///
/// The app holds the mesh of one level, the current one, and sculpts it. Every
/// call that changes level takes that mesh's positions and mask as they now
/// are and gives back the mesh to carry on with.
///
/// ```text
/// let mut levels = Levels::new(vertex_count, triangles, polys)?;   // one level: the mesh as it is
/// let finer = levels.subdivide(&positions, &mask, Split::Smooth)?; // the app swaps its mesh for `finer`
/// // ... sculpt the detail ...
/// let step = levels.step_to(0, &positions, &mask)?;                // down: carry on with `step.mesh`
/// // ... bend the arm ...
/// let step = levels.step_to(levels.top(), &positions, &mask)?;     // up: the detail has come along
/// ```
///
/// A clone shares the faces and tables of every level, and the kept finest
/// level until one of the two changes it, so keeping one for undo costs
/// little: see [`Self::owned_bytes`].
#[derive(Clone)]
pub struct Levels {
    /// The faces of every level that has a finer one, and of the only level
    /// while there is one. Shared between clones, as the steps are: neither
    /// changes once made, and a clone is how the app keeps a stack for undo.
    faces: Vec<Arc<Faces>>,
    /// `steps[k]` makes level `k + 1` from level `k`.
    steps: Vec<Arc<Step>>,
    /// One entry per level.
    sizes: Vec<Size>,
    current: usize,
    /// The finest level, kept while the app is below it.
    kept: Option<Kept>,
}

#[derive(Clone)]
struct Step {
    tables: Tables,
    split: Split,
}

#[derive(Clone)]
enum Tables {
    Loop(LoopTables),
    Quad(QuadTables),
}

#[derive(Clone, Copy)]
struct Size {
    vertices: usize,
    triangles: usize,
}

/// The finest level while the app is below it. Shared, because a step that
/// carries nothing up hands it on unchanged, and the app keeps the stack of
/// every step for undo: copied, a model of ten million triangles paid 80 MB
/// and tens of milliseconds a step for a copy of what it already had. (An
/// `Arc<Vec>` and not an `Arc<[T]>`: a step that does carry an edit up makes
/// the new list as a `Vec`, and arriving on the finest level hands the list
/// to the app as one, and neither then has to be copied again.)
#[derive(Clone)]
struct Kept {
    positions: Arc<Vec<Vec3>>,
    mask: Arc<Vec<f32>>,
}

/// Runs `$body` with `$r` the [`Refiner`] that makes level `$level + 1` of
/// `$levels`. A macro and not a function because the two schemes' refiners
/// are different types, and the loops they are handed to are compiled for
/// each rather than calling through a pointer per vertex.
macro_rules! with_refiner {
    ($levels:expr, $level:expr, |$r:ident| $body:expr) => {{
        let levels: &$crate::levels::Levels = $levels;
        let level: usize = $level;
        match (&levels.steps[level].tables, &*levels.faces[level]) {
            ($crate::levels::Tables::Loop(tables), $crate::mesh::Faces::Triangles(triangles)) => {
                let $r = $crate::loop_scheme::LoopRefiner::new(triangles, tables);
                $body
            }
            ($crate::levels::Tables::Quad(tables), faces) => {
                let poly = faces.poly_ref().expect("a polygon level has polygon faces");
                let $r = $crate::catmull_clark::QuadRefiner::new(levels.sizes[level].vertices, poly, tables);
                $body
            }
            ($crate::levels::Tables::Loop(_), _) => unreachable!("a triangle level has triangle faces"),
        }
    }};
}
pub(crate) use with_refiner;

impl Tables {
    /// The tables that make the level above `faces`, and that level's size.
    fn build(faces: &Faces, size: Size) -> Result<(Tables, Size), String> {
        const TOO_LARGE: &str = "The mesh is too large to subdivide again.";
        let (tables, new_vertices, triangles) = match faces {
            Faces::Triangles(triangles) => {
                if triangles.len() > (u32::MAX / 4) as usize {
                    return Err(TOO_LARGE.into());
                }
                let tables = LoopTables::build(size.vertices, triangles);
                let new = tables.edge_count();
                (Tables::Loop(tables), new, size.triangles * 4)
            }
            polys => {
                let poly = polys.poly_ref().expect("faces that are not triangles are polygons");
                if poly.corners.len() > (u32::MAX / 4) as usize {
                    return Err(TOO_LARGE.into());
                }
                let tables = QuadTables::build(size.vertices, poly);
                let new = tables.edge_count() + poly.face_count();
                (Tables::Quad(tables), new, poly.corners.len() * 2)
            }
        };
        let vertices = size.vertices + new_vertices;
        if vertices > MOST_VERTICES {
            return Err(TOO_LARGE.into());
        }
        Ok((tables, Size { vertices, triangles }))
    }

    fn bytes(&self) -> usize {
        match self {
            Tables::Loop(tables) => tables.bytes(),
            Tables::Quad(tables) => tables.bytes(),
        }
    }
}

impl Levels {
    /// A stack of one level: the mesh as it is. A mesh with polygon faces
    /// (`polys`, with `triangles` those faces cut into triangles) is
    /// subdivided by Catmull-Clark and one without by Loop.
    ///
    /// Refuses indices that point past `vertex_count`, and polygon faces and
    /// triangles that are not the same surface. For the latter the app can
    /// start again without the polygon faces and have Loop levels.
    pub fn new(vertex_count: usize, triangles: Vec<u32>, polys: Option<Polys>) -> Result<Levels, String> {
        let faces = Faces::given(vertex_count, triangles, polys)?;
        let size = Size { vertices: vertex_count, triangles: faces.triangle_count() };
        Ok(Levels { faces: vec![Arc::new(faces)], steps: Vec::new(), sizes: vec![size], current: 0, kept: None })
    }

    pub fn scheme(&self) -> Scheme {
        match *self.faces[0] {
            Faces::Triangles(_) => Scheme::Loop,
            _ => Scheme::CatmullClark,
        }
    }

    /// How many levels there are; never fewer than one.
    pub fn level_count(&self) -> usize {
        self.sizes.len()
    }

    /// The finest level.
    pub fn top(&self) -> usize {
        self.steps.len()
    }

    /// The level whose mesh the app holds.
    pub fn current(&self) -> usize {
        self.current
    }

    /// Panics if there is no such level, as do the other per-level getters.
    pub fn vertex_count(&self, level: usize) -> usize {
        self.sizes[level].vertices
    }

    pub fn triangle_count(&self, level: usize) -> usize {
        self.sizes[level].triangles
    }

    /// How `level` was made from the one below. Level 0 was not made here and has none.
    pub fn split(&self, level: usize) -> Option<Split> {
        level.checked_sub(1).map(|below| self.steps[below].split)
    }

    /// The triangles of `level`.
    pub fn triangles(&self, level: usize) -> Vec<u32> {
        self.faces_of(level).triangles()
    }

    /// The polygon faces of `level`, for a polygon mesh.
    pub fn polys(&self, level: usize) -> Option<Polys> {
        match self.scheme() {
            Scheme::Loop => None,
            Scheme::CatmullClark => self.faces_of(level).polys(),
        }
    }

    /// The triangles of `level` and, for a polygon mesh, its polygon faces:
    /// [`Self::triangles`] and [`Self::polys`] at once, for the cost of one.
    pub fn faces(&self, level: usize) -> (Vec<u32>, Option<Polys>) {
        match self.faces_of(level) {
            Cow::Borrowed(faces) => (faces.triangles(), faces.polys()),
            Cow::Owned(faces) => faces.into_lists(),
        }
    }

    /// For every triangle of `level`, the triangle of the level below that it
    /// lies in. For triangle levels that is simply `triangle / 4`.
    pub fn parent_triangles(&self, level: usize) -> Result<Vec<u32>, String> {
        self.check_level(level)?;
        let below = level.checked_sub(1).ok_or("The coarsest level has no level below it.")?;
        Ok(with_refiner!(self, below, |r| r.parent_triangles()))
    }

    /// For every polygon face of `level`, the face of the level below that it
    /// was made in; `None` for a triangle mesh.
    pub fn parent_polys(&self, level: usize) -> Result<Option<Vec<u32>>, String> {
        self.check_level(level)?;
        let below = level.checked_sub(1).ok_or("The coarsest level has no level below it.")?;
        Ok(self.faces[below].poly_ref().map(catmull_clark::parent_faces))
    }

    /// Adds a level finer than the finest and moves to it. `positions` and
    /// `mask` are the finest level as it now stands; the app must be on it.
    ///
    /// The new mask is the old one carried up without smoothing.
    pub fn subdivide(&mut self, positions: &[Vec3], mask: &[f32], split: Split) -> Result<LevelMesh, String> {
        if self.current != self.top() {
            return Err("Step up to the finest level before subdividing.".into());
        }
        self.check_mesh(positions, mask)?;
        let top = self.top();
        self.grow(split)?;
        let (positions, mask) = with_refiner!(self, top, |r| (place(&r, split, positions), raise(&r, mask)));
        self.current = top + 1;
        Ok(self.mesh(top + 1, positions, mask))
    }

    /// Takes back the last [`Self::subdivide`]: drops the finest level and
    /// leaves the stack on the one below, as it was. The app must be on the
    /// finest level, and puts back the positions and mask it handed to
    /// `subdivide` itself (they were only borrowed).
    pub fn undo_subdivide(&mut self) -> Result<(), String> {
        if self.current != self.top() || self.top() == 0 {
            return Err("There is no finer level to take back from here.".into());
        }
        self.current -= 1;
        self.drop_above(self.current);
        Ok(())
    }

    /// Moves the app from the current level to `level`. `positions` and
    /// `mask` are the current level as it now stands, sculpted or not; the
    /// result holds the mesh of `level` and what [`Self::undo_step`] needs to
    /// take the move back.
    ///
    /// Stepping to the current level is allowed. It carries any edit of this
    /// level up through the finer ones now, and hands back this level as it
    /// then stands (see the module notes on why that can differ).
    ///
    /// Below the finest level, a position or mask value that is not a number
    /// is taken as not edited. (On the finest level the mesh handed in is
    /// simply kept, whatever it holds.)
    pub fn step_to(&mut self, level: usize, positions: &[Vec3], mask: &[f32]) -> Result<Stepped, String> {
        let (positions, mask, undo) = self.step_lists(level, positions, mask)?;
        Ok(Stepped { mesh: self.mesh(level, positions, mask), undo })
    }

    /// [`Self::step_to`] without the faces of the level arrived at: its
    /// positions and mask, and what [`Self::undo_step`] needs. Writing out
    /// the faces of a finest level of ten million triangles takes a while,
    /// and a caller that kept them from before has no use for them
    /// ([`Self::faces`] gives them when it has not).
    pub fn step_lists(
        &mut self,
        level: usize,
        positions: &[Vec3],
        mask: &[f32],
    ) -> Result<(Vec<Vec3>, Vec<f32>, StepUndo), String> {
        self.check_level(level)?;
        self.check_mesh(positions, mask)?;
        let (from, top) = (self.current, self.top());
        let mut undo = StepUndo::new(from, level, self.level_count());
        match self.kept.take() {
            // On the finest level the handed mesh is the stack's whole state.
            None => self.kept = Some(Kept::of(positions.to_vec(), mask.to_vec())),
            Some(kept) => self.kept = Some(self.fold(from, kept, positions, mask, &mut undo)),
        }
        self.current = level;
        // Arriving at the finest level takes the kept arrays with it; anywhere else gets a copy of its share.
        let kept = self.kept.take().expect("the finest level was just kept");
        let (positions, mask) = if level == top {
            kept.into_lists()
        } else {
            let first = kept.first(self.sizes[level]);
            self.kept = Some(kept);
            first
        };
        Ok((positions, mask, undo))
    }

    /// Makes the current level the coarsest by dropping every level below it.
    /// The app's mesh is not affected: the levels that remain keep their
    /// vertex and face numbers.
    pub fn delete_lower(&mut self) {
        let level = self.current;
        if level == 0 {
            return;
        }
        if level < self.faces.len() {
            self.faces.drain(..level);
        } else {
            self.faces = vec![Arc::new(self.faces_of(level).into_owned())];
        }
        self.steps.drain(..level);
        self.sizes.drain(..level);
        self.current = 0;
    }

    /// Makes the current level the finest by dropping every level above it,
    /// and with them the detail they held. The app's mesh is not affected.
    pub fn delete_higher(&mut self) {
        self.drop_above(self.current);
    }

    /// Roughly what the stack holds in memory.
    pub fn memory_bytes(&self) -> usize {
        (0..self.level_count()).map(|level| self.level_bytes(level)).sum::<usize>() + self.kept_bytes()
    }

    /// What is held for `level` itself: its faces and the tables the level
    /// above is made from. Nothing for the finest level of a stack of several.
    pub fn level_bytes(&self, level: usize) -> usize {
        self.faces.get(level).map_or(0, |faces| faces.bytes())
            + self.steps.get(level).map_or(0, |step| step.tables.bytes())
    }

    /// This stack's share of the memory: of each level's faces and tables,
    /// and of the kept finest level, an equal part with every clone that
    /// shares them. Summed over a stack and its clones it is what they hold
    /// between them, so each can be charged for it without anything being
    /// counted once per clone.
    pub fn owned_bytes(&self) -> usize {
        let faces: usize = self.faces.iter().map(|faces| faces.bytes() / Arc::strong_count(faces)).sum();
        let steps: usize = self.steps.iter().map(|step| step.tables.bytes() / Arc::strong_count(step)).sum();
        let kept = self.kept.as_ref().map_or(0, |kept| {
            kept.positions.len() * 12 / Arc::strong_count(&kept.positions)
                + kept.mask.len() * 4 / Arc::strong_count(&kept.mask)
        });
        faces + steps + kept
    }

    /// The finest level's positions as the stack keeps them while the app is
    /// on a coarser level: as they were when the finest level was last left,
    /// with whatever has been carried up into them since. `None` on the
    /// finest level, where the app's own mesh is that level.
    pub fn finest_positions(&self) -> Option<&[Vec3]> {
        self.kept.as_ref().map(|kept| &kept.positions[..])
    }

    /// The finest level's positions and mask, held only while the app is
    /// below it: 16 bytes for each of its vertices, and the whole cost of
    /// keeping every level's detail.
    pub fn kept_bytes(&self) -> usize {
        self.kept.as_ref().map_or(0, |kept| kept.positions.len() * 12 + kept.mask.len() * 4)
    }

    // ---- inside ------------------------------------------------------------

    fn check_level(&self, level: usize) -> Result<(), String> {
        if level < self.level_count() { Ok(()) } else { Err(format!("There is no subdivision level {level}.")) }
    }

    fn check_mesh(&self, positions: &[Vec3], mask: &[f32]) -> Result<(), String> {
        let vertices = self.sizes[self.current].vertices;
        if positions.len() == vertices && mask.len() == vertices {
            Ok(())
        } else {
            Err(format!(
                "The mesh has {} vertices and {} mask values; subdivision level {} has {vertices} vertices.",
                positions.len(),
                mask.len(),
                self.current
            ))
        }
    }

    fn faces_of(&self, level: usize) -> Cow<'_, Faces> {
        match self.faces.get(level) {
            Some(faces) => Cow::Borrowed(&**faces),
            None => Cow::Owned(with_refiner!(self, level - 1, |r| r.fine_faces())),
        }
    }

    fn mesh(&self, level: usize, positions: Vec<Vec3>, mask: Vec<f32>) -> LevelMesh {
        let (triangles, polys) = self.faces(level);
        LevelMesh { positions, mask, triangles, polys }
    }

    /// Adds the tables for a level above the finest. Positions play no part:
    /// a stack read back from a file is rebuilt with this alone.
    fn grow(&mut self, split: Split) -> Result<(), String> {
        let top = self.top();
        let stored = top < self.faces.len();
        if !stored {
            let faces = self.faces_of(top).into_owned();
            self.faces.push(Arc::new(faces));
        }
        match Tables::build(&self.faces[top], self.sizes[top]) {
            Ok((tables, size)) => {
                self.steps.push(Arc::new(Step { tables, split }));
                self.sizes.push(size);
                Ok(())
            }
            Err(message) => {
                if !stored {
                    self.faces.pop();
                }
                Err(message)
            }
        }
    }

    fn drop_above(&mut self, level: usize) {
        self.steps.truncate(level);
        self.sizes.truncate(level + 1);
        self.faces.truncate(level.max(1));
        self.kept = None;
    }

    /// Carries what the app changed on level `from` up into the kept finest
    /// level, noting in `undo` what it changed there. What did not change is
    /// handed on as it was, shared.
    fn fold(&self, from: usize, kept: Kept, positions: &[Vec3], mask: &[f32], undo: &mut StepUndo) -> Kept {
        let Kept { positions: kept_positions, mask: kept_mask } = kept;
        let top = self.top();

        let live = sound(positions, &kept_positions, |p| p.is_finite());
        let positions = match undo.note_live_positions(&live, &kept_positions) {
            Some(changed) => {
                let (raised, changed) = self.raise_positions(from, &live, changed, &kept_positions, Way::Quicker);
                undo.note_kept_positions(&kept_positions, changed);
                Arc::new(raised)
            }
            None => kept_positions,
        };

        let live = sound(mask, &kept_mask, |m| m.is_finite());
        let mask = match undo.note_live_mask(&live, &kept_mask) {
            Some(changed) => {
                let settle = |m: f32| m.clamp(0.0, 1.0);
                let (raised, changed) = self.raise_edit(from, top, &live, changed, &kept_mask, &settle, Way::Quicker);
                undo.note_kept_mask(&kept_mask, changed);
                Arc::new(raised)
            }
            None => kept_mask,
        };
        Kept { positions, mask }
    }

    /// The finest level's positions after level `from` was edited to `live`
    /// at the vertices `changed` (ascending), `kept` being the finest level
    /// before; and where the two differ, ascending. `way` is for tests.
    pub(crate) fn raise_positions(
        &self,
        from: usize,
        live: &[Vec3],
        changed: &[u32],
        kept: &[Vec3],
        way: Way,
    ) -> (Vec<Vec3>, Vec<u32>) {
        let mut work = copied(kept);
        for &v in changed {
            work[v as usize] = live[v as usize];
        }
        let mut changed = changed.to_vec();
        for level in from..self.top() {
            let split = self.steps[level].split;
            changed = with_refiner!(self, level, |r| dirty::carry_level(&r, split, kept, &mut work, &changed, way));
        }
        (work, changed)
    }

    /// A channel at level `to` after its entries for level `from` were edited
    /// to `edited` at the vertices `changed` (ascending), `fine` being the
    /// channel at `to` before; and where the two differ, ascending.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn raise_edit<T: Channel>(
        &self,
        from: usize,
        to: usize,
        edited: &[T],
        changed: &[u32],
        fine: &[T],
        settle: &(impl Fn(T) -> T + Sync),
        way: Way,
    ) -> (Vec<T>, Vec<u32>) {
        let mut work = copied(fine);
        for &v in changed {
            work[v as usize] = edited[v as usize];
        }
        let mut changed = changed.to_vec();
        for level in from..to {
            changed = with_refiner!(self, level, |r| dirty::raise_level(&r, fine, &mut work, &changed, settle, way));
        }
        (work, changed)
    }
}

#[cfg(test)]
impl Levels {
    /// The kept finest level's mask, as [`Self::finest_positions`] is of its positions.
    pub(crate) fn finest_mask(&self) -> Option<&[f32]> {
        self.kept.as_ref().map(|kept| &kept.mask[..])
    }

    /// For each fine vertex of level `level + 1`, the vertices its fan names;
    /// and for each vertex of `level`, what [`Refiner::touched_by`] names.
    pub(crate) fn fans_and_touches(&self, level: usize) -> (Vec<Vec<u32>>, Vec<Vec<u32>>) {
        with_refiner!(self, level, |r| {
            let fan = |i: usize| {
                let mut named = Vec::new();
                r.fan(i, |a, b| named.extend([a, b]));
                named
            };
            let touched = |c: u32| {
                let mut named = Vec::new();
                r.touched_by(c, |v| named.push(v));
                named
            };
            ((0..r.total()).map(fan).collect(), (0..r.kept() as u32).map(touched).collect())
        })
    }
}

impl Kept {
    fn of(positions: Vec<Vec3>, mask: Vec<f32>) -> Kept {
        Kept { positions: Arc::new(positions), mask: Arc::new(mask) }
    }

    /// The level of `size` out of the finest: its first entries.
    fn first(&self, size: Size) -> (Vec<Vec3>, Vec<f32>) {
        (self.positions[..size.vertices].to_vec(), self.mask[..size.vertices].to_vec())
    }

    /// The lists themselves, copied only if a clone of the stack shares them.
    fn into_lists(self) -> (Vec<Vec3>, Vec<f32>) {
        (Arc::unwrap_or_clone(self.positions), Arc::unwrap_or_clone(self.mask))
    }
}

/// A copy of `list` made on every thread: these are the finest level's.
fn copied<T: Copy + Send + Sync>(list: &[T]) -> Vec<T> {
    list.par_iter().with_min_len(PER_TASK * 8).copied().collect()
}

/// What the app handed in for a level, with anything that is not a number
/// put back to what the stack holds for that vertex.
fn sound<T: Copy + Send + Sync>(handed: &[T], kept: &[T], is_sound: impl Fn(T) -> bool + Sync) -> Vec<T> {
    handed
        .par_iter()
        .zip(&kept[..handed.len()])
        .with_min_len(PER_TASK)
        .map(|(&handed, &kept)| if is_sound(handed) { handed } else { kept })
        .collect()
}
