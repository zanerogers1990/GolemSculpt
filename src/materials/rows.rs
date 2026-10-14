//! The scene's palette as the painted shader reads it: one [`Row`] per
//! entry in a storage buffer, and the custom shaders the painted pipeline
//! has to be built with ([`Upload`]). Worked out on the CPU, where it can be
//! tested, and handed to the renderer (render/paint.rs) whenever it changes.
//!
//! Row 0 is no material (it is never weighted); an entry's row is found by
//! its slot through `slot_row`.

use std::collections::BTreeSet;

use bytemuck::{Pod, Zeroable};

use super::custom::Shaders;
use super::palette::material_of;
use super::{Kind, Settings};
use crate::look::{FinishParams, linear};
use crate::mesh::paint::palette::ScenePaint;

/// One palette entry for the shader; mirrors `PaletteRow` in painted.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq, Debug)]
pub struct Row {
    /// x the finish (`Kind` as a number), y tint mode (0 off, 1 multiply,
    /// 2 replace), z tint amount, w which custom shader (for `Kind::Custom`)
    pub head: [f32; 4],
    /// Linear colour of the plain surface (a plain material, and under a glow).
    pub base: [f32; 4],
    /// x metallic, y smoothness of that plain surface.
    pub extra: [f32; 4],
    /// The swatch in linear colour: the matcap's tint, and what a material
    /// that cannot be drawn shows.
    pub swatch: [f32; 4],
    pub params: FinishParams,
}

/// A custom shader the painted pipeline is built with.
#[derive(Clone, PartialEq, Debug)]
pub struct Code {
    pub id: String,
    pub function: String,
    pub source: String,
    /// Changes with the source and sidecar: part of the pipeline's key.
    pub hash: u64,
    pub curvature: bool,
    pub back_depth: bool,
}

/// What the renderer needs of the palette.
#[derive(Clone, PartialEq, Debug)]
pub struct Upload {
    pub rows: Vec<Row>,
    /// For each slot (0 to 255), its row; 0 for a slot with no entry.
    pub slot_row: Vec<u32>,
    /// The finishes some row uses (no material, row 0, is not counted).
    pub kinds: BTreeSet<Kind>,
    /// The custom shaders some row uses, in the order rows name them.
    pub customs: Vec<Code>,
    /// For each row, whether it reads curvature and back-face depth.
    pub needs: Vec<(bool, bool)>,
    /// What to say about custom materials that cannot be drawn.
    pub failures: Vec<String>,
}

impl Default for Upload {
    fn default() -> Upload {
        Upload {
            rows: vec![Row::zeroed()],
            slot_row: vec![0; 256],
            kinds: BTreeSet::new(),
            customs: Vec::new(),
            needs: vec![(false, false)],
            failures: Vec::new(),
        }
    }
}

impl Upload {
    /// The row of `slot`.
    pub fn row_of(&self, slot: u8) -> u32 {
        self.slot_row.get(slot as usize).copied().unwrap_or(0)
    }

    /// Whether any of `slots` is a material that reads the back faces' depth.
    pub fn back_depth_for(&self, slots: impl IntoIterator<Item = u8>) -> bool {
        slots.into_iter().any(|s| self.needs.get(self.row_of(s) as usize).is_some_and(|n| n.1))
    }

    /// Whether any entry at all reads the mesh's curvature.
    pub fn any_curvature(&self) -> bool {
        self.needs.iter().any(|n| n.0)
    }
}

/// The palette of `paint` packed for the shader, each entry's patterns at
/// its frozen reference (or `model_size` where it has none).
pub fn upload(paint: &ScenePaint, model_size: f32, shaders: &Shaders) -> Upload {
    let mut out = Upload::default();
    for entry in &paint.palette {
        let material = material_of(entry);
        let size = if entry.reference > 0.0 { entry.reference } else { model_size };
        let packed = material.pack(size, shaders);
        let mut custom_index = 0u32;
        let (mut curvature, mut back_depth) = (packed.kind.reads_curvature(), packed.kind.reads_back_depth());
        if let Settings::Custom(c) = &material.settings {
            match shaders.usable(&c.id) {
                Some(shader) if packed.kind == Kind::Custom => {
                    curvature = shader.sidecar.needs("curvature");
                    back_depth = shader.sidecar.needs("back_depth");
                    let code = Code {
                        id: shader.id.clone(),
                        function: shader.function(),
                        source: shader.source.clone(),
                        hash: shader.hash(),
                        curvature,
                        back_depth,
                    };
                    custom_index = match out.customs.iter().position(|k| k.id == code.id) {
                        Some(i) => i as u32,
                        None => {
                            out.customs.push(code);
                            out.customs.len() as u32 - 1
                        }
                    };
                }
                _ => {
                    let said = match shaders.get(&c.id).and_then(|s| s.failure()) {
                        Some(failure) => failure,
                        None => format!(
                            "Custom material \"{}\" needs a shader called {} that is not here. It shows as plain colour.",
                            material.name, c.id
                        ),
                    };
                    if !out.failures.contains(&said) {
                        out.failures.push(said);
                    }
                }
            }
        }
        let swatch = linear(material.swatch);
        let tint = material.tint;
        let row = Row {
            head: [packed.kind as u32 as f32, tint.mode.code(), tint.amount.clamp(0.0, 1.0), custom_index as f32],
            base: [packed.base[0], packed.base[1], packed.base[2], 1.0],
            extra: [packed.base_metal_smooth[0], packed.base_metal_smooth[1], 0.0, 0.0],
            swatch: [swatch[0], swatch[1], swatch[2], 1.0],
            params: packed.params,
        };
        let at = out.rows.len() as u32;
        out.rows.push(row);
        out.needs.push((curvature, back_depth));
        out.kinds.insert(packed.kind);
        if let Some(slot) = out.slot_row.get_mut(entry.slot as usize) {
            if entry.slot != 0 {
                *slot = at;
            }
        }
    }
    out
}
