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
//! ```
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

use super::{LoadedMesh, Result, export_points, replace_file};
use crate::mesh::{Mesh, Polys};

const MAGIC: &[u8; 8] = b"GSLINK01";
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
        let (mesh, repaired, _) = LoadedMesh { positions, indices, mask: None, polys, hidden: None }.build();
        (mesh, repaired)
    }
}

/// Writes a package. The file appears whole or not at all, so the program
/// watching the tray never reads half of one.
pub fn write_link_package(path: &Path, from: &str, smooth: bool, objects: &[LinkObject]) -> Result<()> {
    let listed: Vec<Value> = objects
        .iter()
        .map(|o| {
            let (polys, corners) = o.polys.as_ref().map_or((0, 0), |p| (p.start.len().saturating_sub(1), p.indices.len()));
            json!({
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
            })
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
        Ok(())
    })
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
    for o in header["objects"].as_array().map_or(&[][..], Vec::as_slice) {
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
        });
    }
    Ok(LinkPackage { from: header["from"].as_str().unwrap_or_default().to_owned(), smooth: header["smooth"].as_bool().unwrap_or(true), objects })
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
}
