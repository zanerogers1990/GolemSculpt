//! Paint put on a mesh for checks and benchmarks: materials in soft-edged
//! wedges round the model's vertical axis and a band of colour round its
//! middle, laid through the paint data's own layer commands (a layer per
//! material, each filled under a sculpt mask shaped like its wedge), as a
//! sculptor's strokes would leave it. Where the wedges' soft edges overlap,
//! two materials blend; at the poles every wedge meets, which is where three
//! or more materials share a triangle.

use std::f32::consts::TAU;

use crate::mesh::Mesh;
use crate::mesh::paint::{ChannelSet, FillArea, Mat8, PaintMode, PaintRefusal, PaintTarget, Rgba8, Texel};

/// How far each wedge's soft edge reaches into its neighbours, as a
/// fraction of a wedge.
const SOFT: f32 = 0.18;

/// Paints `mesh` with the palette `slots`, one wedge each round the Y axis,
/// and with `colour` (sRGB) a band round the middle. With `cap`, the top of
/// the model is left bare, so the Look's own finish shows there and blends
/// into the paint. The sculpt mask is used to shape the fills and is left
/// cleared.
pub fn paint_bands(mesh: &mut Mesh, slots: &[u8], colour: Option<[u8; 3]>, cap: bool) -> Result<(), PaintRefusal> {
    let bounds = mesh.bounds();
    let (centre, half) = ((bounds.min + bounds.max) * 0.5, ((bounds.max - bounds.min) * 0.5).max(glam::Vec3::splat(1e-6)));
    let n = slots.len().max(1) as f32;
    let wedge = TAU / n;
    for (k, &slot) in slots.iter().enumerate() {
        let middle = k as f32 * wedge;
        let _ = mesh.remap_mask(|m, v, _| {
            let p = (m.positions[v as usize] - centre) / half;
            let angle = p.z.atan2(p.x).rem_euclid(TAU);
            let off = ((angle - middle + TAU * 1.5).rem_euclid(TAU) - TAU * 0.5).abs() / wedge;
            let inside = ((0.5 + SOFT - off) / (2.0 * SOFT)).clamp(0.0, 1.0);
            let top = if cap { ((0.75 - p.y) / 0.25).clamp(0.0, 1.0) } else { 1.0 };
            inside * top
        });
        mesh.add_paint_layer(Some(&format!("Material {}", k + 1)))?;
        match mesh.add_paint_material(slot) {
            Ok(_) | Err(PaintRefusal::Unchanged) => {}
            Err(e) => return Err(e),
        }
        let texel = Texel { material: Mat8([slot, 255]), ..Texel::BARE };
        let target = PaintTarget { texel, channels: ChannelSet::MATERIAL, mode: PaintMode::Paint };
        mesh.fill_paint(&target, FillArea::SculptMask { invert: false })?;
    }
    if let Some([r, g, b]) = colour {
        let _ = mesh.remap_mask(|m, v, _| {
            let y = (m.positions[v as usize].y - centre.y) / half.y;
            ((0.3 - y.abs()) / 0.12).clamp(0.0, 1.0)
        });
        mesh.add_paint_layer(Some("Colour"))?;
        let texel = Texel { colour: Rgba8([r, g, b, 255]), ..Texel::BARE };
        let target = PaintTarget { texel, channels: ChannelSet::COLOUR, mode: PaintMode::Paint };
        mesh.fill_paint(&target, FillArea::SculptMask { invert: false })?;
    }
    let _ = mesh.remap_mask(|_, _, _| 0.0);
    Ok(())
}
