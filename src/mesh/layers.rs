//! Sculpt layers: passes of sculpting kept apart from the model underneath,
//! each of which can be turned down, switched off or thrown away without
//! touching the rest (pores, wrinkles, a scar, another expression).
//!
//! **A layer is an offset for every vertex.** The shape is the model
//! underneath plus, for each layer, its offsets times its strength. Only the
//! offsets are stored. `Mesh::positions` holds the finished shape, as it
//! always has, so everything that reads the mesh (brushes, picking, export,
//! the printing check, a measure) reads the shape with every layer in it and
//! knows nothing of layers; and the model underneath is not stored at all: it
//! is whatever the positions are less the layers.
//!
//! That one choice decides what every edit has to do:
//!
//! - **A stroke with a layer recording** moves the positions, as any stroke
//!   does, and when it ends what it moved is added to that layer's offsets,
//!   divided by the layer's strength. The shape on screen is exactly what was
//!   sculpted, at any strength.
//! - **A stroke with no layer recording** changes the model underneath
//!   simply by not telling any layer. What the layers have to be told
//!   depends on the kind of stroke (`Ride`), because a vertex on screen is
//!   the model plus its offsets and the stroke moved that:
//!   - A brush that **pushes** the surface (all but Pose) moves the two ends
//!     of an offset the same way, to within how much the push changes over
//!     the offset's own length. So the offsets are left exactly as they are,
//!     and the layer switched off afterwards leaves the model as the stroke
//!     would have left it bare. (The offsets now lean against the skin that
//!     tilted under them, which is what the stroke did to the shape on
//!     screen. Turning them upright with the surface instead was measured:
//!     under a hard pull with the Move brush it left more than the layer's
//!     own depth behind, where leaving them leaves a quarter of it:
//!     `a_push_underneath_leaves_the_layers_standing_as_they_stood`.)
//!   - A move that **turns** what it moves (the Pose brush swinging a limb,
//!     the gizmo turning or scaling the unmasked part) turns both ends of an
//!     offset about a point, so the offsets are turned with the surface
//!     (`sculpt_multires::Turn`, the rule the subdivision levels turn their
//!     own detail by), or carried by the gizmo's own matrix. Without that a
//!     layer switched off after a pose would take its detail off in the
//!     direction the skin used to face, and leave a ghost of it behind.
//!
//!   Either way nothing moves on screen: it is bookkeeping about which part
//!   of the shape is the layer's. What each vertex's offsets have been
//!   carried by since the mesh was built is kept (`Layers::frame`), so that
//!   Reset Shape, which puts the model underneath back where it was built,
//!   can carry them back by exactly that.
//! - **A change of strength** (the slider, the eye, Delete) moves each
//!   vertex by the change in strength times its offset. A drag of the slider
//!   works every position out from where it was at the press, so a long drag
//!   rounds once and not once a frame.
//! - **Bake** forgets the layer and moves nothing: what it added is the
//!   model underneath from then on.
//!
//! **Rounding.** A stroke, and Bake, change no position by arithmetic, so the
//! sculpted shape is kept to the bit. A change of strength is one
//! multiplication and one addition per coordinate, rounded once: taking a
//! layer to 0% and back to 100% returns each coordinate to within one unit in
//! its last place (about a ten-millionth of the model's size). Undo does not
//! go by arithmetic at all: every step keeps the positions and offsets it
//! replaced and puts those back.
//!
//! **Strength is whole percent**, 0 to 100, and a layer that is off or at 0%
//! cannot record (the app refuses the stroke): there would be nothing to
//! divide by. At 1% a stroke is stored a hundred times its size, which is the
//! most it can be magnified, and is what the sculptor will see if they then
//! raise the slider: the card says so while a layer records below 100%.
//!
//! **Layers live in the mesh**, as the subdivision levels do (`levels.rs`
//! says why): instances share them, a mesh kept whole for undo keeps them,
//! and whatever builds a new mesh from the positions (remesh, decimate, a
//! boolean, a join...) gets the shape with every layer in it at its strength
//! and no layers, without having to know they exist.
//!
//! **Dynamic topology** gives a vertex made by a split the mean of the
//! offsets at the two ends of the edge it split, as it gives it the mean of
//! their mask. The split itself decides where the vertex goes, so nothing
//! jumps; and the model underneath gets a vertex in the middle of its own
//! edge, so a layer switched off afterwards leaves a smooth surface.
//!
//! **Subdivision levels** carry the layers with the mesh: see
//! `levels/layered.rs`. Below the finest level a layer can record, be made,
//! renamed and baked, but its strength cannot be changed (nor the layer
//! deleted): that would have to move the finest level, which is not the mesh
//! in hand (`Mesh::layer_strength_refusal`).

use glam::{DMat3, DVec3, Mat3, Vec3};
use rayon::prelude::*;
use sculpt_multires::{Fan, Turn};

use super::{Carry, Mesh, StrokeDelta};

/// Vertices handed to one rayon task.
const PER_TASK: usize = 1 << 12;

/// The most layers the app lets one mesh have. Each is twelve bytes a
/// vertex, and a list longer than this no longer fits its card.
pub const MOST_LAYERS: usize = 24;

/// How a stroke on the model underneath carries the layers' offsets. A
/// vertex on screen is the model plus its offsets, the stroke moved that, and
/// what the layer must be told is where the stroke sent the offset's two
/// ends: which depends on what kind of move it was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ride {
    /// Turned with the surface: for a move that turns what it moves (the
    /// Pose brush, which swings a limb round a joint), and for a move of a
    /// kind nobody has said.
    #[default]
    Turned,
    /// Left pointing where they point: for a move that pushes the surface
    /// along (every other brush). A push carries the two ends of an offset
    /// the same way, to within how much the push changes across the offset's
    /// own length, so the offset is as it was: it now leans against the skin
    /// that tilted under it, which is what the stroke did to it on screen.
    Still,
}

/// One layer. Its offsets are private: they change only with the positions.
pub struct Layer {
    id: u32,
    name: String,
    on: bool,
    percent: u8,
    offsets: Vec<Vec3>,
}

impl Layer {
    /// How much of its offsets is in the shape: 0 while it is off.
    fn strength(&self) -> f64 {
        if self.on { self.percent as f64 / 100.0 } else { 0.0 }
    }
}

/// A mesh's layers, in the order of the list (the first made at the top).
pub struct Layers {
    list: Vec<Layer>,
    /// The layer strokes are recorded into, if any.
    recording: Option<u32>,
    /// The number the next layer made will have. Numbers are never handed
    /// out twice, so an undo record that names a layer cannot mean another.
    next_id: u32,
    /// What every vertex's offsets have been carried by, by moves of the
    /// model underneath, since the mesh was built: the product of the
    /// matrices, a list for each of its three columns (so that whatever keeps
    /// the layers' own lists in step with the vertices keeps these too).
    /// `None` until something has carried them: most brushes never do.
    frame: Option<Box<[Vec<Vec3>; 3]>>,
}

/// The numbers the frame's three columns go by where a layer goes by its
/// own: in a stroke's undo record. No layer is ever given one this high.
const FRAME: u32 = u32::MAX - 2;

/// One row of the Layers card.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRow {
    pub id: u32,
    pub name: String,
    pub on: bool,
    pub percent: u8,
    pub recording: bool,
}

/// A layer as a file keeps it, or as a level hands it to the next: its row
/// and its offsets, in whatever vertex order the holder says.
pub struct SavedLayer {
    pub id: u32,
    pub name: String,
    pub on: bool,
    pub percent: u8,
    pub offsets: Vec<Vec3>,
}

/// A mesh's layers as plain lists.
#[derive(Default)]
pub struct SavedLayers {
    pub layers: Vec<SavedLayer>,
    pub recording: Option<u32>,
    pub next_id: u32,
}

/// The layers on the other side of an undo step that changed them: the
/// list, and the positions where the two sides differ. Stepping across it
/// swaps both with what the mesh has, so it serves undo and redo alike and
/// puts back exactly what was there.
pub struct LayersStep {
    rows: Vec<StepRow>,
    recording: Option<u32>,
    next_id: u32,
    verts: Vec<u32>,
    positions: Vec<Vec3>,
    /// The mesh's frame (`Layers::frame`), kept here while the mesh is on
    /// the side that has no layers to keep it with.
    frame: Option<Box<[Vec<Vec3>; 3]>>,
}

struct StepRow {
    id: u32,
    name: String,
    on: bool,
    percent: u8,
    /// The layer's offsets, for a layer the mesh does not have on the side
    /// it is on now (one that was deleted or baked, seen from after).
    offsets: Option<Vec<Vec3>>,
}

/// A drag of a layer's Strength slider (or a flick of its eye) from the
/// press on: the vertices the layer moves and where they were, so that every
/// position along the way is worked out from there.
pub struct StrengthDrag {
    id: u32,
    before: (bool, u8),
    verts: Vec<u32>,
    positions: Vec<Vec3>,
    leaves: Vec<u32>,
}

/// What a stroke did to the layers, beside what it did to the positions:
/// the old and new offsets of the vertices it touched, layer by layer (and
/// the frame's columns likewise), and the offsets of the vertices dynamic
/// topology added.
pub struct StrokeLayers {
    changes: Vec<Change>,
    grown: Option<Grown>,
}

struct Change {
    layer: u32,
    verts: Vec<u32>,
    before: Vec<Vec3>,
    after: Vec<Vec3>,
}

impl Change {
    fn of(layer: u32) -> Change {
        Change { layer, verts: Vec::new(), before: Vec::new(), after: Vec::new() }
    }

    /// Notes that the list's entry for `v` went from `before` to `after`.
    fn note(&mut self, v: u32, before: Vec3, after: Vec3) {
        self.verts.push(v);
        self.before.push(before);
        self.after.push(after);
    }
}

/// The vertices a stroke added, numbered from `from`, and every layer's
/// offsets for them as the stroke left them.
struct Grown {
    from: u32,
    offsets: Vec<(u32, Vec<Vec3>)>,
}

impl StrokeLayers {
    pub fn bytes(&self) -> usize {
        let changed: usize = self.changes.iter().map(|c| c.verts.len() * 28).sum();
        let grown: usize = self.grown.iter().flat_map(|g| &g.offsets).map(|(_, offsets)| offsets.len() * 12).sum();
        changed + grown
    }

    /// After the positions went back (`undo`) or forward: the offsets go
    /// with them.
    fn apply(&self, layers: &mut Layers, undo: bool) {
        if let Some(grown) = &self.grown {
            for (id, list) in layers.lists_mut() {
                list.truncate(grown.from as usize);
                let new = grown.offsets.iter().find(|(of, _)| *of == id).map(|(_, offsets)| &offsets[..]);
                if let (false, Some(new)) = (undo, new) {
                    list.extend_from_slice(new);
                }
            }
        }
        for change in &self.changes {
            let Some((_, list)) = layers.lists_mut().find(|(id, _)| *id == change.layer) else { continue };
            let values = if undo { &change.before } else { &change.after };
            for (&v, &offset) in change.verts.iter().zip(values) {
                if let Some(slot) = list.get_mut(v as usize) {
                    *slot = offset;
                }
            }
        }
    }
}

fn same(a: Vec3, b: Vec3) -> bool {
    a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits)
}

/// The faces round one vertex of a mesh at the end of a stroke: as they were
/// when the stroke began and as they are.
struct StrokeFan<'a> {
    mesh: &'a Mesh,
    vertex: u32,
}

impl StrokeFan<'_> {
    /// Where a vertex was when the stroke began. (`Mesh::start_pos` answers
    /// only while the stroke is open.)
    fn start(&self, v: u32) -> DVec3 {
        let (mesh, i) = (self.mesh, v as usize);
        let moved = mesh.stamp[i] == mesh.stroke_id;
        if moved { mesh.orig_pos[i].as_dvec3() } else { mesh.positions[i].as_dvec3() }
    }
}

impl Fan for StrokeFan<'_> {
    fn wedges(&self, mut wedge: impl FnMut([DVec3; 2], [DVec3; 2])) {
        let mesh = self.mesh;
        let now = |v: u32| mesh.positions[v as usize].as_dvec3();
        let (centre_was, centre_now) = (self.start(self.vertex), now(self.vertex));
        for &t in mesh.vt.row(self.vertex as usize) {
            let c = &mesh.indices[t as usize * 3..t as usize * 3 + 3];
            let Some(k) = c.iter().position(|&v| v == self.vertex) else { continue };
            let (a, b) = (c[(k + 1) % 3], c[(k + 2) % 3]);
            wedge([self.start(a) - centre_was, self.start(b) - centre_was], [now(a) - centre_now, now(b) - centre_now]);
        }
    }
}

impl Layers {
    fn new() -> Layers {
        Layers { list: Vec::new(), recording: None, next_id: 1, frame: None }
    }

    /// Every list that has an entry for each vertex: the layers' offsets
    /// under the layers' numbers, and the frame's columns under theirs.
    fn lists(&self) -> impl Iterator<Item = (u32, &Vec<Vec3>)> {
        let frame = self.frame.iter().flat_map(|frame| frame.iter().enumerate());
        self.list.iter().map(|layer| (layer.id, &layer.offsets)).chain(frame.map(|(k, column)| (FRAME + k as u32, column)))
    }

    fn lists_mut(&mut self) -> impl Iterator<Item = (u32, &mut Vec<Vec3>)> {
        let frame = self.frame.iter_mut().flat_map(|frame| frame.iter_mut().enumerate());
        let layers = self.list.iter_mut().map(|layer| (layer.id, &mut layer.offsets));
        layers.chain(frame.map(|(k, column)| (FRAME + k as u32, column)))
    }

    /// What vertex `v`'s offsets have been carried by since the mesh was built.
    fn frame_at(&self, v: usize) -> Mat3 {
        self.frame.as_ref().map_or(Mat3::IDENTITY, |frame| Mat3::from_cols(frame[0][v], frame[1][v], frame[2][v]))
    }

    /// [`Self::frame_at`] for every vertex in the order `place` gives (entry
    /// `i` is vertex `place[i]`'s), or `None` if nothing has carried them.
    pub fn frames(&self, place: &[u32]) -> Option<Vec<Mat3>> {
        self.frame.as_ref()?;
        Some(place.par_iter().with_min_len(PER_TASK).map(|&v| self.frame_at(v as usize)).collect())
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn rows(&self) -> Vec<LayerRow> {
        let row = |l: &Layer| LayerRow {
            id: l.id,
            name: l.name.clone(),
            on: l.on,
            percent: l.percent,
            recording: self.recording == Some(l.id),
        };
        self.list.iter().map(row).collect()
    }

    pub fn row(&self, id: u32) -> Option<LayerRow> {
        self.rows().into_iter().find(|row| row.id == id)
    }

    /// The layer strokes are recorded into, whether or not it can take one
    /// just now.
    pub fn recording(&self) -> Option<LayerRow> {
        self.row(self.recording?)
    }

    /// What every layer's offsets take in memory: twelve bytes a vertex for
    /// each layer.
    pub fn bytes(&self) -> usize {
        let frame = self.frame.as_ref().map_or(0, |frame| frame[0].len() * 36);
        self.list.iter().map(|layer| layer.offsets.len() * 12 + layer.name.len()).sum::<usize>() + frame
    }

    /// The offsets of layer `id`, one per vertex: for whatever needs to read
    /// what a layer holds (a brush that rubs a layer out under the ring, when
    /// there is one).
    pub fn offsets(&self, id: u32) -> Option<&[Vec3]> {
        self.list.iter().find(|layer| layer.id == id).map(|layer| &layer.offsets[..])
    }

    fn index_of(&self, id: u32) -> Option<usize> {
        self.list.iter().position(|layer| layer.id == id)
    }

    /// The place in the list of the layer that would take a stroke now: the
    /// recording layer, if it is on and above 0%.
    fn taking(&self) -> Option<usize> {
        let index = self.index_of(self.recording?)?;
        (self.list[index].strength() > 0.0).then_some(index)
    }

    /// The rows as an undo step keeps them, with no offsets.
    fn step_rows(&self) -> Vec<StepRow> {
        let row = |l: &Layer| StepRow { id: l.id, name: l.name.clone(), on: l.on, percent: l.percent, offsets: None };
        self.list.iter().map(row).collect()
    }

    /// Every layer's offset (and the frame) at a vertex made between `a` and `b`.
    pub(super) fn split(&mut self, a: u32, b: u32) {
        for (_, list) in self.lists_mut() {
            let between = (list[a as usize] + list[b as usize]) * 0.5;
            list.push(between);
        }
    }

    /// Hands each per-vertex list to `sort`, to be put in a new vertex order.
    pub(super) fn reorder(&mut self, mut sort: impl FnMut(&mut [Vec3])) {
        self.lists_mut().for_each(|(_, list)| sort(list));
    }

    /// The layers as plain lists, each layer's offsets put in the order
    /// `place` gives: entry `i` of the result is the offset of vertex
    /// `place[i]`.
    pub fn saved(&self, place: Option<&[u32]>) -> SavedLayers {
        let ordered = |offsets: &[Vec3]| match place {
            Some(place) => place.par_iter().with_min_len(PER_TASK).map(|&v| offsets[v as usize]).collect(),
            None => offsets.to_vec(),
        };
        let layer = |l: &Layer| SavedLayer {
            id: l.id,
            name: l.name.clone(),
            on: l.on,
            percent: l.percent,
            offsets: ordered(&l.offsets),
        };
        SavedLayers { layers: self.list.iter().map(layer).collect(), recording: self.recording, next_id: self.next_id }
    }

    /// Layers from plain lists, for a mesh of `vertices` vertices where
    /// entry `i` of each layer's offsets belongs to vertex `place[i]` (or to
    /// vertex `i` with no `place`). An entry placed at `u32::MAX` belongs to
    /// a vertex the mesh was built without (one no triangle used, as dynamic
    /// topology's merges leave), and is let go. `None` if a list is not that
    /// mesh's, or holds something that is not a number.
    pub fn restored(saved: SavedLayers, vertices: usize, place: Option<&[u32]>) -> Option<Box<Layers>> {
        let SavedLayers { layers, recording, next_id } = saved;
        if layers.is_empty() {
            return None;
        }
        let entries = place.map_or(vertices, <[u32]>::len);
        let mut list = Vec::with_capacity(layers.len());
        for layer in layers {
            let fits = layer.offsets.len() == entries && layer.offsets.par_iter().all(|offset| offset.is_finite());
            if !fits || list.iter().any(|other: &Layer| other.id == layer.id) {
                return None;
            }
            let offsets = match place {
                Some(place) => {
                    let mut placed = vec![Vec3::ZERO; vertices];
                    for (&offset, &to) in layer.offsets.iter().zip(place).filter(|&(_, &to)| to != u32::MAX) {
                        *placed.get_mut(to as usize)? = offset;
                    }
                    placed
                }
                None => layer.offsets,
            };
            list.push(Layer { id: layer.id, name: layer.name, on: layer.on, percent: layer.percent.min(100), offsets });
        }
        let most = list.iter().map(|layer| layer.id).max().unwrap_or(0);
        let recording = recording.filter(|id| list.iter().any(|layer| layer.id == *id));
        // A mesh built from lists is a new mesh: nothing has carried its offsets yet.
        Some(Box::new(Layers { list, recording, next_id: next_id.max(most + 1), frame: None }))
    }
}

impl LayersStep {
    /// What the step holds in memory: the offsets of layers only it has, and
    /// the positions it keeps.
    pub fn bytes(&self) -> usize {
        let offsets: usize = self.rows.iter().flat_map(|row| &row.offsets).map(|offsets| offsets.len() * 12).sum();
        let frame = self.frame.as_ref().map_or(0, |frame| frame[0].len() * 36);
        offsets + frame + self.verts.len() * 16 + self.rows.iter().map(|row| row.name.len() + 16).sum::<usize>()
    }

    /// Swaps the layers and positions the step keeps with the mesh's.
    pub fn swap(&mut self, mesh: &mut Mesh) {
        debug_assert!(!mesh.in_stroke);
        let vertices = mesh.positions.len();
        let Layers { list, recording, next_id, frame } = mesh.layers.take().map_or_else(Layers::new, |layers| *layers);
        // The mesh's side, to keep: every layer with its offsets, until the
        // other side turns out to have the same layer and takes them over.
        let mut mine: Vec<StepRow> = list
            .into_iter()
            .map(|l| StepRow { id: l.id, name: l.name, on: l.on, percent: l.percent, offsets: Some(l.offsets) })
            .collect();
        let mut theirs = Vec::with_capacity(self.rows.len());
        for row in std::mem::take(&mut self.rows) {
            let shared = |mine: &mut Vec<StepRow>| mine.iter_mut().find(|m| m.id == row.id).and_then(|m| m.offsets.take());
            let mut offsets = row.offsets.or_else(|| shared(&mut mine)).unwrap_or_default();
            offsets.resize(vertices, Vec3::ZERO);
            theirs.push(Layer { id: row.id, name: row.name, on: row.on, percent: row.percent, offsets });
        }
        // The frame is the mesh's and stays with it: on its layers while it
        // has any, and here while it has none.
        let frame = frame.or_else(|| self.frame.take()).filter(|frame| frame[0].len() == vertices);
        let mut there = Layers { list: theirs, recording: self.recording, next_id: self.next_id, frame: None };
        if there.list.is_empty() {
            self.frame = frame;
        } else {
            there.frame = frame;
            mesh.layers = Some(Box::new(there));
        }
        (self.rows, self.recording, self.next_id) = (mine, recording, next_id);

        for (&v, kept) in self.verts.iter().zip(&mut self.positions) {
            std::mem::swap(&mut mesh.positions[v as usize], kept);
        }
        if !self.verts.is_empty() {
            let leaves = mesh.leaves_of(&self.verts);
            mesh.positions_changed(&leaves);
        }
    }
}

impl Mesh {
    /// The rows of the Layers card, top first; none for a mesh without layers.
    pub fn layer_rows(&self) -> Vec<LayerRow> {
        self.layers.as_ref().map_or_else(Vec::new, |layers| layers.rows())
    }

    pub fn layer_count(&self) -> usize {
        self.layers.as_ref().map_or(0, |layers| layers.len())
    }

    /// A number that is the same for the same shape under the same layers
    /// and, in practice, different for any other: every vertex's position
    /// with every layer's offset there, and the rows. It does not depend on
    /// how the mesh is numbered, so it can be compared across a rebuild (a
    /// file opened, a change of subdivision level). For checks that compare
    /// a model before and after something that should have left it alone.
    pub fn layers_print(&self) -> u64 {
        fn mix(mut x: u64) -> u64 {
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        }
        let fold = |print: u64, point: Vec3| point.to_array().iter().fold(print, |p, c| mix(p ^ c.to_bits() as u64));
        let lists: Vec<&[Vec3]> = self.layers.iter().flat_map(|layers| &layers.list).map(|l| &l.offsets[..]).collect();
        let vertex = |v: usize| lists.iter().fold(fold(1, self.positions[v]), |print, list| fold(print, list[v]));
        let shape = (0..self.positions.len()).into_par_iter().map(vertex).reduce(|| 0, u64::wrapping_add);
        let row = |print: u64, row: &LayerRow| {
            let flags = (row.id as u64) << 32 | (row.percent as u64) << 8 | (row.on as u64) << 1 | row.recording as u64;
            row.name.bytes().fold(mix(print ^ flags), |print, byte| mix(print ^ byte as u64))
        };
        self.layer_rows().iter().fold(mix(shape), row)
    }

    /// Why a stroke that moves the surface cannot be taken just now, if it
    /// cannot: the layer that would record it is off or at 0%, so there is
    /// no strength to divide what the stroke moves by.
    pub fn layer_refusal(&self) -> Option<String> {
        let recording = self.layers.as_ref()?.recording()?;
        let why = match (recording.on, recording.percent) {
            (false, _) => "is switched off",
            (true, 0) => "is at 0%",
            _ => return None,
        };
        Some(format!(
            "The layer \"{}\" is recording but {why}, so there is nothing to sculpt on. Switch it on and raise its \
             Strength, or stop recording to sculpt the model underneath.",
            recording.name
        ))
    }

    /// The layers as they are, with no offsets and no positions: the start
    /// of an undo step for a change about to be made.
    fn layers_step(&self) -> LayersStep {
        let (rows, recording, next_id) = match &self.layers {
            Some(layers) => (layers.step_rows(), layers.recording, layers.next_id),
            None => (Vec::new(), None, 1),
        };
        LayersStep { rows, recording, next_id, verts: Vec::new(), positions: Vec::new(), frame: None }
    }

    /// Adds an empty layer at the bottom of the list, recording, and gives
    /// back its number with the undo step. Without a name it is "Layer 1",
    /// or the first such name not in use.
    pub fn add_layer(&mut self, name: Option<String>) -> (u32, LayersStep) {
        let step = self.layers_step();
        let vertices = self.positions.len();
        let least = self.levels.as_ref().map_or(1, |levels| levels.next_layer_id());
        let layers = self.layers.get_or_insert_with(|| Box::new(Layers::new()));
        layers.next_id = layers.next_id.max(least);
        let taken = |name: &str| layers.list.iter().any(|layer| layer.name == name);
        let name = name.map(|name| name.trim().to_owned()).filter(|name| !name.is_empty());
        let name = name.unwrap_or_else(|| (1..).map(|n| format!("Layer {n}")).find(|name| !taken(name)).unwrap());
        let id = layers.next_id;
        layers.next_id += 1;
        layers.list.push(Layer { id, name, on: true, percent: 100, offsets: vec![Vec3::ZERO; vertices] });
        layers.recording = Some(id);
        (id, step)
    }

    /// Gives layer `id` another name. `None` if there is no such layer or
    /// the name is the one it has.
    pub fn rename_layer(&mut self, id: u32, name: &str) -> Option<LayersStep> {
        let name = name.trim();
        let index = self.layers.as_ref()?.index_of(id)?;
        if name.is_empty() || self.layers.as_ref()?.list[index].name == name {
            return None;
        }
        let step = self.layers_step();
        self.layers.as_mut()?.list[index].name = name.to_owned();
        Some(step)
    }

    /// Makes layer `id` the one strokes are recorded into, or with `None`
    /// stops recording, so that strokes change the model underneath.
    pub fn record_on_layer(&mut self, id: Option<u32>) -> Option<LayersStep> {
        let layers = self.layers.as_ref()?;
        if layers.recording == id || id.is_some_and(|id| layers.index_of(id).is_none()) {
            return None;
        }
        let step = self.layers_step();
        self.layers.as_mut()?.recording = id;
        Some(step)
    }

    /// Why a layer's strength cannot be changed just now, if it cannot (nor
    /// its eye flicked, nor the layer deleted, which are changes of strength):
    /// the mesh is on a subdivision level below its finest. The layer's
    /// detail is on the finest level, which the level stack is keeping, and
    /// would have to move there too.
    pub fn layer_strength_refusal(&self) -> Option<&'static str> {
        self.below_finest().then_some(
            "A layer's Strength, its eye and Delete work on the finest subdivision level, where the layer's detail \
             is. Step up to the finest level first (Page Up).",
        )
    }

    /// Opens a change of layer `id`'s strength: notes the vertices the layer
    /// moves and where they are now. `None` if there is no such layer or its
    /// strength cannot be changed ([`Mesh::layer_strength_refusal`]).
    pub fn drag_layer(&self, id: u32) -> Option<StrengthDrag> {
        if self.below_finest() {
            return None;
        }
        let layers = self.layers.as_ref()?;
        let layer = &layers.list[layers.index_of(id)?];
        let verts: Vec<u32> = (0..layer.offsets.len() as u32)
            .into_par_iter()
            .with_min_len(PER_TASK)
            .filter(|&v| layer.offsets[v as usize] != Vec3::ZERO)
            .collect();
        let positions = verts.iter().map(|&v| self.positions[v as usize]).collect();
        let leaves = self.leaves_of(&verts);
        Some(StrengthDrag { id, before: (layer.on, layer.percent), verts, positions, leaves })
    }

    /// Puts the layer of `drag` at `percent`, on or off, and the shape with
    /// it: each vertex goes to where it was at the press plus the change in
    /// strength times its offset.
    pub fn drag_layer_to(&mut self, drag: &StrengthDrag, on: bool, percent: u8) {
        let Some(index) = self.layers.as_ref().and_then(|layers| layers.index_of(drag.id)) else { return };
        let Some(layers) = self.layers.as_mut() else { return };
        let layer = &mut layers.list[index];
        let was = if drag.before.0 { drag.before.1 as f64 / 100.0 } else { 0.0 };
        (layer.on, layer.percent) = (on, percent.min(100));
        let change = layer.strength() - was;
        let offsets = &layer.offsets;
        let moved: Vec<Vec3> = drag
            .verts
            .par_iter()
            .zip(&drag.positions)
            .with_min_len(PER_TASK)
            .map(|(&v, &start)| {
                // No change is exactly no change: the press position, not one worked out.
                if change == 0.0 { start } else { (start.as_dvec3() + offsets[v as usize].as_dvec3() * change).as_vec3() }
            })
            .collect();
        for (&v, position) in drag.verts.iter().zip(moved) {
            self.positions[v as usize] = position;
        }
        self.positions_changed(&drag.leaves);
    }

    /// Closes a change of strength and gives back its undo step, or `None`
    /// if the layer is as it was at the press.
    pub fn end_layer_drag(&mut self, drag: StrengthDrag) -> Option<LayersStep> {
        let layers = self.layers.as_ref()?;
        let layer = &layers.list[layers.index_of(drag.id)?];
        if (layer.on, layer.percent) == drag.before {
            return None;
        }
        let mut step = self.layers_step();
        let row = step.rows.iter_mut().find(|row| row.id == drag.id)?;
        (row.on, row.percent) = drag.before;
        (step.verts, step.positions) = (drag.verts, drag.positions);
        Some(step)
    }

    /// Sets layer `id`'s strength and whether it is on, in one go.
    pub fn set_layer(&mut self, id: u32, on: bool, percent: u8) -> Option<LayersStep> {
        let drag = self.drag_layer(id)?;
        self.drag_layer_to(&drag, on, percent);
        self.end_layer_drag(drag)
    }

    /// Takes layer `id` off the list. With `bake` what it adds stays in the
    /// shape, as part of the model underneath from now on; without, it goes.
    pub fn remove_layer(&mut self, id: u32, bake: bool) -> Option<LayersStep> {
        let index = self.layers.as_ref()?.index_of(id)?;
        let mut step = self.layers_step();
        if !bake {
            // Deleting is turning it all the way down first.
            let drag = self.drag_layer(id)?;
            self.drag_layer_to(&drag, false, 0);
            (step.verts, step.positions) = (drag.verts, drag.positions);
        }
        let layers = self.layers.as_mut()?;
        let layer = layers.list.remove(index);
        step.rows[index].offsets = Some(layer.offsets);
        if layers.recording == Some(id) {
            layers.recording = None;
        }
        if layers.list.is_empty() {
            step.frame = layers.frame.take();
            self.layers = None;
        }
        Some(step)
    }

    /// Bakes every layer: the shape stays as it is and the list is emptied.
    pub fn bake_all_layers(&mut self) -> Option<LayersStep> {
        let mut layers = self.layers.take()?;
        let mut step = LayersStep {
            rows: layers.step_rows(),
            recording: layers.recording,
            next_id: layers.next_id,
            verts: Vec::new(),
            positions: Vec::new(),
            frame: layers.frame.take(),
        };
        for (row, layer) in step.rows.iter_mut().zip(layers.list) {
            row.offsets = Some(layer.offsets);
        }
        Some(step)
    }

    /// Every layer's offset at each vertex, times its strength, summed in
    /// double precision: what the layers add to the model underneath.
    fn layers_sum(&self) -> Option<Vec<DVec3>> {
        let layers = self.layers.as_ref()?;
        let active: Vec<(&[Vec3], f64)> =
            layers.list.iter().map(|l| (&l.offsets[..], l.strength())).filter(|(_, strength)| *strength > 0.0).collect();
        let sum = |v: usize| active.iter().map(|(offsets, strength)| offsets[v].as_dvec3() * *strength).sum::<DVec3>();
        Some((0..self.positions.len()).into_par_iter().with_min_len(PER_TASK).map(sum).collect())
    }

    /// The model underneath: the positions less what the layers add. The
    /// positions themselves for a mesh without layers.
    pub fn model_underneath(&self) -> Vec<Vec3> {
        match self.layers_sum() {
            Some(sum) => self.positions.iter().zip(&sum).map(|(p, add)| (p.as_dvec3() - *add).as_vec3()).collect(),
            None => self.positions.clone(),
        }
    }

    /// `underneath` with the layers on it: the shape the mesh would have
    /// over that model. `underneath` itself for a mesh without layers, and
    /// for a vertex no layer moves.
    pub fn with_layers(&self, underneath: &[Vec3]) -> Vec<Vec3> {
        let Some(sum) = self.layers_sum().filter(|sum| sum.len() == underneath.len()) else {
            return underneath.to_vec();
        };
        let over = |(p, add): (&Vec3, &DVec3)| if *add == DVec3::ZERO { *p } else { (p.as_dvec3() + *add).as_vec3() };
        underneath.iter().zip(&sum).map(over).collect()
    }

    /// The layers for a copy of this mesh whose vertices were renumbered:
    /// vertex `v` of this mesh is vertex `place[v]` of the copy, or is not in
    /// it (`u32::MAX`).
    pub fn layers_renumbered(&self, place: &[u32]) -> Option<Box<Layers>> {
        let layers = self.layers.as_ref()?;
        let kept = place.iter().filter(|&&to| to != u32::MAX).count();
        Layers::restored(layers.saved(None), kept, Some(place))
    }

    /// Whether the mesh is on a subdivision level below its finest, where the
    /// mesh's layers are only the first entries of the layers.
    fn below_finest(&self) -> bool {
        self.levels.as_ref().is_some_and(|levels| levels.current() < levels.top())
    }

    /// Called as a stroke that moved vertices ends, with its undo record:
    /// gives the stroke to the layers (see the module notes) and hands back
    /// the record, with what that did to the layers added to it.
    ///
    /// `linear` is for a move that is known exactly: the gizmo turning or
    /// stretching the unmasked part of the surface (each half its own way
    /// under mirror symmetry, so it gives the matrix by where the vertex
    /// began). Each vertex moved as far toward its matrix as it is unmasked, and with no layer recording its
    /// offsets are carried the same way, so a limb turned or made larger
    /// takes its layers' detail round, or up in size, with it. Without it
    /// `Mesh::layer_ride` says what kind of move the stroke was.
    pub(super) fn settle_layers(&mut self, delta: StrokeDelta, linear: Option<Carry>) -> StrokeDelta {
        let StrokeDelta::Positions { verts, before, after, topology } = &delta else { return delta };
        let Some(taking) = self.layers.as_ref().map(|layers| layers.taking()) else { return delta };
        let from = topology.as_ref().map(|topology| topology.vertices_before());
        let changes = match taking {
            Some(index) => self.record_stroke(index, verts, before, after, from).into_iter().collect(),
            None => self.carry_offsets(verts, from, linear),
        };
        let lists_from = |from: u32| -> Vec<(u32, Vec<Vec3>)> {
            let lists = self.layers.iter().flat_map(|layers| layers.lists());
            lists.map(|(id, list)| (id, list[from as usize..].to_vec())).collect()
        };
        let grown = from.map(|from| Grown { from, offsets: lists_from(from) });
        if changes.is_empty() && grown.is_none() {
            return delta;
        }
        StrokeDelta::Layered { stroke: Box::new(delta), layers: Box::new(StrokeLayers { changes, grown }) }
    }

    /// Adds what the stroke moved to the layer at `index` of the list,
    /// divided by its strength. `from` is the first of the vertices dynamic
    /// topology added, if it added any: those are recorded too, from where
    /// each was when the stroke first moved it.
    fn record_stroke(&mut self, index: usize, verts: &[u32], before: &[Vec3], after: &[Vec3], from: Option<u32>) -> Option<Change> {
        let born: Vec<(u32, Vec3)> = match from {
            Some(from) => (from..self.positions.len() as u32)
                .filter(|&v| self.stamp[v as usize] == self.stroke_id)
                .map(|v| (v, self.positions[v as usize] - self.orig_pos[v as usize]))
                .collect(),
            None => Vec::new(),
        };
        let layer = &mut self.layers.as_mut()?.list[index];
        let strength = layer.strength();
        let added = |offset: Vec3, moved: Vec3| (offset.as_dvec3() + moved.as_dvec3() / strength).as_vec3();
        let mut change = Change::of(layer.id);
        for ((&v, &was), &now) in verts.iter().zip(before).zip(after) {
            // A vertex that did not move, or went somewhere that is not a
            // place (damage, which the mesh's own repair deals with), leaves
            // the layer as it was.
            let moved = now - was;
            if moved == Vec3::ZERO || !moved.is_finite() {
                continue;
            }
            let offset = layer.offsets[v as usize];
            layer.offsets[v as usize] = added(offset, moved);
            change.note(v, offset, layer.offsets[v as usize]);
        }
        for (v, moved) in born.into_iter().filter(|(_, moved)| moved.is_finite()) {
            layer.offsets[v as usize] = added(layer.offsets[v as usize], moved);
        }
        (!change.verts.is_empty()).then_some(change)
    }

    /// After a stroke on the model underneath: every layer's offsets at the
    /// vertices it moved are carried as that kind of stroke carries them
    /// (see `Ride`; by `linear` if it is given). Gives back what changed, list
    /// by list.
    fn carry_offsets(&mut self, verts: &[u32], from: Option<u32>, linear: Option<Carry>) -> Vec<Change> {
        if self.layers.is_none() || (linear.is_none() && self.layer_ride == Ride::Still) {
            return Vec::new();
        }
        // Every vertex the stroke moved, the ones it made included, whether
        // or not a layer has anything there yet: a layer sculpted there later
        // still has to be carried back by Reset Shape.
        let moved_now = |v: &u32| self.stamp[*v as usize] == self.stroke_id;
        let born = from.map_or(0..0, |from| from..self.positions.len() as u32).filter(moved_now);
        let carried: Vec<u32> = verts.iter().copied().chain(born).collect();
        // The matrix each vertex's offsets are carried by.
        let carries: Vec<DMat3> = match linear {
            Some(matrix) => {
                // Known: as far toward the gizmo's matrix as the vertex went, which is its unmasked share.
                let share = |v: &u32| (1.0 - self.mask[*v as usize]).clamp(0.0, 1.0) as f64;
                let blended = |v: &u32| {
                    let matrix = matrix(self.orig_pos[*v as usize]).as_dmat3();
                    DMat3::IDENTITY + (matrix - DMat3::IDENTITY) * share(v)
                };
                carried.par_iter().with_min_len(PER_TASK).map(blended).collect()
            }
            None => {
                // Read off the faces round it, as they were when the stroke began and as they are.
                let turned = |&v: &u32| Turn::of(&StrokeFan { mesh: self, vertex: v }).matrix();
                carried.par_iter().with_min_len(PER_TASK / 8).map(turned).collect()
            }
        };
        self.carry_each(&carried, &carries, from.unwrap_or(u32::MAX))
    }

    /// Carries every layer's offset at each vertex of `carried` by the
    /// matching entry of `carries`, notes it in the frame, and gives back
    /// what changed, list by list. Vertices from `first_new` on were made by
    /// the stroke.
    fn carry_each(&mut self, carried: &[u32], carries: &[DMat3], first_new: u32) -> Vec<Change> {
        let vertices = self.positions.len();
        let Some(layers) = self.layers.as_mut() else { return Vec::new() };
        if carries.iter().all(|carry| *carry == DMat3::IDENTITY) {
            return Vec::new();
        }
        let mut changes = Vec::new();
        for layer in &mut layers.list {
            let mut change = Change::of(layer.id);
            for (&v, carry) in carried.iter().zip(carries) {
                let offset = layer.offsets[v as usize];
                let turned = if offset == Vec3::ZERO { offset } else { (*carry * offset.as_dvec3()).as_vec3() };
                if same(turned, offset) {
                    continue;
                }
                layer.offsets[v as usize] = turned;
                // A vertex the stroke made has no "before" to go back to: undo takes it away.
                if v < first_new {
                    change.note(v, offset, turned);
                }
            }
            changes.push(change);
        }
        let whole = |axis: Vec3| vec![axis; vertices];
        let frame = layers.frame.get_or_insert_with(|| Box::new([whole(Vec3::X), whole(Vec3::Y), whole(Vec3::Z)]));
        let mut columns = [Change::of(FRAME), Change::of(FRAME + 1), Change::of(FRAME + 2)];
        for (&v, carry) in carried.iter().zip(carries) {
            let v_at = v as usize;
            let was = Mat3::from_cols(frame[0][v_at], frame[1][v_at], frame[2][v_at]);
            let now = (*carry * was.as_dmat3()).as_mat3();
            for (k, column) in columns.iter_mut().enumerate() {
                if !same(was.col(k), now.col(k)) {
                    frame[k][v_at] = now.col(k);
                    if v < first_new {
                        column.note(v, was.col(k), now.col(k));
                    }
                }
            }
        }
        changes.extend(columns);
        changes.retain(|change| !change.verts.is_empty());
        changes
    }

    /// Carries every layer's offset at each of `verts` back by what it has
    /// been carried by since the mesh was built, and clears the frame there:
    /// the layers as they would stand on the model as it was built.
    fn carry_home(&mut self, verts: &[u32]) -> Vec<Change> {
        let Some(layers) = self.layers.as_mut().filter(|layers| layers.frame.is_some()) else { return Vec::new() };
        let back = |v: u32| {
            let frame = layers.frame_at(v as usize);
            let back = frame.as_dmat3().inverse();
            (frame != Mat3::IDENTITY && back.is_finite()).then_some(back)
        };
        let backs: Vec<Option<DMat3>> = verts.iter().map(|&v| back(v)).collect();
        let mut changes = Vec::new();
        for layer in &mut layers.list {
            let mut change = Change::of(layer.id);
            for (&v, back) in verts.iter().zip(&backs) {
                let (Some(back), offset) = (back, layer.offsets[v as usize]) else { continue };
                let home = (*back * offset.as_dvec3()).as_vec3();
                if !same(home, offset) {
                    layer.offsets[v as usize] = home;
                    change.note(v, offset, home);
                }
            }
            changes.push(change);
        }
        let frame = layers.frame.as_mut().expect("checked above");
        for (k, axis) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
            let mut change = Change::of(FRAME + k as u32);
            for (&v, back) in verts.iter().zip(&backs) {
                if back.is_some() && !same(frame[k][v as usize], axis) {
                    change.note(v, frame[k][v as usize], axis);
                    frame[k][v as usize] = axis;
                }
            }
            changes.push(change);
        }
        changes.retain(|change| !change.verts.is_empty());
        changes
    }

    /// Puts the model underneath the layers at `rest`, one point for every
    /// vertex, and leaves the layers on it: Reset Shape for a mesh with
    /// layers. Whatever the layers' offsets were carried by as the model
    /// underneath was posed or turned since it was built, they are carried
    /// back by (`Layers::frame`), so detail that went round a bend with a
    /// limb comes back round with it. Gives back the undo record, or `None`
    /// if the model underneath is at rest already.
    ///
    /// The model underneath is worked out, not stored (positions less
    /// layers), which rounds: a vertex within a few units in the last place
    /// of its rest is at rest.
    pub fn rest_underneath(&mut self, rest: &[Vec3]) -> Option<StrokeDelta> {
        let under = self.model_underneath();
        if rest.len() != under.len() {
            return None;
        }
        let at_rest = |v: usize| {
            let slack = 4.0 * f32::EPSILON * self.positions[v].abs().max(under[v].abs()).max_element();
            (under[v] - rest[v]).abs().max_element() <= slack
        };
        let verts: Vec<u32> = (0..under.len()).filter(|&v| !at_rest(v)).map(|v| v as u32).collect();
        if verts.is_empty() {
            return None;
        }
        let mut target = under;
        verts.iter().for_each(|&v| target[v as usize] = rest[v as usize]);
        let changes = self.carry_home(&verts);
        let shape = self.with_layers(&target);
        let before: Vec<Vec3> = verts.iter().map(|&v| self.positions[v as usize]).collect();
        let after: Vec<Vec3> = verts.iter().map(|&v| shape[v as usize]).collect();
        let stroke = StrokeDelta::Positions { verts, before, after, topology: None };
        self.apply_delta(&stroke, false);
        if changes.is_empty() {
            return Some(stroke);
        }
        Some(StrokeDelta::Layered { stroke: Box::new(stroke), layers: Box::new(StrokeLayers { changes, grown: None }) })
    }

    /// Undo or redo of what a stroke did to the layers, once its positions
    /// (and the vertices dynamic topology added) have been put back.
    pub(super) fn apply_stroke_layers(&mut self, record: &StrokeLayers, undo: bool) {
        if let Some(layers) = self.layers.as_mut() {
            record.apply(layers, undo);
        }
    }
}

#[cfg(test)]
mod tests;
