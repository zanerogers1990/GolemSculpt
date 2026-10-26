//! Packages for the Blender link: the one file either program writes for the
//! other when objects are sent across (`app/blender.rs` is the app's side,
//! `assets/blender/golemsculpt_link.py` is Blender's, and says the same).
//!
//! A package is all little-endian:
//!
//! ```text
//! "GSLINK01"            8 bytes
//! length of the header  u32
//! the header            JSON, UTF-8
//! then, for each object of the header, in order:
//!     its points        vertices x 3 x f32
//!     its triangles     triangles x 3 x u32
//!     and, where "polys" is not 0, its faces as they were before they were
//!     cut into triangles:
//!         their sizes   polys x u32
//!         their corners poly_corners x u32
//! then, only if some object of the header has "paint":
//!     "GSPAINT1"        8 bytes
//!     for each object that has "paint", in order:
//!         the composite      vertices x 4 x u8   (sRGB red, green, blue, alpha)
//!         each layer listed  vertices x 4 x u8   (sRGB red, green, blue, coverage)
//!         each material      vertices x f32      (its weight, 0 to 1)
//! ```
//!
//! **Paint** rides at the end, after every object, where a reader from
//! before paint (which reads its objects and stops) never looks: the format
//! stays 1, and an older GolemSculpt or an older copy of Blender's side reads
//! a package with paint as one without. An object's `"paint"` in the header
//! says what its part of the end holds:
//!
//! ```text
//! "paint": { "composite": "GolemSculpt Paint",
//!            "layers":    [ { "name", "attribute", "visible", "opacity" } ],
//!            "materials": [ { "name", "attribute", "slot" } ] }
//! ```
//!
//! `composite` and `attribute` are the names of the attributes on Blender's
//! side. From here the composite is the colour seen (the paint over the Base
//! Colour, alpha 255, as every export writes it: `io/painted.rs`), each
//! layer's colour is that layer alone with its coverage as alpha, and each
//! material is how much of it is at each point. From Blender only the
//! composite comes back, and only if the object has one: that is the colour
//! its painting there changed (`app/blender/paint.rs` says what is done with it).
//!
//! Everything in it is in this app's own terms whichever way it is going: Y
//! up, right-handed, 1 unit = 1 cm, and every point where it stands in the
//! scene, with the object's turn and size in its points. So there is nothing
//! to convert here, and nothing to guess on the way back (an OBJ, by
//! contrast, states no size, and is fitted to the scene when it is imported).
//! Blender's side turns it to Z up and to the units of its own scene.
//!
//! Each object has a `mark`, a name for that sending of it, may say it is the
//! `same_as` an earlier one, and says what it last `was`: see `app/blender.rs`
//! for what those are for.

use std::io::Write;
use std::path::Path;

use glam::{Mat4, Vec3};
use serde_json::{Value, json};

use super::painted::{ExportPaint, Painted};
use super::{LoadedMesh, Result, export_points, replace_file};
use crate::mesh::paint::{Channels, PaintLayer, PaintRow, PaintStack, Rgba8};
use crate::mesh::{Mesh, Polys};

const MAGIC: &[u8; 8] = b"GSLINK01";
/// What begins the paint at the end of a package.
const PAINT_MAGIC: &[u8; 8] = b"GSPAINT1";
/// The colour attribute the composite is, from here.
pub const COMPOSITE: &str = "GolemSculpt Paint";
/// The paint layer what Blender painted comes back as.
pub const FROM_BLENDER: &str = "From Blender";
const FORMAT: u64 = 1;
/// The file's ending. Anything else in a tray is a package still being written.
pub const LINK_ENDING: &str = "gslink";
/// A header is names and counts: one longer than this is not a header.
const HEADER_LIMIT: usize = 64 << 20;

/// One object of a package.
#[derive(Clone, Default)]
pub struct LinkObject {
    pub name: String,
    /// The object of this app's it is, as this app wrote it: `run:id`. Blender
    /// keeps it with the object and sends it back.
    pub reference: String,
    /// On the way back: the name it went to Blender under.
    pub sent_as: String,
    /// A live mirror or array copy, which the app draws itself.
    pub copy: bool,
    /// The object's centre, in the scene.
    pub origin: Vec3,
    pub mark: String,
    /// The mark of the last sending the object is still the same as, if it is.
    pub same_as: String,
    /// The mark of what was last exchanged for the object, changed since or not.
    pub was: String,
    /// Where each point stands in the scene.
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
    pub polys: Option<Polys>,
    /// Its paint, one entry per point in every list.
    pub paint: Option<LinkPaint>,
}

/// One object's paint in a package (see the module notes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkPaint {
    /// The name of the colour attribute the composite is.
    pub composite_name: String,
    /// The colour seen at each point: sRGB red, green, blue, and alpha.
    pub composite: Vec<[u8; 4]>,
    pub layers: Vec<LinkLayer>,
    pub materials: Vec<LinkMaterial>,
}

/// One paint layer's colour, as its own colour attribute in Blender.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkLayer {
    pub name: String,
    pub attribute: String,
    pub visible: bool,
    pub opacity: u8,
    /// sRGB red, green, blue and coverage, one per point.
    pub colours: Vec<[u8; 4]>,
}

/// How much of one material is at each point, as a number attribute in Blender.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkMaterial {
    pub name: String,
    pub attribute: String,
    /// The material's palette slot here.
    pub slot: u8,
    pub weights: Vec<f32>,
}

/// A package as read.
pub struct LinkPackage {
    /// `golemsculpt` or `blender`.
    #[cfg_attr(not(test), allow(dead_code))] // which tray a package is in says who sent it
    pub from: String,
    /// Whether the sender shades its models smooth.
    #[cfg_attr(not(test), allow(dead_code))] // Blender's side reads it; shading here is the scene's own
    pub smooth: bool,
    pub objects: Vec<LinkObject>,
    /// The header said some object had paint, and the paint at the end of
    /// the package could not be read: the shapes stand, without it.
    pub paint_lost: bool,
}

impl LinkObject {
    /// `mesh` as it stands at `place`, for sending, and how many of its points
    /// were not numbers and go mended (the mesh on screen is not touched, as
    /// with any export). The names and marks are the caller's to fill in. A
    /// place that is a reflection, a live mirror copy's, is written with its
    /// faces turned back the right way out.
    pub fn placed(mesh: &Mesh, place: Mat4) -> (LinkObject, usize) {
        let (points, mended) = export_points(mesh);
        let positions = points.positions.iter().map(|p| place.transform_point3(*p)).collect();
        let mut indices = mesh.indices.clone();
        let mut polys = mesh.polys.clone();
        if place.determinant() < 0.0 {
            indices.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
            if let Some(Polys { start, indices }) = &mut polys {
                for face in start.windows(2) {
                    if let Some(corners) = indices.get_mut(face[0] as usize..face[1] as usize) {
                        corners.reverse();
                    }
                }
            }
        }
        let origin = place.transform_point3(Vec3::ZERO);
        (LinkObject { origin, positions, indices, polys, ..Default::default() }, mended)
    }

    /// The object's mesh in the space `into_local` leads to from the scene's,
    /// and how many damaged points were put in order on the way: a point that
    /// is not a number never gets into a mesh, whoever sent it.
    pub fn into_mesh(self, into_local: Mat4) -> (Mesh, usize) {
        let positions = self.positions.iter().map(|p| into_local.transform_point3(*p)).collect();
        let mut indices = self.indices;
        // A space that is a reflection of the scene's shows the faces from behind.
        let flipped = into_local.determinant() < 0.0;
        if flipped {
            indices.chunks_exact_mut(3).for_each(|t| t.swap(1, 2));
        }
        let polys = self.polys.filter(|_| !flipped);
        // Colour from Blender comes in as a layer of its own, which is renumbered with the points.
        let points = self.positions.len();
        let paint = self.paint.filter(|p| p.composite.len() == points).map(|p| from_blender(&p.composite));
        let (levels, layers, groups, uvs, lost) = (None, None, None, None, [false; 5]);
        let loaded = LoadedMesh { positions, indices, mask: None, polys, hidden: None, levels, layers, groups, paint, uvs, lost };
        let (mesh, repaired, _) = loaded.build();
        (mesh, repaired)
    }
}

/// A paint stack of one layer, [`FROM_BLENDER`], of the colours Blender sent,
/// each fully covering its point: what Blender shows is the colour it has.
/// (Blender's alpha is not coverage: a colour attribute made there is
/// usually opaque or, made by a script, often all zero.)
fn from_blender(colours: &[[u8; 4]]) -> PaintStack {
    let colour = colours.iter().map(|c| Rgba8([c[0], c[1], c[2], 255])).collect();
    let row = PaintRow::new(1, FROM_BLENDER.to_owned());
    let layers = vec![PaintLayer { row, data: Channels { colour: Some(colour), ..Channels::default() } }];
    PaintStack { layers, active: Some(1), next_id: 2, ..PaintStack::new(colours.len()) }
}

/// Names Blender keeps for itself, or that would mean something else there.
const RESERVED: [&str; 12] = [
    "position",
    "normal",
    "id",
    "radius",
    "material_index",
    "sharp_face",
    "sharp_edge",
    "crease_vert",
    "crease_edge",
    "UVMap",
    "velocity",
    "shade_smooth",
];

/// An attribute name for `wanted` that none of `taken` has: Blender names an
/// attribute with at most 63 bytes, keeps the ones that begin with a dot for
/// itself, and would refuse a second attribute of one name.
fn attribute_name(wanted: &str, taken: &[String]) -> String {
    let mut name = wanted.trim().trim_start_matches('.').to_owned();
    if name.is_empty() {
        name = "Paint".into();
    }
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&name)) {
        name += " paint";
    }
    let cut = |name: &str, room: usize| {
        let mut end = room.min(name.len());
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name[..end].to_owned()
    };
    let free = |n: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    let stem = cut(&name, 63);
    if free(&stem) {
        return stem;
    }
    (2..).map(|k| format!("{} {k}", cut(&name, 63 - 1 - k.to_string().len()))).find(|n| free(n)).expect("some number is free")
}

impl LinkPaint {
    /// A mesh's paint as it goes to Blender, or `None` without paint: the
    /// composite as [`COMPOSITE`], every layer that has colour as an
    /// attribute named after it, and every material the paint uses as one
    /// named after the material.
    pub fn of(mesh: &Mesh, paint: &ExportPaint) -> Option<LinkPaint> {
        let painted = Painted::of(mesh, paint)?;
        let stack = mesh.paint_stack()?;
        let mut taken = vec![COMPOSITE.to_owned()];
        let mut layers = Vec::new();
        for layer in &stack.layers {
            let Some(colours) = &layer.data.colour else { continue };
            let attribute = attribute_name(&layer.row.name, &taken);
            taken.push(attribute.clone());
            let colours = colours.iter().map(|c| c.0).collect();
            let row = &layer.row;
            layers.push(LinkLayer { name: row.name.clone(), attribute, visible: row.visible, opacity: row.opacity, colours });
        }
        let mut materials = Vec::new();
        for &k in &painted.materials {
            let slot = painted.table[k];
            let name = paint.name_of(slot);
            let attribute = attribute_name(&name, &taken);
            taken.push(attribute.clone());
            let weights = (0..painted.seen.len()).map(|v| painted.weight(v, k)).collect();
            materials.push(LinkMaterial { name, attribute, slot, weights });
        }
        let composite = painted.seen.iter().map(|c| [c[0], c[1], c[2], 255]).collect();
        Some(LinkPaint { composite_name: COMPOSITE.to_owned(), composite, layers, materials })
    }

    /// Whether every list is one entry per point of an object of `points` points.
    fn fits(&self, points: usize) -> bool {
        self.composite.len() == points
            && self.layers.iter().all(|l| l.colours.len() == points)
            && self.materials.iter().all(|m| m.weights.len() == points)
    }

    fn header(&self) -> Value {
        let layers: Vec<Value> = self
            .layers
            .iter()
            .map(|l| json!({ "name": l.name, "attribute": l.attribute, "visible": l.visible, "opacity": l.opacity }))
            .collect();
        let materials: Vec<Value> =
            self.materials.iter().map(|m| json!({ "name": m.name, "attribute": m.attribute, "slot": m.slot })).collect();
        json!({ "composite": self.composite_name, "layers": layers, "materials": materials })
    }
}

/// Writes a package. The file appears whole or not at all, so the program
/// watching the tray never reads half of one.
pub fn write_link_package(path: &Path, from: &str, smooth: bool, objects: &[LinkObject]) -> Result<()> {
    // Paint whose lists do not fit its object is not written at all: the header and the end must agree.
    let painted: Vec<Option<&LinkPaint>> =
        objects.iter().map(|o| o.paint.as_ref().filter(|p| p.fits(o.positions.len()))).collect();
    let listed: Vec<Value> = objects
        .iter()
        .zip(&painted)
        .map(|(o, paint)| {
            let (polys, corners) = o.polys.as_ref().map_or((0, 0), |p| (p.start.len().saturating_sub(1), p.indices.len()));
            let mut entry = json!({
                "name": o.name,
                "ref": o.reference,
                "sent_as": o.sent_as,
                "copy": o.copy,
                "origin": [o.origin.x, o.origin.y, o.origin.z],
                "mark": o.mark,
                "same_as": o.same_as,
                "was": o.was,
                "vertices": o.positions.len(),
                "triangles": o.indices.len() / 3,
                "polys": polys,
                "poly_corners": corners,
            });
            if let Some(paint) = paint {
                entry["paint"] = paint.header();
            }
            entry
        })
        .collect();
    let header = json!({ "format": FORMAT, "from": from, "smooth": smooth, "objects": listed }).to_string();
    replace_file(path, |w| {
        w.write_all(MAGIC)?;
        w.write_all(&(header.len() as u32).to_le_bytes())?;
        w.write_all(header.as_bytes())?;
        for o in objects {
            w.write_all(bytemuck::cast_slice(&o.positions))?;
            w.write_all(bytemuck::cast_slice(&o.indices[..o.indices.len() / 3 * 3]))?;
            if let Some(polys) = &o.polys {
                // The file has each face's size; the mesh keeps where each begins.
                for face in polys.start.windows(2) {
                    w.write_all(&face[1].saturating_sub(face[0]).to_le_bytes())?;
                }
                w.write_all(bytemuck::cast_slice(&polys.indices))?;
            }
        }
        if painted.iter().any(Option::is_some) {
            w.write_all(PAINT_MAGIC)?;
            for paint in painted.iter().flatten() {
                w.write_all(bytemuck::cast_slice(&paint.composite))?;
                for layer in &paint.layers {
                    w.write_all(bytemuck::cast_slice(&layer.colours))?;
                }
                for material in &paint.materials {
                    w.write_all(bytemuck::cast_slice(&material.weights))?;
                }
            }
        }
        Ok(())
    })
}

/// What an object's `"paint"` in a header says its part of the end holds:
/// the composite's name, each layer's (name, attribute, visible, opacity),
/// and each material's (name, attribute, slot).
struct PaintHead {
    composite: String,
    layers: Vec<(String, String, bool, u8)>,
    materials: Vec<(String, String, u8)>,
}

impl PaintHead {
    fn of(said: &Value) -> Option<PaintHead> {
        let text = |v: &Value, name: &str| v[name].as_str().unwrap_or_default().to_owned();
        let list = |name: &str| said[name].as_array().cloned().unwrap_or_default();
        let byte = |v: &Value, name: &str, or: u64| v[name].as_u64().unwrap_or(or).min(255) as u8;
        said.is_object().then(|| PaintHead {
            composite: text(said, "composite"),
            layers: list("layers")
                .iter()
                .map(|l| (text(l, "name"), text(l, "attribute"), l["visible"].as_bool().unwrap_or(true), byte(l, "opacity", 255)))
                .collect(),
            materials: list("materials").iter().map(|m| (text(m, "name"), text(m, "attribute"), byte(m, "slot", 0))).collect(),
        })
    }

    /// The paint itself, read from `rest` (which it moves past) for an
    /// object of `points` points; `None` if `rest` is too short for it.
    fn read(&self, rest: &mut &[u8], points: usize) -> Option<LinkPaint> {
        let mut take = |each: usize| -> Option<&[u8]> {
            let length = points.checked_mul(each).filter(|&n| n <= rest.len())?;
            let (block, after) = rest.split_at(length);
            *rest = after;
            Some(block)
        };
        let colours = |block: &[u8]| block.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect::<Vec<[u8; 4]>>();
        let composite = colours(take(4)?);
        let mut layers = Vec::new();
        for (name, attribute, visible, opacity) in &self.layers {
            let colours = colours(take(4)?);
            layers.push(LinkLayer { name: name.clone(), attribute: attribute.clone(), visible: *visible, opacity: *opacity, colours });
        }
        let mut materials = Vec::new();
        for (name, attribute, slot) in &self.materials {
            let weights = take(4)?.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            materials.push(LinkMaterial { name: name.clone(), attribute: attribute.clone(), slot: *slot, weights });
        }
        Some(LinkPaint { composite_name: self.composite.clone(), composite, layers, materials })
    }
}

/// Reads the paint at the end of a package (`rest`, what follows the last
/// object) onto the objects whose header said they had some. Gives back
/// whether it could; if not, no object has any.
fn read_paint(mut rest: &[u8], heads: &[Option<PaintHead>], objects: &mut [LinkObject]) -> bool {
    if heads.iter().all(Option::is_none) {
        return true;
    }
    let Some(after) = rest.strip_prefix(PAINT_MAGIC) else { return false };
    rest = after;
    let mut read = Vec::new();
    for (head, object) in heads.iter().zip(objects.iter()) {
        read.push(match head {
            Some(head) => match head.read(&mut rest, object.positions.len()) {
                Some(paint) => Some(paint),
                None => return false,
            },
            None => None,
        });
    }
    for (object, paint) in objects.iter_mut().zip(read) {
        object.paint = paint;
    }
    true
}

/// Reads a package. A file that is not one, is cut short or is from a newer
/// version is refused whole; nothing in it is trusted to be in range (a mesh
/// is built from it only by [`LinkObject::into_mesh`], which leaves out what
/// is not).
pub fn read_link_package(path: &Path) -> Result<LinkPackage> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read what Blender sent: {e}"))?;
    let damaged = || "What Blender sent is damaged or cut short, so nothing was taken from it.".to_owned();
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        return Err(damaged());
    }
    let length = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if length > HEADER_LIMIT || bytes.len() - 12 < length {
        return Err(damaged());
    }
    let header: Value = serde_json::from_slice(&bytes[12..12 + length]).map_err(|_| damaged())?;
    if header["format"].as_u64() != Some(FORMAT) {
        return Err("What Blender sent is in a newer form than this version of GolemSculpt reads.".into());
    }
    let mut rest = &bytes[12 + length..];
    // The next `count` numbers of four bytes each, as read.
    let mut take = |count: u64| -> Result<&[u8]> {
        let wanted = usize::try_from(count).ok().and_then(|count| count.checked_mul(4)).filter(|&n| n <= rest.len());
        let (block, after) = rest.split_at(wanted.ok_or_else(damaged)?);
        rest = after;
        Ok(block)
    };
    let numbers = |block: &[u8]| block.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect::<Vec<u32>>();

    let mut objects = Vec::new();
    let mut heads = Vec::new();
    for o in header["objects"].as_array().map_or(&[][..], Vec::as_slice) {
        heads.push(PaintHead::of(&o["paint"]));
        let count = |name: &str| o[name].as_u64().unwrap_or(0);
        let text = |name: &str| o[name].as_str().unwrap_or_default().to_owned();
        let part = |i: usize| o["origin"][i].as_f64().unwrap_or(0.0) as f32;
        let three = |count: u64| count.checked_mul(3).ok_or_else(damaged);
        let positions: Vec<Vec3> = take(three(count("vertices"))?)?
            .chunks_exact(12)
            .map(|b| {
                let at = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
                Vec3::new(at(0), at(4), at(8))
            })
            .collect();
        let indices = numbers(take(three(count("triangles"))?)?);
        let polys = match count("polys") {
            0 => None,
            faces => {
                let sizes = numbers(take(faces)?);
                let corners = numbers(take(count("poly_corners"))?);
                let mut start = Vec::with_capacity(sizes.len() + 1);
                let mut at = 0u32;
                start.push(at);
                for size in sizes {
                    at = at.saturating_add(size);
                    start.push(at);
                }
                // One that does not add up is dropped by the mesh's own check; the triangles stand.
                Some(Polys { start, indices: corners })
            }
        };
        let origin = Vec3::new(part(0), part(1), part(2));
        objects.push(LinkObject {
            name: text("name"),
            reference: text("ref"),
            sent_as: text("sent_as"),
            copy: o["copy"].as_bool().unwrap_or(false),
            origin: if origin.is_finite() { origin } else { Vec3::ZERO },
            mark: text("mark"),
            same_as: text("same_as"),
            was: text("was"),
            positions,
            indices,
            polys,
            paint: None,
        });
    }
    let paint_lost = !read_paint(rest, &heads, &mut objects);
    let from = header["from"].as_str().unwrap_or_default().to_owned();
    Ok(LinkPackage { from, smooth: header["smooth"].as_bool().unwrap_or(true), objects, paint_lost })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    fn temp(name: &str) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join("sculpt_link_tests");
        std::fs::create_dir_all(&folder).unwrap();
        folder.join(name)
    }

    fn quad() -> LinkObject {
        LinkObject {
            name: "Slab \"one\"".into(),
            reference: "run:7".into(),
            origin: Vec3::new(1.0, 2.0, 3.0),
            mark: "m1".into(),
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y],
            indices: vec![0, 1, 2, 0, 2, 3],
            polys: Some(Polys { start: vec![0, 4], indices: vec![0, 1, 2, 3] }),
            ..Default::default()
        }
    }

    #[test]
    fn a_package_comes_back_as_it_was_written() {
        let ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let plain = LinkObject {
            name: "Ball".into(),
            copy: true,
            sent_as: "Ball".into(),
            same_as: "m0".into(),
            was: "m0".into(),
            mark: "m0".into(),
            positions: ball.positions.clone(),
            indices: ball.indices.clone(),
            ..Default::default()
        };
        let path = temp("whole.gslink");
        write_link_package(&path, "golemsculpt", false, &[plain.clone(), quad()]).unwrap();
        let back = read_link_package(&path).unwrap();
        assert_eq!((back.from.as_str(), back.smooth), ("golemsculpt", false));
        assert_eq!(back.objects.len(), 2);
        for (got, sent) in back.objects.iter().zip([plain, quad()]) {
            let said = |o: &LinkObject| (o.name.clone(), o.reference.clone(), o.sent_as.clone(), o.copy, o.origin, o.mark.clone(), o.same_as.clone(), o.was.clone());
            assert_eq!(said(got), said(&sent));
            assert!(got.positions == sent.positions && got.indices == sent.indices);
            let faces = |o: &LinkObject| o.polys.as_ref().map(|p| (p.start.clone(), p.indices.clone()));
            assert_eq!(faces(got), faces(&sent));
        }
        assert!(!temp("whole.gslink.tmp").exists(), "nothing is left beside it");
    }

    #[test]
    fn a_mesh_is_built_in_the_space_asked_for_with_its_quads() {
        let aside = Mat4::from_translation(Vec3::new(-1.0, -2.0, -3.0));
        let (mesh, repaired) = quad().into_mesh(aside);
        assert_eq!((mesh.triangle_count(), mesh.vertex_count(), repaired), (2, 4, 0));
        assert!(mesh.positions.iter().any(|p| (*p - Vec3::new(-1.0, -2.0, -3.0)).length() < 1e-6));
        let polys = mesh.polys.as_ref().expect("the quad came across");
        assert_eq!((polys.start.as_slice(), polys.indices.len()), (&[0, 4][..], 4));

        // Into a mirrored space, the faces are turned so they still look outward.
        let ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let volume = |mesh: &Mesh| {
            let p = |i: u32| mesh.positions[i as usize];
            mesh.indices.chunks_exact(3).map(|t| p(t[0]).dot(p(t[1]).cross(p(t[2])))).sum::<f32>() / 6.0
        };
        let whole = LinkObject { positions: ball.positions.clone(), indices: ball.indices.clone(), ..Default::default() };
        let (mirrored, _) = whole.into_mesh(Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        assert!(volume(&ball) > 0.0 && volume(&mirrored) > 0.0);
        assert!((volume(&ball) - volume(&mirrored)).abs() < 1e-4);
    }

    #[test]
    fn an_object_is_sent_where_it_stands_and_a_mirror_copy_faces_outward() {
        let mut ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
        ball.positions[5] = Vec3::NAN;
        let place = Mat4::from_scale_rotation_translation(Vec3::splat(3.0), glam::Quat::from_rotation_y(0.7), Vec3::new(4.0, 5.0, 6.0));
        let (sent, mended) = LinkObject::placed(&ball, place);
        assert_eq!((mended, sent.origin), (1, Vec3::new(4.0, 5.0, 6.0)));
        assert!(sent.positions.iter().all(|p| p.is_finite()), "a damaged point goes mended");
        assert!(sent.positions.iter().all(|p| ((*p - sent.origin).length() - 1.5).abs() < 0.2), "three times the size, about its centre");
        assert!(ball.positions[5].is_nan(), "and the mesh itself is not touched");

        let volume = |o: &LinkObject| {
            let p = |i: u32| o.positions[i as usize] - o.origin;
            o.indices.chunks_exact(3).map(|t| p(t[0]).dot(p(t[1]).cross(p(t[2])))).sum::<f32>() / 6.0
        };
        let mut quads = quad();
        quads.positions.iter_mut().for_each(|p| *p += Vec3::Z);
        let mut flat = Mesh::new(quads.positions.clone(), quads.indices.clone(), None);
        flat.polys = Some(Polys { start: vec![0, 4], indices: vec![0, 1, 2, 3] });
        let reflected = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
        let (copy, _) = LinkObject::placed(&flat, reflected);
        let (straight, _) = LinkObject::placed(&flat, Mat4::IDENTITY);
        assert!((volume(&copy) - volume(&straight)).abs() < 1e-6 && volume(&straight).abs() > 0.1);
        let faces = copy.polys.expect("the quad goes as a quad");
        assert_eq!(faces.indices, [3, 2, 1, 0], "walked the other way round");
    }

    #[test]
    fn what_is_out_of_range_or_not_a_number_never_reaches_a_mesh() {
        let mut bad = quad();
        bad.indices.extend([0, 1, 99]);
        bad.positions[3] = Vec3::NAN;
        bad.polys = Some(Polys { start: vec![0, 4], indices: vec![0, 1, 2, 99] });
        let path = temp("bad.gslink");
        write_link_package(&path, "blender", true, &[bad]).unwrap();
        let back = read_link_package(&path).unwrap().objects.remove(0);
        let (mesh, repaired) = back.into_mesh(Mat4::IDENTITY);
        assert_eq!((mesh.triangle_count(), repaired), (2, 1));
        assert!(mesh.positions.iter().all(|p| p.is_finite()));
        assert!(mesh.polys.is_none(), "a face that names a point the mesh does not hold is dropped");
    }

    #[test]
    fn a_file_that_is_not_a_whole_package_is_refused() {
        let path = temp("cut.gslink");
        write_link_package(&path, "blender", true, &[quad()]).unwrap();
        let whole = std::fs::read(&path).unwrap();
        for cut in [0, 5, 11, 40, whole.len() - 1] {
            std::fs::write(&path, &whole[..cut]).unwrap();
            assert!(read_link_package(&path).is_err(), "cut at {cut}");
        }
        std::fs::write(&path, b"SCLP0004 a scene, not a package").unwrap();
        assert!(read_link_package(&path).is_err());
        // Counts that claim more than the file holds, or more than fits in memory.
        let lying = |vertices: u64| {
            let header = json!({ "format": FORMAT, "from": "blender", "objects": [{ "name": "x", "vertices": vertices, "triangles": 0 }] }).to_string();
            let mut bytes = MAGIC.to_vec();
            bytes.extend((header.len() as u32).to_le_bytes());
            bytes.extend(header.as_bytes());
            bytes.extend([0u8; 24]);
            std::fs::write(&path, bytes).unwrap();
            read_link_package(&path).is_err()
        };
        assert!(lying(3) && lying(u64::MAX) && lying(u64::MAX / 3) && !lying(2));
        let newer = json!({ "format": FORMAT + 1, "objects": [] }).to_string();
        let mut bytes = MAGIC.to_vec();
        bytes.extend((newer.len() as u32).to_le_bytes());
        bytes.extend(newer.as_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(read_link_package(&path).is_err_and(|why| why.contains("newer")));
        assert!(read_link_package(&temp("not there.gslink")).is_err());
    }

    // ---------------------------------------------------------------- paint

    use crate::mesh::paint::palette::{PaletteEntry, ScenePaint};
    use crate::mesh::paint::{Mat8, PaintLayer, PaintRow};

    /// A ball with a "Base" colour layer (red on top), a hidden "position"
    /// layer (named as Blender names its own points), a material-only layer,
    /// and slot 4 painted on the right-hand side.
    pub(crate) fn painted_ball() -> Mesh {
        let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let n = mesh.vertex_count();
        let p = mesh.positions.clone();
        let colour = |f: &dyn Fn(usize) -> Rgba8| Some((0..n).map(f).collect::<Vec<_>>());
        let base = Channels { colour: colour(&|v| if p[v].y > 0.0 { Rgba8([200, 10, 10, 255]) } else { Rgba8([0; 4]) }), ..Default::default() };
        let odd = Channels { colour: colour(&|v| Rgba8([0, (v % 256) as u8, 0, 90])), ..Default::default() };
        let rust = (0..n).map(|v| if p[v].x > 0.2 { Mat8([4, 255]) } else { Mat8([0, 0]) }).collect();
        let metal = Channels { material: Some(rust), ..Default::default() };
        let mut stack = PaintStack::new(n);
        let mut hidden = PaintRow::new(2, "position".into());
        (hidden.visible, hidden.opacity) = (false, 40);
        stack.layers = vec![
            PaintLayer { row: PaintRow::new(1, "Base".into()), data: base },
            PaintLayer { row: hidden, data: odd },
            PaintLayer { row: PaintRow::new(3, "Metal".into()), data: metal },
        ];
        (stack.next_id, stack.active, stack.table) = (4, Some(1), vec![4]);
        mesh.set_paint(Some(stack));
        mesh
    }

    fn rust_palette() -> ExportPaint {
        let rust = PaletteEntry { slot: 4, name: "Metal - Rust".into(), swatch: [0.6, 0.2, 0.1], ..PaletteEntry::default() };
        ExportPaint::of([0.5, 0.5, 0.5], &ScenePaint { palette: vec![rust], ..ScenePaint::default() })
    }

    #[test]
    fn a_meshs_paint_goes_as_its_composite_each_layer_and_each_material() {
        let mesh = painted_ball();
        let paint = rust_palette();
        let sent = LinkPaint::of(&mesh, &paint).unwrap();
        let painted = Painted::of(&mesh, &paint).unwrap();
        assert_eq!(sent.composite_name, COMPOSITE);
        assert!(sent.composite.iter().zip(&painted.seen).all(|(c, s)| c[..3] == s[..] && c[3] == 255));
        // Only the layers that have colour, named for Blender: "position" is Blender's own.
        let layers: Vec<(&str, &str, bool, u8)> =
            sent.layers.iter().map(|l| (l.name.as_str(), l.attribute.as_str(), l.visible, l.opacity)).collect();
        assert_eq!(layers, [("Base", "Base", true, 255), ("position", "position paint", false, 40)]);
        let stack = mesh.paint_stack().unwrap();
        let raw: Vec<[u8; 4]> = stack.layers[1].data.colour.as_ref().unwrap().iter().map(|c| c.0).collect();
        assert_eq!(sent.layers[1].colours, raw, "a layer goes as it is, its coverage as alpha");
        assert_eq!(sent.materials.len(), 1);
        let rust = &sent.materials[0];
        assert_eq!((rust.name.as_str(), rust.attribute.as_str(), rust.slot), ("Metal - Rust", "Metal - Rust", 4));
        assert!(rust.weights.iter().enumerate().all(|(v, &w)| w == painted.weight(v, 0)));
        assert!(rust.weights.contains(&1.0) && rust.weights.contains(&0.0));
        assert!(LinkPaint::of(&primitives::build(Primitive::Cube, 100, 0.5), &paint).is_none());
    }

    #[test]
    fn attribute_names_are_ones_blender_takes_and_never_twice() {
        let taken = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(attribute_name("Dirt", &taken(&["GolemSculpt Paint"])), "Dirt");
        assert_eq!(attribute_name("dirt", &taken(&["Dirt"])), "dirt 2");
        assert_eq!(attribute_name("Dirt", &taken(&["Dirt", "Dirt 2"])), "Dirt 3");
        assert_eq!(attribute_name("  .hidden ", &[]), "hidden");
        assert_eq!(attribute_name("", &[]), "Paint");
        assert_eq!(attribute_name("Material_Index", &[]), "Material_Index paint");
        let long = "é".repeat(40);
        let cut = attribute_name(&long, &[]);
        assert!(cut.len() <= 63 && cut.chars().all(|c| c == 'é'));
        let again = attribute_name(&long, std::slice::from_ref(&cut));
        assert!(again.len() <= 63 && again.ends_with(" 2"), "{again}");
    }

    /// Where the objects' part of a package ends: the magic, the header, and every object's blocks.
    fn objects_end(bytes: &[u8], objects: &[LinkObject]) -> usize {
        let header = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let blocks: usize = objects
            .iter()
            .map(|o| o.positions.len() * 12 + o.indices.len() * 4 + o.polys.as_ref().map_or(0, |p| (p.start.len() - 1) * 4 + p.indices.len() * 4))
            .sum();
        12 + header + blocks
    }

    #[test]
    fn paint_rides_at_the_end_where_a_reader_from_before_never_looks() {
        let mesh = painted_ball();
        let sent = LinkPaint::of(&mesh, &rust_palette()).unwrap();
        let (mut ball, _) = LinkObject::placed(&mesh, Mat4::IDENTITY);
        (ball.name, ball.mark, ball.paint) = ("Ball".into(), "m1".into(), Some(sent.clone()));
        let mut copy = ball.clone();
        (copy.name, copy.copy) = ("Ball_MirrorX".into(), true);
        let objects = [ball, quad(), copy];
        let path = temp("painted.gslink");
        write_link_package(&path, "golemsculpt", true, &objects).unwrap();
        let back = read_link_package(&path).unwrap();
        assert!(!back.paint_lost);
        let paints: Vec<Option<&LinkPaint>> = back.objects.iter().map(|o| o.paint.as_ref()).collect();
        assert_eq!(paints, [Some(&sent), None, Some(&sent)]);

        // The same objects without paint: the objects' blocks are the same bytes, and the
        // paint is all that follows them, so a reader from before reads the same objects.
        let bare: Vec<LinkObject> = objects.iter().cloned().map(|o| LinkObject { paint: None, ..o }).collect();
        let plain = temp("plain.gslink");
        write_link_package(&plain, "golemsculpt", true, &bare).unwrap();
        let (with, without) = (std::fs::read(&path).unwrap(), std::fs::read(&plain).unwrap());
        let (end, plain_end) = (objects_end(&with, &objects), objects_end(&without, &bare));
        assert_eq!(plain_end, without.len(), "a package without paint ends with its objects, as ever");
        let header_len = |b: &[u8]| u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        assert_eq!(with[12 + header_len(&with)..end], without[12 + header_len(&without)..plain_end]);
        assert_eq!(&with[end..end + 8], PAINT_MAGIC);
        let n = mesh.vertex_count();
        assert_eq!(with.len() - end - 8, 2 * (n * 4 + 2 * n * 4 + n * 4), "composite, two layers, one material, twice");
        // And the header of a package with paint is still format 1, with each object's "paint" said.
        let header: Value = serde_json::from_slice(&with[12..12 + header_len(&with)]).unwrap();
        assert_eq!(header["format"], 1);
        assert_eq!(header["objects"][0]["paint"]["layers"][1]["attribute"], "position paint");
        assert!(header["objects"][1].get("paint").is_none());
        // An object from before paint reads the same with or without the end.
        let old = read_link_package(&plain).unwrap();
        assert!(old.objects.iter().zip(&back.objects).all(|(a, b)| a.positions == b.positions && a.indices == b.indices));
    }

    #[test]
    fn paint_cut_short_costs_the_paint_and_nothing_else() {
        let mesh = painted_ball();
        let (mut ball, _) = LinkObject::placed(&mesh, Mat4::IDENTITY);
        ball.paint = LinkPaint::of(&mesh, &rust_palette());
        let path = temp("cut paint.gslink");
        write_link_package(&path, "blender", true, std::slice::from_ref(&ball)).unwrap();
        let whole = std::fs::read(&path).unwrap();
        let end = objects_end(&whole, std::slice::from_ref(&ball));
        for cut in [end, end + 4, end + 8, whole.len() - 1] {
            std::fs::write(&path, &whole[..cut]).unwrap();
            let back = read_link_package(&path).unwrap();
            assert!(back.paint_lost && back.objects[0].paint.is_none(), "cut at {cut}");
            assert_eq!(back.objects[0].positions, ball.positions);
        }
        // Paint whose lists do not fit its object is not written, header and all.
        let mut wrong = ball.clone();
        wrong.paint.as_mut().unwrap().composite.pop();
        write_link_package(&path, "blender", true, &[wrong]).unwrap();
        let back = read_link_package(&path).unwrap();
        assert!(!back.paint_lost && back.objects[0].paint.is_none());
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(objects_end(&bytes, std::slice::from_ref(&ball)), bytes.len(), "nothing after the objects");
    }

    #[test]
    fn colour_from_blender_comes_in_as_a_layer_on_the_points_it_was_on() {
        let ball = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let n = ball.vertex_count();
        let composite: Vec<[u8; 4]> = (0..n).map(|v| [(v % 251) as u8, (v / 251 % 256) as u8, 7, 0]).collect();
        let paint = LinkPaint { composite_name: "Col".into(), composite: composite.clone(), ..LinkPaint::default() };
        // Points out of order: the mesh numbers them its own way, and the colours have to follow.
        let mut order: Vec<usize> = (0..n).collect();
        order.reverse();
        let positions: Vec<Vec3> = order.iter().map(|&v| ball.positions[v]).collect();
        let at: Vec<u32> = {
            let mut at = vec![0u32; n];
            for (new, &old) in order.iter().enumerate() {
                at[old] = new as u32;
            }
            at
        };
        let indices = ball.indices.iter().map(|&v| at[v as usize]).collect();
        let colours: Vec<[u8; 4]> = order.iter().map(|&v| composite[v]).collect();
        let entry = LinkObject { positions, indices, paint: Some(LinkPaint { composite: colours, ..paint }), ..Default::default() };
        let (mesh, _) = entry.into_mesh(Mat4::IDENTITY);
        assert_eq!(mesh.paint_rows().iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), [FROM_BLENDER]);
        let stack = mesh.paint_stack().unwrap();
        let got = stack.layers[0].data.colour.as_ref().unwrap();
        let key = |p: Vec3| p.to_array().map(f32::to_bits);
        let was: std::collections::HashMap<[u32; 3], usize> = ball.positions.iter().enumerate().map(|(v, p)| (key(*p), v)).collect();
        for (v, p) in mesh.positions.iter().enumerate() {
            let c = composite[was[&key(*p)]];
            assert_eq!(got[v], Rgba8([c[0], c[1], c[2], 255]), "covered whatever Blender's alpha said");
        }
        // Without colour, or with colour for another number of points, no paint.
        let plain = LinkObject { positions: ball.positions.clone(), indices: ball.indices.clone(), ..Default::default() };
        assert!(!plain.clone().into_mesh(Mat4::IDENTITY).0.has_paint());
        let short = LinkObject { paint: Some(LinkPaint { composite: vec![[0; 4]; 3], ..LinkPaint::default() }), ..plain };
        assert!(!short.into_mesh(Mat4::IDENTITY).0.has_paint());
    }
}
