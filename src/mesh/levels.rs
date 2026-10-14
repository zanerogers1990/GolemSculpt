//! Subdivision levels of a mesh: the mesh kept at several levels of
//! fineness, sculpted at any of them, with the finer detail carried along
//! (`sculpt_multires` does the geometry; this is how a `Mesh` holds it).
//!
//! **The levels live in the mesh** (`Mesh::levels`), and each mesh carries
//! the whole of them: the stack as it stood when this mesh was the one being
//! sculpted, and how this mesh's numbering maps onto the stack's. Everything
//! else follows from that one choice:
//!
//! - Instances share a mesh, so they share its levels.
//! - A change of level builds a new mesh and leaves the old one untouched,
//!   stack and all. The old mesh goes on the undo stack whole, as after a
//!   remesh, so Undo and Redo are a swap: exact, instant, and with nothing to
//!   work out again that might come out differently.
//! - Anything that puts a new mesh in an object's place (remesh, decimate, a
//!   boolean, a join, a mesh back from Blender...) leaves the levels behind
//!   with the old mesh without having to know they exist, and Undo of it
//!   brings them back the same way.
//!
//! What that costs is small. The stack's tables are shared between its
//! copies (`Levels::owned_bytes`), so a mesh kept for undo adds only the
//! finest level's positions and mask, and only if it was a coarser level.
//!
//! **Numbering.** Building a mesh reorders its vertices and triangles for the
//! spatial index (`Bvh::build`), and the stack numbers a level its own way
//! (kept vertices first, then new ones by rule). So a mesh with levels keeps
//! two lists: for each vertex of its level, the mesh vertex it is, and for
//! each mesh triangle, the level's triangle. Brushes, undo records and the
//! renderer see an ordinary mesh; the lists are read only when the level
//! changes.
//!
//! **Hidden faces** follow a change of level through the stack's parent
//! triangles. Going up, a face is hidden when the face it came from is. Going
//! down, a face is hidden only when every face made from it is: whatever
//! shows on the finer level still shows on the coarser one, so nothing that
//! was in view vanishes and the brush can reach everything it could before.
//! (The other rule, hidden when any is, hides more with every step down and
//! up.) Either rule loses the exact edge of a hidden patch when it is coarser
//! than the patch, so each level's flags are remembered when the level is
//! left, and put back as they were if, on the way back, what is hidden still
//! agrees with them.
//!
//! **Face groups** follow a change of level too, by rules of their own (see
//! `levels/grouped.rs`).
//!
//! **Dynamic topology** splits triangles in place, which a level's numbering
//! does not survive, so a mesh with levels is never refined (`Mesh::refine`).
//!
//! **The way back up is quick.** A mesh below its finest level keeps what
//! the finest level's mesh was built into (`levels/cached.rs`), so stepping
//! up to it again does not build a ten-million-triangle mesh from nothing.

use std::sync::Arc;

use glam::Vec3;
use rayon::prelude::*;
use sculpt_multires::{LevelMesh, Levels, LevelsParts, Scheme, Split};

use super::layers::Layers;
use super::{Mesh, Polys, sound};

mod cached;
mod grouped;
mod layered;

/// The most triangles a level made by Subdivide may have. Past this a mesh
/// and its undo no longer fit comfortably in memory, and a step between
/// levels would take many seconds.
pub const MOST_TRIANGLES: usize = 25_000_000;

/// Vertices handed to one rayon task.
const PER_TASK: usize = 1 << 14;

/// How building a mesh renumbered what it was given.
pub struct Numbering {
    /// For each vertex handed in, its number in the mesh (`u32::MAX` for one
    /// no triangle uses).
    pub vertex: Vec<u32>,
    /// For each triangle of the mesh, its place in the list handed in.
    pub triangle: Vec<u32>,
}

/// A mesh's subdivision levels: see the module notes.
pub struct MeshLevels {
    stack: Levels,
    /// For each vertex of the current level, in the stack's numbering, the
    /// mesh's vertex.
    vertex: Vec<u32>,
    /// For each triangle of the mesh, the stack's triangle.
    triangle: Vec<u32>,
    /// Which faces were hidden on each level when it was last left, in the
    /// stack's order; `None` for a level with nothing hidden or never left.
    hidden: Remembered,
    /// The finest level's copy of each sculpt layer's offsets, while the
    /// mesh is on a coarser level (see `layered.rs`).
    finest: layered::Finest,
    /// The face groups of the other levels (see `grouped.rs`).
    groups: grouped::Kept,
    /// Below the finest level: what the finest level's mesh was built into,
    /// for the way back up (see `cached.rs`). Shared with every mesh kept
    /// for undo since the step down.
    top_mesh: Option<Arc<cached::TopMesh>>,
    /// How tight this mesh's spatial index was when it was built
    /// (`cached::spread`); 0 when not known.
    spread: f64,
}

/// A mesh's levels as a file keeps them: the stack as plain lists, and the
/// two numbering lists against the mesh as the file holds it.
pub struct SavedLevels {
    pub parts: LevelsParts,
    pub vertex: Vec<u32>,
    pub triangle: Vec<u32>,
    pub hidden: Vec<Option<Vec<bool>>>,
    /// The finest level's copy of each sculpt layer's offsets, by the
    /// layer's number; none on the finest level.
    pub finest: Vec<(u32, Vec<Vec3>)>,
    /// The face groups each level was left with, in the stack's order; kept
    /// in the file with the mesh's groups, not with its levels (`io/groups.rs`).
    pub groups: Vec<Option<Vec<u16>>>,
}

/// What Subdivide did besides adding a level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Subdivided {
    /// The mesh had quads that are not its triangles cut up, so it was
    /// subdivided as triangles and has no quads now.
    pub quads_set_aside: bool,
}

impl Mesh {
    /// [`Mesh::new`], also saying where every vertex and triangle went.
    /// `hidden` is one flag per triangle handed in, or empty.
    pub fn new_numbered(
        positions: Vec<Vec3>,
        indices: Vec<u32>,
        mask: Option<Vec<f32>>,
        hidden: &[bool],
    ) -> (Mesh, Numbering) {
        // A build leaves out triangles it cannot use and numbers the rest
        // among themselves: this is where each of those stood in the list.
        let vertex_count = positions.len() as u32;
        let kept: Vec<u32> = indices
            .chunks_exact(3)
            .enumerate()
            .filter(|(_, t)| sound(t, vertex_count))
            .map(|(place, _)| place as u32)
            .collect();
        let flagged = hidden.len() == indices.len() / 3 && hidden.contains(&true);
        let (mut mesh, vertex, came_from) = Mesh::new_ordered(positions, indices, mask);
        let triangle: Vec<u32> = came_from.iter().map(|&t| kept[t as usize]).collect();
        if flagged {
            let flags: Vec<bool> = triangle.iter().map(|&t| hidden[t as usize]).collect();
            mesh.restore_hidden_triangles(&flags);
        }
        (mesh, Numbering { vertex, triangle })
    }

    /// How many subdivision levels the mesh has: 1 for a mesh without any.
    pub fn level_count(&self) -> usize {
        self.levels.as_ref().map_or(1, |levels| levels.stack.level_count())
    }
}

/// A list as long as `map` whose entry `map[i]` is `values[i]`: a level's
/// list put in the order of the thing the map numbers.
fn placed<T: Copy + Default>(values: &[T], map: &[u32]) -> Vec<T> {
    let mut out = vec![T::default(); map.len()];
    for (&value, &at) in values.iter().zip(map) {
        out[at as usize] = value;
    }
    out
}

/// A level's faces hidden as the faces of the level below are: one step
/// down. A face is hidden when it has children and they all are.
fn hidden_below(parents: &[u32], fine: &[bool], coarse_count: usize) -> Vec<bool> {
    // 0: no child seen, 1: every child so far is hidden, 2: one shows.
    let mut state = vec![0u8; coarse_count];
    for (&parent, &hidden) in parents.iter().zip(fine) {
        let seen = &mut state[parent as usize];
        *seen = (*seen).max(if hidden { 1 } else { 2 });
    }
    state.iter().map(|&seen| seen == 1).collect()
}

/// Hidden flags of level `from` carried to level `to` by the rules in the
/// module notes.
fn carried_hidden(stack: &Levels, from: usize, to: usize, flags: &[bool]) -> Vec<bool> {
    if to >= from {
        return stack.face_values(from, to, flags, false).expect("flags of the level they were read from");
    }
    let mut now = flags.to_vec();
    for level in (to..from).rev() {
        let parents = stack.parent_triangles(level + 1).expect("a level below the one the flags are of");
        now = hidden_below(&parents, &now, stack.triangle_count(level));
    }
    now
}

/// What each level was left with hidden, in the stack's order.
type Remembered = Vec<Option<Arc<[bool]>>>;

/// The hidden faces for level `to` of `stack` on leaving level `from` with
/// `now` hidden, and what to remember from here on: the flags `to` was left
/// with if what is hidden now still agrees with them, else what is hidden
/// now carried over. An empty list is nothing hidden.
fn hidden_on(
    stack: &Levels,
    remembered: &Remembered,
    from: usize,
    to: usize,
    now: Option<Arc<[bool]>>,
) -> (Vec<bool>, Remembered) {
    let mut remembered = remembered.clone();
    remembered.resize(stack.level_count(), None);
    let nothing: Vec<bool> = vec![false; stack.triangle_count(from)];
    let here: &[bool] = now.as_deref().unwrap_or(&nothing);
    let kept = remembered[to]
        .as_ref()
        .filter(|kept| kept.len() == stack.triangle_count(to) && carried_hidden(stack, to, from, kept) == here);
    let there = match kept {
        Some(kept) => kept.to_vec(),
        None if now.is_none() => Vec::new(),
        None => carried_hidden(stack, from, to, here),
    };
    remembered[from] = now;
    (there, remembered)
}

/// A level's two numbering lists (its vertices' numbers in a mesh, and for
/// each triangle of that mesh the level's) after the mesh was rebuilt and
/// `numbering` says how. `None` if a number falls outside the lists.
fn through(numbering: &Numbering, vertex: &[u32], triangle: &[u32]) -> Option<(Vec<u32>, Vec<u32>)> {
    let vertex = vertex.iter().map(|&v| numbering.vertex.get(v as usize).copied()).collect::<Option<_>>()?;
    let triangle = numbering.triangle.iter().map(|&t| triangle.get(t as usize).copied()).collect::<Option<_>>()?;
    Some((vertex, triangle))
}

impl MeshLevels {
    pub fn count(&self) -> usize {
        self.stack.level_count()
    }

    /// The level the mesh is of; 0 is the coarsest.
    pub fn current(&self) -> usize {
        self.stack.current()
    }

    pub fn top(&self) -> usize {
        self.stack.top()
    }

    pub fn scheme(&self) -> Scheme {
        self.stack.scheme()
    }

    pub fn triangle_count(&self, level: usize) -> usize {
        self.stack.triangle_count(level)
    }

    /// Whether `level` was made by splitting faces flat, for a level above
    /// the coarsest.
    pub fn is_flat(&self, level: usize) -> bool {
        self.stack.split(level) == Some(Split::Flat)
    }

    /// This mesh's share of what its levels hold in memory (see
    /// `Levels::owned_bytes`), with its own numbering lists.
    pub fn bytes(&self) -> usize {
        let remembered: usize = self.hidden.iter().flatten().map(|flags| flags.len()).sum();
        let lists = (self.vertex.len() + self.triangle.len()) * 4;
        let top_mesh = self.top_mesh.as_ref().map_or(0, |top| top.bytes() / Arc::strong_count(top));
        self.stack.owned_bytes() + lists + remembered + self.layer_bytes() + self.groups.bytes() + top_mesh
    }

    /// What the finest copies of the sculpt layers take while the mesh is on
    /// a coarser level: twelve bytes for each layer and finest vertex, shared
    /// with every mesh kept for undo since the level was arrived at.
    pub fn layer_bytes(&self) -> usize {
        self.finest.iter().map(|(_, finest)| finest.len() * 12 / Arc::strong_count(finest)).sum()
    }

    /// The lowest number a layer made on this mesh may be given: one past
    /// every number a finest copy here goes by. A layer baked (or lost with a
    /// damaged file) below the finest level leaves its copy behind until the
    /// level is left, and a new layer must not be taken for it.
    pub(super) fn next_layer_id(&self) -> u32 {
        self.finest.iter().map(|(id, _)| id + 1).max().unwrap_or(1)
    }

    /// What every copy of the stack's tables holds, counted once: the figure
    /// to show a person, where [`Self::bytes`] is the figure to charge.
    pub fn whole_bytes(&self) -> usize {
        self.stack.memory_bytes() + (self.vertex.len() + self.triangle.len()) * 4
    }

    /// A number that is the same for the same sculpt on the same levels and,
    /// in practice, different for any other: the level, how many there are,
    /// and every vertex, mask value and hidden face in the stack's own order
    /// (so it does not depend on how the mesh happened to be numbered when
    /// it was built). For checks that compare a model before and after
    /// something that should have left it alone.
    pub fn print(&self, mesh: &Mesh) -> u64 {
        fn mix(mut x: u64) -> u64 {
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        }
        let (positions, mask) = self.level_arrays(mesh);
        let hidden = self.level_hidden(mesh);
        let mut print = mix((self.count() as u64) << 32 | self.current() as u64);
        for (v, (p, m)) in positions.iter().zip(&mask).enumerate() {
            for bits in [p.x.to_bits(), p.y.to_bits(), p.z.to_bits(), m.to_bits()] {
                print = mix(print ^ bits as u64 ^ (v as u64) << 32);
            }
        }
        for (t, &flag) in hidden.iter().flat_map(|flags| flags.iter()).enumerate() {
            print = mix(print ^ ((t as u64) << 1 | flag as u64));
        }
        for (t, &group) in self.level_groups(mesh).iter().flat_map(|groups| groups.iter()).enumerate() {
            print = mix(print ^ ((t as u64) << 17 | 1 << 16 | group as u64));
        }
        print
    }

    /// The mesh's positions in the level's own numbering, which is the same
    /// however the mesh happened to be built: for comparing a shape before
    /// and after changes of level.
    pub fn ordered_positions(&self, mesh: &Mesh) -> Vec<Vec3> {
        self.level_arrays(mesh).0
    }

    /// The mesh's face groups in the level's own triangle order, as
    /// [`Self::ordered_positions`] is of its points; empty for none.
    pub fn ordered_groups(&self, mesh: &Mesh) -> Vec<u16> {
        self.level_groups(mesh).map_or(Vec::new(), |groups| groups.to_vec())
    }

    /// The mesh's positions and mask in the stack's numbering.
    fn level_arrays(&self, mesh: &Mesh) -> (Vec<Vec3>, Vec<f32>) {
        let gather = |v: &u32| (mesh.positions[*v as usize], mesh.mask[*v as usize]);
        self.vertex.par_iter().with_min_len(PER_TASK).map(gather).unzip()
    }

    /// The mesh's hidden faces in the stack's order, if any are.
    fn level_hidden(&self, mesh: &Mesh) -> Option<Arc<[bool]>> {
        mesh.hidden_triangles().map(|hidden| placed(hidden, &self.triangle).into())
    }

    /// The mesh's face groups in the stack's order, if it has any.
    fn level_groups(&self, mesh: &Mesh) -> Option<Arc<[u16]>> {
        mesh.has_groups().then(|| placed(mesh.group_ids(), &self.triangle).into())
    }

    /// The face groups the other levels were left with, as a file keeps them
    /// beside the mesh's own: none once the mesh's groups have changed since.
    pub fn saved_groups(&self, mesh: &Mesh) -> Vec<Option<Vec<u16>>> {
        self.groups.saved(&self.level_groups(mesh))
    }

    /// The levels as a file keeps them.
    pub fn saved(&self) -> SavedLevels {
        SavedLevels {
            groups: Vec::new(),
            parts: self.stack.to_parts(),
            vertex: self.vertex.clone(),
            triangle: self.triangle.clone(),
            hidden: self.hidden.iter().map(|flags| flags.as_ref().map(|flags| flags.to_vec())).collect(),
            finest: self.finest.iter().map(|(id, finest)| (*id, finest.to_vec())).collect(),
        }
    }

    /// The levels of a mesh rebuilt from the one they were written for (a
    /// file opened, a copy made), `numbering` saying how the rebuild
    /// renumbered that mesh. `None` if they do not belong to `mesh`: the
    /// lists must be this mesh's level, triangle for triangle, or a later
    /// change of level would put vertices where they do not belong.
    pub fn restored(saved: SavedLevels, mesh: &Mesh, numbering: &Numbering) -> Option<Box<MeshLevels>> {
        let SavedLevels { parts, vertex, triangle, hidden, finest, groups } = saved;
        let stack = Levels::from_parts(parts).ok()?;
        let level = stack.current();
        let (vertices, triangles) = (stack.vertex_count(level), stack.triangle_count(level));
        if vertex.len() != vertices || numbering.vertex.len() != vertices || mesh.vertex_count() != vertices {
            return None;
        }
        if triangle.len() != triangles || numbering.triangle.len() != triangles || mesh.triangle_count() != triangles {
            return None;
        }
        let (vertex, triangle) = through(numbering, &vertex, &triangle)?;
        let hidden = (0..stack.level_count())
            .map(|level| {
                hidden.get(level).cloned().flatten().filter(|flags| flags.len() == stack.triangle_count(level))
            })
            .map(|flags| flags.map(Arc::from))
            .collect();
        // A layer's finest copy that is not the finest level's size, or holds
        // something that is not a number, would be carried into the model
        // at the next step: the levels are refused whole rather than that.
        let whole = |list: &[Vec3]| list.len() == stack.vertex_count(stack.top()) && list.iter().all(|o| o.is_finite());
        if (level == stack.top() && !finest.is_empty()) || !finest.iter().all(|(_, list)| whole(list)) {
            return None;
        }
        let finest = finest.into_iter().map(|(id, list)| (id, Arc::from(list))).collect();
        // The mesh has its own groups back already; the other levels' are good for those.
        let now = mesh.has_groups().then(|| placed(mesh.group_ids(), &triangle).into());
        let groups = grouped::Kept::restored(groups, &stack, now);
        let spread = cached::spread(&mesh.bvh);
        let levels = MeshLevels { stack, vertex, triangle, hidden, finest, groups, top_mesh: None, spread };
        levels.fits(mesh).then(|| Box::new(levels))
    }

    /// Whether the mesh is this stack's current level under the two lists:
    /// every mesh triangle the level triangle the list names, corner for
    /// corner.
    fn fits(&self, mesh: &Mesh) -> bool {
        let level = self.stack.triangles(self.stack.current());
        let in_range = |list: &[u32], below: usize| list.par_iter().all(|&i| (i as usize) < below);
        if !in_range(&self.vertex, mesh.vertex_count()) || !in_range(&self.triangle, level.len() / 3) {
            return false;
        }
        mesh.indices.par_chunks_exact(3).zip(&self.triangle).with_min_len(PER_TASK).all(|(corners, &t)| {
            let of_level = &level[t as usize * 3..t as usize * 3 + 3];
            (0..3).all(|k| self.vertex[of_level[k] as usize] == corners[k])
        })
    }

    /// These levels for a copy of their mesh that `numbering` renumbered: an
    /// independent copy of an object keeps its levels.
    pub fn renumbered(&self, copy: &Mesh, numbering: &Numbering) -> Option<Box<MeshLevels>> {
        let (vertex, triangle) = through(numbering, &self.vertex, &self.triangle)?;
        let (stack, hidden, finest) = (self.stack.clone(), self.hidden.clone(), self.finest.clone());
        // The finest level's mesh is the stack's, not this mesh's: the copy can step up to it as well.
        let (top_mesh, spread) = (self.top_mesh.clone(), cached::spread(&copy.bvh));
        let groups = self.groups.clone();
        let levels = MeshLevels { stack, vertex, triangle, hidden, finest, groups, top_mesh, spread };
        let same_size = copy.vertex_count() == self.vertex.len() && copy.triangle_count() == self.triangle.len();
        (same_size && levels.fits(copy)).then(|| Box::new(levels))
    }
}

/// The mesh of the level `stack` is on, as the stack handed it over (its
/// positions and mask, and its faces if they were handed over too), carrying
/// the stack and the mesh's sculpt layers as they are on that level.
/// `top_mesh` is what the finest level's mesh was built into, if that is
/// known: it makes the finest level quicker to build, and goes with a
/// coarser one.
#[allow(clippy::too_many_arguments)]
fn built(
    stack: Levels,
    (positions, mask, faces): (Vec<Vec3>, Vec<f32>, Option<(Vec<u32>, Option<sculpt_multires::Polys>)>),
    hidden: &[bool],
    remembered: Remembered,
    riding: Option<layered::Riding>,
    (groups, mut kept): (Vec<u16>, grouped::Kept),
    top_mesh: Option<Arc<cached::TopMesh>>,
) -> Result<Mesh, String> {
    let level = stack.current();
    let (vertices, count) = (positions.len(), stack.triangle_count(level));
    let on_top = level == stack.top();
    let reused = top_mesh.as_ref().filter(|_| on_top).and_then(|top| top.mesh(&positions, &mask, hidden));
    let (mut mesh, numbering, spread, polys) = match reused {
        Some((mesh, numbering, spread)) => {
            let polys = faces.map_or_else(|| stack.polys(level), |(_, polys)| polys);
            (mesh, numbering, spread, polys)
        }
        None => {
            let (triangles, polys) = faces.unwrap_or_else(|| stack.faces(level));
            let (mesh, numbering) = Mesh::new_numbered(positions, triangles, Some(mask), hidden);
            let spread = cached::spread(&mesh.bvh);
            (mesh, numbering, spread, polys)
        }
    };
    // A level has no vertex that no face uses and no face that is not one:
    // if a build left something out, the two numberings cannot be matched.
    if mesh.vertex_count() != vertices || mesh.triangle_count() != count {
        return Err("This mesh has loose points or collapsed faces, which subdivision levels cannot keep track of. \
                    Remesh it first."
            .into());
    }
    mesh.polys = polys.map(|polys| Polys {
        start: polys.start,
        indices: polys.indices.iter().map(|&v| numbering.vertex[v as usize]).collect(),
    });
    let mut finest = Vec::new();
    if let Some(riding) = riding {
        mesh.layers = Layers::restored(riding.layers, vertices, Some(&numbering.vertex));
        finest = riding.finest;
    }
    // The level's face groups, and the list the mesh ends up with: a quad's
    // two triangles are made to agree as it goes on.
    mesh.set_group_ids(super::groups::renumbered(&groups, &numbering.triangle));
    kept.arrive(mesh.has_groups().then(|| placed(mesh.group_ids(), &numbering.triangle).into()));
    mesh.levels = Some(Box::new(MeshLevels {
        stack,
        vertex: numbering.vertex,
        triangle: numbering.triangle,
        hidden: remembered,
        finest,
        groups: kept,
        top_mesh: top_mesh.filter(|_| !on_top),
        spread,
    }));
    Ok(mesh)
}

/// The levels of a mesh that has none yet: one level, the mesh as it is,
/// numbered as the mesh is. With `quads` false its quads are left out.
fn first_levels(mesh: &Mesh, quads: bool) -> Result<MeshLevels, String> {
    let polys = mesh.polys.as_ref().filter(|_| quads);
    let polys =
        polys.map(|polys| sculpt_multires::Polys { start: polys.start.clone(), indices: polys.indices.clone() });
    let stack = Levels::new(mesh.vertex_count(), mesh.indices.clone(), polys)?;
    Ok(MeshLevels {
        stack,
        vertex: (0..mesh.vertex_count() as u32).collect(),
        triangle: (0..mesh.triangle_count() as u32).collect(),
        hidden: vec![None],
        finest: Vec::new(),
        groups: grouped::Kept::default(),
        top_mesh: None,
        spread: cached::spread(&mesh.bvh),
    })
}

/// How many triangles Subdivide would give `mesh`, which must be on its
/// finest level: four for each it has now. (A face of a quad remesh that is
/// not a quad makes a few more or fewer on the first step; this is the
/// figure to show beforehand, not a promise.)
pub fn triangles_after(mesh: &Mesh) -> usize {
    mesh.triangle_count() * 4
}

/// Why `mesh` cannot be subdivided, if it cannot.
pub fn subdivide_refusal(mesh: &Mesh) -> Option<String> {
    if mesh.triangle_count() == 0 {
        return Some("There is nothing here to subdivide.".into());
    }
    if mesh.levels.as_ref().is_some_and(|levels| levels.current() != levels.top()) {
        return Some(
            "Subdivide adds a level above the finest one. Step up to the finest level first (Page Up).".into(),
        );
    }
    let after = triangles_after(mesh);
    (after > MOST_TRIANGLES).then(|| {
        format!(
            "Subdividing would make {:.1} million triangles; the most a level can have is {} million.",
            after as f64 / 1e6,
            MOST_TRIANGLES / 1_000_000
        )
    })
}

/// `mesh` with a finer level added above its finest, as a new mesh on that
/// level. `mesh` itself is not changed: it is what Undo goes back to.
pub fn subdivided(mesh: &Mesh, split: Split) -> Result<(Mesh, Subdivided), String> {
    if let Some(why) = subdivide_refusal(mesh) {
        return Err(why);
    }
    let mut note = Subdivided::default();
    let first;
    let levels = match &mesh.levels {
        Some(levels) => &**levels,
        None => {
            // Quads that are not the triangles cut up (a file from elsewhere,
            // say) cannot be subdivided as quads; the triangles still can.
            first = first_levels(mesh, true).or_else(|_| {
                note.quads_set_aside = mesh.polys.is_some();
                first_levels(mesh, false)
            })?;
            &first
        }
    };
    let from = levels.current();
    let (positions, mask) = levels.level_arrays(mesh);
    let mut stack = levels.stack.clone();
    let level = stack.subdivide(&positions, &mask, split)?;
    let (hidden, remembered) = hidden_on(&stack, &levels.hidden, from, from + 1, levels.level_hidden(mesh));
    let riding = layered::subdivided(mesh, levels, &stack);
    let groups = levels.groups.step(&stack, from, from + 1, levels.level_groups(mesh));
    let LevelMesh { positions, mask, triangles, polys } = level;
    let lists = (positions, mask, Some((triangles, polys)));
    // A new finest level: whatever was kept of the old one is no use now.
    Ok((built(stack, lists, &hidden, remembered, riding, groups, None)?, note))
}

/// `mesh` stepped to level `to`, as a new mesh, and whether anything
/// sculpted on the level being left had to be carried up into the finer
/// levels on the way. `mesh` itself is not changed.
pub fn stepped(mesh: &Mesh, to: usize) -> Result<(Mesh, bool), String> {
    let levels = mesh.levels.as_deref().ok_or("This object has no subdivision levels.")?;
    let from = levels.current();
    let (positions, mask) = levels.level_arrays(mesh);
    let mut stack = levels.stack.clone();
    let (arrived, arrived_mask, undo) = stack.step_lists(to, &positions, &mask)?;
    let carried = undo.carried_an_edit();
    let (hidden, remembered) = hidden_on(&stack, &levels.hidden, from, to, levels.level_hidden(mesh));
    let riding = layered::stepped(mesh, levels, &positions, &mask, to, &stack, &arrived);
    let groups = levels.groups.step(&stack, from, to, levels.level_groups(mesh));
    // Leaving the finest level, what its mesh was built into is kept for the way back.
    let top_mesh = match from == levels.top() {
        true => Some(Arc::new(cached::TopMesh::of(mesh, levels))),
        false => levels.top_mesh.clone(),
    };
    let lists = (arrived, arrived_mask, None);
    Ok((built(stack, lists, &hidden, remembered, riding, groups, top_mesh)?, carried))
}

/// The levels `mesh` would have with every level below its own dropped
/// (`lower`) or every level above it. `None` when that leaves one level: a
/// mesh of one level is a mesh without levels. The mesh itself is the same
/// either way, so only its levels change hands.
pub fn without(mesh: &Mesh, lower: bool) -> Option<Box<MeshLevels>> {
    let levels = mesh.levels.as_deref()?;
    let (mut stack, mut hidden, mut finest) = (levels.stack.clone(), levels.hidden.clone(), levels.finest.clone());
    hidden.resize(stack.level_count(), None);
    let level = stack.current();
    if lower {
        stack.delete_lower();
        hidden.drain(..level);
    } else {
        // This level is the finest now: what the mesh's layers hold is all of them.
        stack.delete_higher();
        hidden.truncate(level + 1);
        finest.clear();
    }
    let groups = levels.groups.without(level, lower);
    // Dropping the levels below leaves the finest level as it was, numbering
    // and all; dropping those above makes this one the finest.
    let top_mesh = levels.top_mesh.clone().filter(|_| lower);
    (stack.level_count() > 1).then(|| {
        let (vertex, triangle) = (levels.vertex.clone(), levels.triangle.clone());
        Box::new(MeshLevels { stack, vertex, triangle, hidden, finest, groups, top_mesh, spread: levels.spread })
    })
}

#[cfg(test)]
mod tests;
