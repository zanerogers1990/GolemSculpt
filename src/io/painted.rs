//! What the exporters write of an object's paint (B.8 of the Phase 5 design):
//! the one place OBJ, PLY, FBX and the Blender link learn what a painted
//! vertex looks like, so the four agree.
//!
//! **The colour that goes out is the colour that is seen**: the composite of
//! the paint layers (`mesh::paint::export_streams`) laid over the scene's
//! Base Colour by its coverage, in sRGB, byte for byte. A vertex nothing was
//! painted on goes out as the Base Colour, a half-covered one halfway between.
//! The other programs have no idea of "unpainted", and a model that went out
//! black wherever it was not painted would be no use to anyone. The Base
//! Colour is the Look panel's (`Options::base_color`); a finish preset (Clay,
//! Metal...) tints the model on screen with colours of its own, which no
//! file format can carry, so they are not baked in. The mix is done on the
//! sRGB numbers, as the layers themselves are mixed (`mesh/paint/blend.rs`).
//!
//! **Materials** go out as their weights (how much of each of the object's
//! materials is at each vertex, 0 to 1) where a format has room for numbers
//! of our own (PLY, the Blender link, FBX's game-shader option), and as the
//! one material that has most of each face where a format only knows a
//! material per face (FBX). "Most" counts the Look's own finish too, the
//! surface no material was painted on: it is the material called
//! [`BASE_MATERIAL`].
//!
//! **A bake hook.** Nothing here has UVs yet, so there is no "Bake Paint to
//! Texture". When there is, it samples the paint at a point of a triangle
//! with [`baked_texel`] (built on `mesh::paint::sample`, which is the
//! composite mixed by barycentric weights), so a texture shows exactly the
//! colour the vertices export.

use crate::mesh::Mesh;
use crate::mesh::paint::palette::ScenePaint;
use crate::mesh::paint::{self, Channels, PaintLayer, PaintRow, PaintStack, Rgba8, Stream};

/// The name of the material of the surface no material was painted on.
pub const BASE_MATERIAL: &str = "GolemSculpt Base";

/// What an export needs to know of the scene to write paint.
#[derive(Clone, Debug)]
pub struct ExportPaint {
    /// The colour where nothing is painted: the scene's Base Colour, sRGB bytes.
    pub base: [u8; 3],
    /// The scene's palette: slot, name, swatch (sRGB 0 to 1).
    pub palette: Vec<(u8, String, [f32; 3])>,
    /// FBX only: each object's material weights also go into its second and
    /// later UV sets, for game shaders (off unless asked for).
    pub weights_in_uvs: bool,
}

impl Default for ExportPaint {
    /// The paint of a scene with the default Base Colour and no palette.
    fn default() -> Self {
        ExportPaint::of(crate::doc::Options::default().base_color, &ScenePaint::default())
    }
}

impl ExportPaint {
    pub fn of(base_color: [f32; 3], scene: &ScenePaint) -> ExportPaint {
        let palette = scene.palette.iter().map(|e| (e.slot, e.name.clone(), e.swatch)).collect();
        ExportPaint { base: base_color.map(unit_byte), palette, weights_in_uvs: false }
    }

    /// A palette slot's name; one the palette does not have (a file from
    /// elsewhere) is "Material <slot>".
    pub fn name_of(&self, slot: u8) -> String {
        match self.palette.iter().find(|(s, ..)| *s == slot) {
            Some((_, name, _)) if !name.trim().is_empty() => name.trim().to_owned(),
            _ => format!("Material {slot}"),
        }
    }

    /// A palette slot's swatch, sRGB 0 to 1; mid grey for one it does not have.
    pub fn swatch_of(&self, slot: u8) -> [f32; 3] {
        self.palette.iter().find(|(s, ..)| *s == slot).map_or([0.5; 3], |(_, _, swatch)| *swatch)
    }

    /// The Base Colour, sRGB 0 to 1.
    pub fn base_unit(&self) -> [f32; 3] {
        self.base.map(|b| b as f32 / 255.0)
    }
}

fn unit_byte(x: f32) -> u8 {
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

/// sRGB 0 to 1 as linear light: what FBX and Blender keep a material's colour in.
pub fn linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// A vertex's composite record laid over `base` by its coverage: the colour
/// that is seen there.
pub fn seen(record: &Stream, base: [u8; 3]) -> [u8; 3] {
    let a = record[3] as u32;
    let mix = |k: usize| ((record[k] as u32 * a + base[k] as u32 * (255 - a) + 127) / 255) as u8;
    [mix(0), mix(1), mix(2)]
}

/// One mesh's paint as the exporters read it.
pub struct Painted {
    /// The composite, one record per vertex.
    pub streams: Vec<Stream>,
    /// The colour seen at each vertex (see the module notes).
    pub seen: Vec<[u8; 3]>,
    /// The local material table: weight `k` of a record is of palette slot `table[k]`.
    pub table: Vec<u8>,
    /// The table entries some vertex has any weight of, in table order.
    pub materials: Vec<usize>,
}

impl Painted {
    /// `None` for a mesh without paint.
    pub fn of(mesh: &Mesh, paint: &ExportPaint) -> Option<Painted> {
        use rayon::prelude::*;
        let streams = paint::export_streams(mesh)?;
        let table = mesh.paint_stack().map(|s| s.table.clone()).unwrap_or_default();
        let seen = streams.par_iter().with_min_len(1 << 13).map(|r| seen(r, paint.base)).collect();
        let mut any = [false; paint::MOST_MATERIALS];
        for r in &streams {
            for (k, flag) in any.iter_mut().enumerate().take(table.len()) {
                *flag |= r[8 + k] > 0;
            }
        }
        let materials = (0..table.len()).filter(|&k| any[k]).collect();
        Some(Painted { streams, seen, table, materials })
    }

    /// Weight of table entry `k` at vertex `v`, 0 to 1.
    pub fn weight(&self, v: usize, k: usize) -> f32 {
        self.streams.get(v).map_or(0.0, |r| r[8 + k] as f32 / 255.0)
    }

    /// The material with most of a face, by the sum of its corners' weights:
    /// `None` for the Look's own finish (what no material was painted on, and
    /// also what wins a tie), else the table entry.
    pub fn dominant(&self, corners: &[u32]) -> Option<usize> {
        let mut sums = [0u32; paint::MOST_MATERIALS];
        let mut base = 0u32;
        for &v in corners {
            let Some(r) = self.streams.get(v as usize) else { continue };
            let painted: u32 = r[8..16].iter().map(|&w| w as u32).sum();
            base += 255u32.saturating_sub(painted);
            for (k, sum) in sums.iter_mut().enumerate() {
                *sum += r[8 + k] as u32;
            }
        }
        let (mut best, mut most) = (None, base);
        for &k in &self.materials {
            if sums[k] > most {
                (best, most) = (Some(k), sums[k]);
            }
        }
        best
    }
}

// ------------------------------------------------------------------ OBJ in

/// The layer an OBJ's vertex colours come in as.
pub const IMPORTED_LAYER: &str = "Imported Colours";

/// The colours of an OBJ's `v x y z r g b` lines as they are read: one entry
/// per `v` line, `None` where a line has no colour.
#[derive(Default)]
pub(super) struct ObjColours {
    colours: Vec<Option<[f32; 3]>>,
    any: bool,
    /// The largest number met: past 1, the file counts 0 to 255.
    most: f32,
}

impl ObjColours {
    /// Notes the colour of a `v` line whose numbers after x y z are `rest`.
    pub(super) fn vertex<'a>(&mut self, rest: impl Iterator<Item = &'a str>) {
        let numbers: Vec<f32> = rest.take(3).map_while(|s| s.parse::<f32>().ok()).collect();
        let colour = match numbers[..] {
            [r, g, b] if [r, g, b].iter().all(|c| c.is_finite() && *c >= 0.0) => Some([r, g, b]),
            _ => None,
        };
        if let Some(c) = colour {
            self.any = true;
            self.most = self.most.max(c[0]).max(c[1]).max(c[2]);
        }
        self.colours.push(colour);
    }

    /// The colours as bytes with coverage, one per `v` line: coverage 255
    /// where the line had a colour, 0 where it had none. Empty for a file
    /// with no colours at all.
    pub(super) fn bytes(&self) -> Vec<Rgba8> {
        if !self.any {
            return Vec::new();
        }
        let scale = if self.most > 1.0 + 1e-4 { 1.0 } else { 255.0 };
        let byte = |x: f32| (x * scale).round().clamp(0.0, 255.0) as u8;
        self.colours
            .iter()
            .map(|c| c.map_or(Rgba8([0; 4]), |c| Rgba8([byte(c[0]), byte(c[1]), byte(c[2]), 255])))
            .collect()
    }
}

/// A paint stack of one layer, [`IMPORTED_LAYER`], holding `colours` (one per
/// vertex): what an OBJ with colours comes in with.
pub fn imported_stack(colours: Vec<Rgba8>) -> PaintStack {
    let len = colours.len();
    let row = PaintRow::new(1, IMPORTED_LAYER.to_owned());
    let data = Channels { colour: Some(colours), ..Channels::default() };
    PaintStack { layers: vec![PaintLayer { row, data }], active: Some(1), next_id: 2, ..PaintStack::new(len) }
}

// ------------------------------------------------------------------ baking

/// What a future "Bake Paint to Texture" writes into one texel: the colour
/// seen at the point `bary` of triangle `tri`, sRGB 0 to 1, and how much of
/// each material is there (palette slot, weight). `None` without paint.
///
/// The colour is each corner's seen colour (laid over the Base Colour, as
/// every export lays it) mixed by `bary`, which is how a viewer blends the
/// vertex colours an export writes, so a baked texture and the exported
/// vertex colours agree. (Mixing the corners' paint first and laying the mix
/// over the base afterwards would not: a red corner with no coverage would
/// tint its neighbours.) This is the one-point form, built on
/// `mesh::paint::sample`; a bake of a whole texture works the composite out
/// once (`Painted::of`) and mixes `Painted::seen` the same way.
#[allow(dead_code)] // the hook for baking to a texture, which needs UVs first
pub fn baked_texel(mesh: &Mesh, tri: u32, bary: [f32; 3], paint: &ExportPaint) -> Option<([f32; 3], Vec<(u8, f32)>)> {
    let base = paint.base_unit();
    let corner = |k: usize| {
        let mut at = [0.0; 3];
        at[k] = 1.0;
        let c = paint::sample(mesh, tri, at)?.colour;
        Some([0, 1, 2].map(|i| c[i] * c[3] + base[i] * (1.0 - c[3])))
    };
    let corners = [corner(0)?, corner(1)?, corner(2)?];
    let colour = [0, 1, 2].map(|i| (0..3).map(|k| corners[k][i] * bary[k]).sum::<f32>());
    Some((colour, paint::sample(mesh, tri, bary)?.materials))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::paint::palette::ScenePaint;
    use crate::mesh::primitives::{self, Primitive};

    #[test]
    fn what_is_seen_is_the_paint_over_the_base_by_its_coverage() {
        let base = [100, 150, 200];
        let record = |rgba: [u8; 4]| {
            let mut r = [0u8; 16];
            r[..4].copy_from_slice(&rgba);
            r
        };
        assert_eq!(seen(&record([10, 20, 30, 255]), base), [10, 20, 30]);
        assert_eq!(seen(&record([10, 20, 30, 0]), base), base);
        assert_eq!(seen(&record([0, 0, 0, 128]), base), [50, 75, 100]);
    }

    #[test]
    fn the_dominant_material_counts_the_unpainted_surface_too() {
        let mut mesh = primitives::build(Primitive::Cube, 12, 0.5);
        let n = mesh.vertex_count();
        let mut stack = PaintStack::new(n);
        stack.table = vec![4, 9];
        let mut material = vec![paint::Mat8([0, 0]); n];
        material[0] = paint::Mat8([9, 255]);
        material[1] = paint::Mat8([9, 100]);
        material[2] = paint::Mat8([4, 120]);
        let data = Channels { material: Some(material), ..Channels::default() };
        stack.layers.push(PaintLayer { row: PaintRow::new(1, "M".into()), data });
        stack.next_id = 2;
        mesh.set_paint(Some(stack));
        let painted = Painted::of(&mesh, &ExportPaint::default()).unwrap();
        assert_eq!(painted.materials, [0, 1]);
        // 355 of slot 9, 120 of slot 4, 290 of nothing.
        assert_eq!(painted.dominant(&[0, 1, 2]), Some(1));
        // 120 of slot 4 against 390 of nothing.
        assert_eq!(painted.dominant(&[2, 3]), None);
        assert_eq!(painted.dominant(&[3, 4, 5]), None);
    }

    #[test]
    fn a_baked_texel_at_a_corner_is_that_vertexs_exported_colour() {
        let mut mesh = primitives::build(Primitive::Sphere, 200, 0.5);
        let n = mesh.vertex_count();
        let colours = (0..n).map(|v| Rgba8([(v * 7 % 256) as u8, 40, 200, (v * 13 % 256) as u8])).collect();
        mesh.set_paint(Some(imported_stack(colours)));
        let paint = ExportPaint::default();
        let painted = Painted::of(&mesh, &paint).unwrap();
        let t = 5usize;
        let corners: Vec<usize> = mesh.indices[t * 3..t * 3 + 3].iter().map(|&v| v as usize).collect();
        let seen_at = |v: usize| painted.seen[v].map(|b| b as f32 / 255.0);
        let (colour, _) = baked_texel(&mesh, t as u32, [0.0, 1.0, 0.0], &paint).unwrap();
        let want = seen_at(corners[1]);
        for k in 0..3 {
            assert!((colour[k] - want[k]).abs() <= 1.0 / 255.0, "{colour:?} {want:?}");
        }
        // Between corners: the corners' exported colours mixed, as a viewer mixes them.
        let bary = [0.2, 0.3, 0.5];
        let (colour, _) = baked_texel(&mesh, t as u32, bary, &paint).unwrap();
        for k in 0..3 {
            let want: f32 = corners.iter().zip(bary).map(|(&v, w)| seen_at(v)[k] * w).sum();
            assert!((colour[k] - want).abs() <= 1.5 / 255.0, "{colour:?} {want}");
        }
    }

    /// A sphere with two colour layers: red over its top half, and a
    /// half-covered green over everything right of the middle.
    fn painted_sphere() -> Mesh {
        let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let n = mesh.vertex_count();
        let p = mesh.positions.clone();
        let layer = |id: u32, name: &str, colour: Vec<Rgba8>| PaintLayer {
            row: PaintRow::new(id, name.into()),
            data: Channels { colour: Some(colour), ..Channels::default() },
        };
        let red = (0..n).map(|v| if p[v].y > 0.0 { Rgba8([220, 20, 10, 255]) } else { Rgba8([0; 4]) }).collect();
        let green = (0..n).map(|v| if p[v].x > 0.0 { Rgba8([0, 200, 0, 128]) } else { Rgba8([0; 4]) }).collect();
        let mut stack = PaintStack::new(n);
        stack.layers = vec![layer(1, "Red", red), layer(2, "Green", green)];
        (stack.next_id, stack.active) = (3, Some(2));
        mesh.set_paint(Some(stack));
        mesh
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("sculpt_io_painted_tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn an_obj_carries_the_colour_seen_and_comes_back_with_it_as_a_layer() {
        let mesh = painted_sphere();
        let paint = ExportPaint::of([0.2, 0.4, 0.6], &ScenePaint::default());
        let painted = Painted::of(&mesh, &paint).unwrap();
        let path = temp("painted.obj");
        super::super::export_obj_painted(&mesh, &[("", glam::Mat4::IDENTITY)], &paint, &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("v ")).collect();
        assert_eq!(lines.len(), mesh.vertex_count());
        for (v, line) in lines.iter().enumerate() {
            let words: Vec<&str> = line.split(' ').collect();
            let want = painted.seen[v].map(|b| format!("{:.4}", b as f32 / 255.0));
            assert_eq!(words[4..], want, "vertex {v}: {line}");
        }
        // All three kinds of vertex are there: bare, covered, half covered.
        assert!(painted.seen.contains(&[51, 102, 153]) && painted.seen.contains(&[220, 20, 10]));
        assert!(painted.seen.iter().any(|c| c[1] > 120 && c[1] < 200));

        let back = super::super::imported_mesh(super::super::import(&path).unwrap());
        let rows = back.paint_rows();
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), [IMPORTED_LAYER]);
        let stack = back.paint_stack().unwrap();
        let colours = stack.layers[0].data.colour.as_ref().unwrap();
        let key = |p: glam::Vec3| p.to_array().map(f32::to_bits);
        let was: std::collections::HashMap<[u32; 3], usize> = mesh.positions.iter().enumerate().map(|(v, p)| (key(*p), v)).collect();
        assert_eq!(back.vertex_count(), mesh.vertex_count());
        for (v, p) in back.positions.iter().enumerate() {
            let old = was[&key(*p)];
            let [r, g, b] = painted.seen[old];
            assert_eq!(colours[v], Rgba8([r, g, b, 255]), "vertex {v}");
        }
        // Exported again, it writes the same colours: nothing drifts on a round trip.
        assert_eq!(Painted::of(&back, &paint).unwrap().seen.iter().filter(|c| !painted.seen.contains(c)).count(), 0);
    }

    /// Writes `painted.obj` and `painted.ply` and the colour each point should
    /// have into the folder GOLEMSCULPT_PAINT_CHECK names, for
    /// tests/blender_link/check_obj_ply_paint.py (see tests/blender_link/README.md).
    #[test]
    #[ignore = "writes the files tests/blender_link/check_obj_ply_paint.py reads in Blender"]
    fn blender_obj_ply_check_files() {
        let folder = std::path::PathBuf::from(std::env::var_os("GOLEMSCULPT_PAINT_CHECK").expect("set GOLEMSCULPT_PAINT_CHECK"));
        std::fs::create_dir_all(&folder).unwrap();
        let mesh = painted_sphere();
        let paint = ExportPaint::of([0.2, 0.4, 0.6], &ScenePaint::default());
        let painted = Painted::of(&mesh, &paint).unwrap();
        super::super::export_obj_painted(&mesh, &[("", glam::Mat4::IDENTITY)], &paint, &folder.join("painted.obj")).unwrap();
        super::super::export_ply_placed(&mesh, &[glam::Mat4::IDENTITY], &paint, &folder.join("painted.ply")).unwrap();
        let expect = serde_json::json!({ "vertices": mesh.vertex_count(), "seen": painted.seen });
        std::fs::write(folder.join("obj_ply_expect.json"), expect.to_string()).unwrap();
    }

    #[test]
    fn an_stl_of_a_painted_model_is_the_stl_of_its_shape() {
        let painted = painted_sphere();
        let mut bare = painted_sphere();
        bare.set_paint(None);
        let models = [glam::Mat4::IDENTITY, glam::Mat4::from_scale(glam::Vec3::new(-1.0, 1.0, 1.0))];
        let (a, b) = (temp("painted.stl"), temp("bare.stl"));
        super::super::export_stl_placed(&painted, &models, &a).unwrap();
        super::super::export_stl_placed(&bare, &models, &b).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    }

    #[test]
    fn an_unpainted_obj_goes_and_comes_as_shape_alone() {
        let mesh = primitives::build(Primitive::Sphere, 500, 0.5);
        let path = temp("plain.obj");
        super::super::export_obj_painted(&mesh, &[("", glam::Mat4::IDENTITY)], &ExportPaint::default(), &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().filter(|l| l.starts_with("v ")).all(|l| l.split(' ').count() == 4));
        let back = super::super::imported_mesh(super::super::import(&path).unwrap());
        assert!(!back.has_paint());
    }

    #[test]
    fn obj_colours_read_in_either_scale_and_lines_without_colour_are_bare() {
        let mut c = ObjColours::default();
        c.vertex("0.5 1 0".split(' '));
        c.vertex(std::iter::empty());
        c.vertex("1 0 0.25".split(' '));
        assert_eq!(c.bytes(), [Rgba8([128, 255, 0, 255]), Rgba8([0; 4]), Rgba8([255, 0, 64, 255])]);
        let mut c = ObjColours::default();
        c.vertex("200 10 0".split(' '));
        assert_eq!(c.bytes(), [Rgba8([200, 10, 0, 255])]);
        let mut none = ObjColours::default();
        none.vertex(std::iter::empty());
        assert!(none.bytes().is_empty());
    }
}
