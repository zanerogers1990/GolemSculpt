//! Changes to the paint layer stack: each one undo step (`PaintStep`),
//! carried out on the data alone, so the Paint Layers card, the bridge and
//! the tests all reach the same code.
//!
//! - **Add** puts an empty layer above the active one and makes it active.
//! - **Delete**, **Rename**, **Move** (reorder), **Duplicate** do what they say.
//! - **Merge Down** folds a layer into the one under it: the lower layer
//!   ends up holding what the two showed together, at full opacity and with
//!   no mask, keeping its own name and blend modes. Where the lower layer is
//!   Normal (or the bottom one) the model looks the same after as before, to
//!   the rounding of a byte. A hidden upper layer adds nothing. One texel
//!   holds one material, so where the two layers held different materials
//!   at one vertex the stronger is kept.
//! - **Flatten All** folds every visible layer into one and drops the hidden
//!   ones, as painting programs do.
//! - **Fill** paints the whole layer, or what the sculpt mask (or its
//!   inverse) or a face group covers, with a colour or a material.
//!   Hidden faces are left alone.
//! - **Clear** empties channels of a layer.
//! - The row's settings (eye, lock, opacity, blends, mask source, channel
//!   toggles), which layer is active, and the local material table.

use std::fmt;

use rayon::prelude::*;

use super::composite::{Ctx, Stream};
use super::{
    ChannelSet, Channels, MOST_MATERIALS, MOST_PAINT_LAYERS, MaskSource, Mat8, PER_TASK, Paint, PaintLayer, PaintRow,
    PaintStack, PaintStep, PaintTarget, RoughMetal8, Rgba8, Texel, Unorm8,
};
use crate::mesh::Mesh;

/// Why a paint command or stroke was not carried out, in words for the
/// status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaintRefusal {
    /// There is no paint layer to work on.
    NoLayer,
    /// The layer is locked.
    Locked(String),
    /// Paint is written inside a stroke only.
    NotInStroke,
    TooManyLayers,
    /// The object's paint uses eight materials already.
    TableFull,
    /// There is no layer under this one to merge into.
    NothingBelow,
    /// The command would change nothing.
    Unchanged,
}

impl fmt::Display for PaintRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaintRefusal::NoLayer => write!(f, "There is no paint layer to paint on: make one on the Paint Layers card."),
            PaintRefusal::Locked(name) => {
                write!(f, "The paint layer \"{name}\" is locked. Unlock it on the Paint Layers card to paint on it.")
            }
            PaintRefusal::NotInStroke => write!(f, "Paint is put on with a stroke."),
            PaintRefusal::TooManyLayers => {
                write!(f, "This object has {MOST_PAINT_LAYERS} paint layers, the most it can have. Merge some first.")
            }
            PaintRefusal::TableFull => write!(
                f,
                "This object's paint uses {MOST_MATERIALS} materials already, the most one object can have. Tidy the \
                 list, or paint with one it uses."
            ),
            PaintRefusal::NothingBelow => write!(f, "There is no paint layer under this one to merge it into."),
            PaintRefusal::Unchanged => write!(f, "That changes nothing."),
        }
    }
}

/// Where a fill goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillArea {
    /// Everywhere that shows.
    All,
    /// As far as the sculpt mask covers each vertex, or with `invert`, as far
    /// as it does not.
    SculptMask { invert: bool },
    /// The faces of a face group.
    Group(u16),
}

/// A vertex's composite record as a texel: what Merge Down and Flatten keep.
fn texel_of(record: &Stream, table: &[u8]) -> Texel {
    let (mut best, mut weight) = (0u8, 0u8);
    for (k, &slot) in table.iter().enumerate().take(MOST_MATERIALS) {
        if record[8 + k] > weight {
            (best, weight) = (slot, record[8 + k]);
        }
    }
    Texel {
        colour: Rgba8([record[0], record[1], record[2], record[3]]),
        material: Mat8(if weight == 0 { [0, 0] } else { [best, weight] }),
        rough_metal: RoughMetal8([record[4], record[6], record[5], record[7]]),
        mask: Unorm8(255),
    }
}

/// A layer holding exactly the composite of `layers` over nothing, with the
/// table `table`, for a mesh: what Merge Down and Flatten All make.
fn folded(layers: Vec<PaintLayer>, table: &[u8], mesh: &Mesh) -> Channels {
    let stack = PaintStack { layers, table: table.to_vec(), ..PaintStack::new(mesh.vertex_count()) };
    let ctx = Ctx::new(&stack, mesh, &[]);
    let records: Vec<Stream> =
        (0..mesh.vertex_count()).into_par_iter().with_min_len(PER_TASK).map(|v| ctx.vertex(v)).collect();
    let present = stack.layers.iter().fold(Channels::default().present(), |set, l| {
        if l.row.visible { set.with(l.data.present().without(ChannelSet::MASK)) } else { set }
    });
    let mut data = Channels::default();
    data.open(present, records.len());
    let texels: Vec<Texel> = records.par_iter().with_min_len(PER_TASK).map(|r| texel_of(r, table)).collect();
    if let Some(l) = &mut data.colour {
        l.par_iter_mut().zip(&texels).for_each(|(c, t)| *c = t.colour);
    }
    if let Some(l) = &mut data.material {
        l.par_iter_mut().zip(&texels).for_each(|(c, t)| *c = t.material);
    }
    if let Some(l) = &mut data.rough_metal {
        l.par_iter_mut().zip(&texels).for_each(|(c, t)| *c = t.rough_metal);
    }
    data
}

impl PaintStack {
    /// Adds palette slot `slot` to the local material table if it is not
    /// there, and gives back its entry. The table only grows (so the weights
    /// already worked out keep their meaning) until it is tidied.
    pub fn table_add(&mut self, slot: u8) -> Result<usize, PaintRefusal> {
        if let Some(k) = self.table_index(slot) {
            return Ok(k);
        }
        if slot == 0 {
            return Err(PaintRefusal::Unchanged);
        }
        if self.table.len() >= MOST_MATERIALS {
            return Err(PaintRefusal::TableFull);
        }
        self.table.push(slot);
        Ok(self.table.len() - 1)
    }

    /// The palette slots some layer's material list uses, in table order
    /// first and then in the order they are found.
    pub fn slots_used(&self) -> Vec<u8> {
        let mut used = [false; 256];
        for layer in &self.layers {
            if let Some(list) = &layer.data.material {
                for m in list {
                    if m.0[1] > 0 {
                        used[m.0[0] as usize] = true;
                    }
                }
            }
        }
        used[0] = false;
        let mut out: Vec<u8> = self.table.iter().copied().filter(|&s| used[s as usize]).collect();
        out.extend((1..=255u8).filter(|&s| used[s as usize] && !self.table.contains(&s)));
        out
    }
}

impl Mesh {
    /// Carries out a change to the stack as one undo step: `changing` are
    /// the layers whose lists `edit` may change (or delete), and with
    /// `create` a mesh without paint gets an empty stack to change. `edit`
    /// checks before it changes anything: a refusal leaves the paint as it was.
    fn paint_change<R>(
        &mut self,
        changing: &[u32],
        create: bool,
        edit: impl FnOnce(&mut PaintStack, &Mesh) -> Result<R, PaintRefusal>,
    ) -> Result<(R, PaintStep), PaintRefusal> {
        debug_assert!(!self.in_stroke);
        self.paint_fit();
        let had = self.paint.is_some();
        if !had && !create {
            return Err(PaintRefusal::NoLayer);
        }
        let n = self.vertex_count();
        let mut paint = self.paint.take().unwrap_or_else(|| Paint::of(PaintStack::new(n)));
        let step = PaintStep::before(had.then_some(&paint.stack), changing);
        match edit(&mut paint.stack, self) {
            Ok(r) => {
                paint.live.all = true;
                // A mesh with no paint layers has no paint, and costs nothing.
                self.paint = (!paint.stack.layers.is_empty()).then_some(paint);
                Ok((r, step))
            }
            Err(why) => {
                self.paint = had.then_some(paint);
                Err(why)
            }
        }
    }

    /// The layer `id` if it can take a change of its lists.
    fn unlocked(stack: &PaintStack, id: u32) -> Result<usize, PaintRefusal> {
        let at = stack.index_of(id).ok_or(PaintRefusal::NoLayer)?;
        let row = &stack.layers[at].row;
        if row.locked { Err(PaintRefusal::Locked(row.name.clone())) } else { Ok(at) }
    }

    /// Adds an empty layer above the active one (at the top if none is) and
    /// makes it active. Without a name it is "Paint 1", or the first such
    /// name not in use.
    pub fn add_paint_layer(&mut self, name: Option<&str>) -> Result<(u32, PaintStep), PaintRefusal> {
        self.paint_change(&[], true, |stack, _| {
            if stack.layers.len() >= MOST_PAINT_LAYERS {
                return Err(PaintRefusal::TooManyLayers);
            }
            let name = name.map(str::trim).filter(|n| !n.is_empty());
            let name = match name {
                Some(name) => name.to_owned(),
                None => (1..).map(|k| format!("Paint {k}")).find(|n| !stack.layers.iter().any(|l| l.row.name == *n)).unwrap(),
            };
            let id = stack.next_id;
            stack.next_id += 1;
            let at = stack.active.and_then(|a| stack.index_of(a)).map_or(stack.layers.len(), |a| a + 1);
            stack.layers.insert(at, PaintLayer { row: PaintRow::new(id, name), data: Channels::default() });
            stack.active = Some(id);
            Ok(id)
        })
    }

    /// Deletes layer `id`. The layer under it becomes active (the one over it
    /// if it was the bottom one).
    pub fn delete_paint_layer(&mut self, id: u32) -> Result<PaintStep, PaintRefusal> {
        let (_, step) = self.paint_change(&[id], false, |stack, _| {
            let at = stack.index_of(id).ok_or(PaintRefusal::NoLayer)?;
            stack.layers.remove(at);
            if stack.active == Some(id) {
                stack.active = stack.layers.get(at.saturating_sub(1)).map(|l| l.row.id);
            }
            Ok(())
        })?;
        Ok(step)
    }

    /// Gives layer `id` another name.
    pub fn rename_paint_layer(&mut self, id: u32, name: &str) -> Result<PaintStep, PaintRefusal> {
        let name = name.trim();
        let (_, step) = self.paint_change(&[], false, |stack, _| {
            let layer = stack.layer_mut(id).ok_or(PaintRefusal::NoLayer)?;
            if name.is_empty() || layer.row.name == name {
                return Err(PaintRefusal::Unchanged);
            }
            layer.row.name = name.to_owned();
            Ok(())
        })?;
        Ok(step)
    }

    /// Moves layer `id` to place `to` in the stack, counted from the bottom.
    pub fn move_paint_layer(&mut self, id: u32, to: usize) -> Result<PaintStep, PaintRefusal> {
        let (_, step) = self.paint_change(&[], false, |stack, _| {
            let at = stack.index_of(id).ok_or(PaintRefusal::NoLayer)?;
            let to = to.min(stack.layers.len() - 1);
            if at == to {
                return Err(PaintRefusal::Unchanged);
            }
            let layer = stack.layers.remove(at);
            stack.layers.insert(to, layer);
            Ok(())
        })?;
        Ok(step)
    }

    /// A copy of layer `id` just over it, active, named "<name> copy".
    pub fn duplicate_paint_layer(&mut self, id: u32) -> Result<(u32, PaintStep), PaintRefusal> {
        self.paint_change(&[], false, |stack, _| {
            if stack.layers.len() >= MOST_PAINT_LAYERS {
                return Err(PaintRefusal::TooManyLayers);
            }
            let at = stack.index_of(id).ok_or(PaintRefusal::NoLayer)?;
            let mut copy = stack.layers[at].clone();
            copy.row.id = stack.next_id;
            copy.row.name = stack.free_name(&format!("{} copy", copy.row.name));
            stack.next_id += 1;
            let new = copy.row.id;
            stack.layers.insert(at + 1, copy);
            stack.active = Some(new);
            Ok(new)
        })
    }

    /// Merge Down: layer `id` folded into the one under it (see the module notes).
    pub fn merge_paint_down(&mut self, id: u32) -> Result<PaintStep, PaintRefusal> {
        let (below, at) = {
            let stack = self.paint.as_ref().map(|p| &p.stack).ok_or(PaintRefusal::NoLayer)?;
            let at = stack.index_of(id).ok_or(PaintRefusal::NoLayer)?;
            let below = stack.layers.get(at.wrapping_sub(1)).ok_or(PaintRefusal::NothingBelow)?.row.id;
            (below, at)
        };
        let (_, step) = self.paint_change(&[id, below], false, |stack, mesh| {
            Self::unlocked(stack, id)?;
            Self::unlocked(stack, below)?;
            let (mut lower, upper) = (stack.layers[at - 1].clone(), stack.layers[at].clone());
            let kept = lower.row.clone();
            // Over nothing a layer's blend makes no difference; its eye is its own to keep.
            lower.row.visible = true;
            let data = folded(vec![lower, upper], &stack.table, mesh);
            let merged = PaintLayer {
                row: PaintRow { opacity: 255, mask: MaskSource::None, channels: ChannelSet::SURFACE, ..kept },
                data,
            };
            stack.layers[at - 1] = merged;
            stack.layers.remove(at);
            if stack.active == Some(id) {
                stack.active = Some(below);
            }
            Ok(())
        })?;
        Ok(step)
    }

    /// Flatten All: every visible layer folded into one, the hidden ones
    /// dropped (see the module notes).
    pub fn flatten_paint(&mut self) -> Result<PaintStep, PaintRefusal> {
        let ids: Vec<u32> = self.paint_rows().iter().map(|r| r.id).collect();
        if ids.len() < 2 {
            return Err(PaintRefusal::Unchanged);
        }
        let (_, step) = self.paint_change(&ids, false, |stack, mesh| {
            let data = folded(stack.layers.clone(), &stack.table, mesh);
            let first = stack.layers.iter().find(|l| l.row.visible).unwrap_or(&stack.layers[0]);
            let row = PaintRow::new(first.row.id, first.row.name.clone());
            stack.active = Some(row.id);
            stack.layers = vec![PaintLayer { row, data }];
            Ok(())
        })?;
        Ok(step)
    }

    /// Fill: the active layer painted with `target` over `area` (see the
    /// module notes).
    pub fn fill_paint(&mut self, target: &PaintTarget, area: FillArea) -> Result<PaintStep, PaintRefusal> {
        let id = self.paint_writable()?;
        let (_, step) = self.paint_change(&[id], false, |stack, mesh| {
            let at = Self::unlocked(stack, id)?;
            let n = mesh.vertex_count();
            let hidden = mesh.hidden_vertices();
            let group = match area {
                FillArea::Group(g) => Some(mesh.group_vertices(g)),
                _ => None,
            };
            let share = |v: usize| -> f32 {
                if hidden.is_some_and(|h| h.get(v).copied().unwrap_or(false)) {
                    return 0.0;
                }
                match area {
                    FillArea::All => 1.0,
                    FillArea::SculptMask { invert } => {
                        let m = mesh.mask.get(v).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                        if invert { 1.0 - m } else { m }
                    }
                    FillArea::Group(_) => group.as_ref().map_or(0.0, |g| if g[v] { 1.0 } else { 0.0 }),
                }
            };
            let data = &mut stack.layers[at].data;
            data.open(target.channels, n);
            let view: &Channels = data;
            let which = target.channels;
            let after: Vec<Option<Texel>> = (0..n)
                .into_par_iter()
                .with_min_len(PER_TASK)
                .map(|v| {
                    let (s, was) = (share(v), view.texel(v));
                    let now = target.applied(was, s);
                    (s > 0.0 && was.with(now, which) != was).then_some(now)
                })
                .collect();
            if after.iter().all(Option::is_none) {
                return Err(PaintRefusal::Unchanged);
            }
            for (v, t) in after.into_iter().enumerate() {
                if let Some(t) = t {
                    data.put(v, t, target.channels);
                }
            }
            Ok(())
        })?;
        Ok(step)
    }

    /// Clear: the channels `which` of layer `id` emptied.
    pub fn clear_paint(&mut self, id: u32, which: ChannelSet) -> Result<PaintStep, PaintRefusal> {
        let (_, step) = self.paint_change(&[id], false, |stack, _| {
            let at = Self::unlocked(stack, id)?;
            let data = &mut stack.layers[at].data;
            if !data.present().any(which) {
                return Err(PaintRefusal::Unchanged);
            }
            if which.any(ChannelSet::COLOUR) {
                data.colour = None;
            }
            if which.any(ChannelSet::MATERIAL) {
                data.material = None;
            }
            if which.any(ChannelSet::MASK) {
                data.mask = None;
            }
            let (rough, metal) = (which.any(ChannelSet::ROUGHNESS), which.any(ChannelSet::METALLIC));
            if rough && metal {
                data.rough_metal = None;
            } else if let Some(list) = &mut data.rough_metal {
                let k = if rough { 1 } else { 3 };
                if rough || metal {
                    list.par_iter_mut().for_each(|rm| rm.0[k] = 0);
                }
            }
            Ok(())
        })?;
        Ok(step)
    }

    /// Changes layer `id`'s settings to `row` (its number must stay).
    pub fn set_paint_row(&mut self, row: PaintRow) -> Result<PaintStep, PaintRefusal> {
        let (_, step) = self.paint_change(&[], false, |stack, _| {
            let layer = stack.layer_mut(row.id).ok_or(PaintRefusal::NoLayer)?;
            if layer.row == row {
                return Err(PaintRefusal::Unchanged);
            }
            layer.row = row;
            Ok(())
        })?;
        Ok(step)
    }

    /// Makes layer `id` the one strokes and fills go to.
    pub fn set_paint_active(&mut self, id: u32) -> Result<PaintStep, PaintRefusal> {
        let (_, step) = self.paint_change(&[], false, |stack, _| {
            stack.index_of(id).ok_or(PaintRefusal::NoLayer)?;
            if stack.active == Some(id) {
                return Err(PaintRefusal::Unchanged);
            }
            stack.active = Some(id);
            Ok(())
        })?;
        Ok(step)
    }

    /// Takes a cavity snapshot for layer `id` (its Cavity mask source): the
    /// recesses of the surface as it is now, open where they are deepest.
    pub fn snapshot_paint_cavity(&mut self, id: u32) -> Result<PaintStep, PaintRefusal> {
        let cavity = self.cavity_now();
        let (_, step) = self.paint_change(&[id], false, |stack, _| {
            let layer = stack.layer_mut(id).ok_or(PaintRefusal::NoLayer)?;
            layer.data.cavity = Some(cavity);
            Ok(())
        })?;
        Ok(step)
    }

    /// How deep in a recess each vertex is, from 0 (flat or a ridge) to 255.
    fn cavity_now(&self) -> Vec<Unorm8> {
        let n = self.vertex_count();
        let size = self.bounds();
        let half = if size.is_empty() { glam::Vec3::ZERO } else { (size.max - size.min) * 0.5 };
        let scale = if half.max_element() < 1e-6 { 1.0 } else { half.max_element() };
        let raw: Vec<f32> = (0..n)
            .into_par_iter()
            .with_min_len(PER_TASK)
            .map(|v| Mesh::raw_curvature(&self.positions, &self.normals, self.neighbours(v as u32), v, scale))
            .collect();
        let mean = if n > 0 { (raw.iter().map(|&r| r as f64).sum::<f64>() / n as f64) as f32 } else { 0.0 };
        (0..n)
            .into_par_iter()
            .with_min_len(PER_TASK)
            .map(|v| {
                let c = Mesh::encode_curvature(&raw, self.neighbours(v as u32), v, mean);
                Unorm8(((c - 0.5) * 2.0 * 255.0).round().clamp(0.0, 255.0) as u8)
            })
            .collect()
    }

    /// Adds palette slot `slot` to the local material table, as one step.
    pub fn add_paint_material(&mut self, slot: u8) -> Result<(usize, PaintStep), PaintRefusal> {
        self.paint_change(&[], false, |stack, _| {
            if stack.table_index(slot).is_some() {
                return Err(PaintRefusal::Unchanged);
            }
            stack.table_add(slot)
        })
    }

    /// Tidy: the local material table cut down to the materials the layers
    /// use, so that a full table has room again.
    pub fn tidy_paint_materials(&mut self) -> Result<PaintStep, PaintRefusal> {
        let (_, step) = self.paint_change(&[], false, |stack, _| {
            let used = stack.slots_used();
            let tidied: Vec<u8> = used.into_iter().take(MOST_MATERIALS).collect();
            if tidied == stack.table {
                return Err(PaintRefusal::Unchanged);
            }
            stack.table = tidied;
            Ok(())
        })?;
        Ok(step)
    }
}
