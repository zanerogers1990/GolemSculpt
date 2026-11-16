//! Autodesk FBX in and out of the web version: the desktop's own reader
//! (`src/io/fbx_read.rs`) and writer (`src/io/fbx.rs`), compiled here as they
//! are, so an FBX the desktop opens opens here too, and what is saved here
//! is byte for byte what the desktop's Export FBX writes.
//!
//! Around them this file adds only the few pieces of the desktop's file
//! module (`src/io.rs`) they reach for: the mesh as read (`RawMesh`), the
//! export's points, and a "file" that is a byte buffer handed to the browser
//! as a download rather than a file on the disk.
//!
//! Like the desktop's: every object in the file comes in as an object of its
//! own, under its own name, standing where the file put it, its quads kept as
//! quads and its UVs and colours kept; a save writes every object shown with
//! its live mirror copies, each an object of its own in the file.
//!
//! The desktop's own tests of these files reach for the rest of its file
//! module, so the crate's own test build leaves them out (`cfg(not(test))`);
//! `web/tests/fbx.rs` tests them from outside, where the crate is built as
//! the page has it.

use glam::Mat4;

use crate::mesh::Mesh;

/// One object read from an FBX file: its name (empty when the file names
/// none) and its mesh, already in the app's axes and size.
pub struct ReadObject {
    pub name: String,
    pub mesh: Mesh,
}

/// Reads an FBX file: one object for each of its models with a mesh, in the
/// file's order, never none; and the position every one of them is given,
/// which puts the middle of them all in the middle of the scene (the
/// desktop's `io::place_imports`). `first_half_size` is the half size of the
/// scene's first object, which a file whose size is not believable is
/// fitted to.
pub fn read(bytes: &[u8], first_half_size: Option<glam::Vec3>) -> Result<(Vec<ReadObject>, glam::Vec3), String> {
    #[cfg(not(test))]
    {
        let mut raws: Vec<desk::RawMesh> = desk::parse(bytes)?.into_iter().filter(|raw| !raw.indices.is_empty()).collect();
        if raws.is_empty() {
            return Err("The file contains no triangles".into());
        }
        let at = desk::place_imports(&mut raws, first_half_size);
        let objects = raws.into_iter().map(|mut raw| ReadObject { name: std::mem::take(&mut raw.name), mesh: desk::imported_mesh(raw) }).collect();
        Ok((objects, at))
    }
    #[cfg(test)]
    {
        let _ = (bytes, first_half_size);
        Err("FBX is left out of the crate's own tests.".into())
    }
}

/// Writes `objects` (each a mesh with the places it is drawn, under the
/// names they go by) as one binary FBX, as the desktop's Export FBX does.
/// Returns the file's bytes and how many points were not numbers and were
/// written mended.
pub fn write(objects: &[(&Mesh, &[(&str, Mat4)])]) -> Result<(Vec<u8>, usize), String> {
    #[cfg(not(test))]
    {
        let paint = desk::painted::ExportPaint::default();
        let mended = desk::fbx::export_fbx_painted(objects, &paint, std::path::Path::new(desk::OUT_NAME))?;
        Ok((desk::take_written(), mended))
    }
    #[cfg(test)]
    {
        let _ = objects;
        Err("FBX is left out of the crate's own tests.".into())
    }
}

/// The desktop's file module, as much of it as FBX needs: its files mounted
/// from `src/io` (so their own `mod` lines find their sub-files), and the
/// few items of `src/io.rs` they use, here.
#[cfg(not(test))]
#[path = "../../src/io"]
#[allow(dead_code, clippy::all)]
mod desk {
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::Path;

    use glam::Vec3;

    use crate::mesh::{Mesh, Polys};

    pub mod fbx;
    pub mod fbx_read;
    pub mod painted;
    mod polygons;
    mod repair;

    pub type Result<T> = std::result::Result<T, String>;

    /// The objects of a binary FBX file (`fbx_read.rs`).
    pub fn parse(bytes: &[u8]) -> Result<Vec<RawMesh>> {
        fbx_read::parse(bytes)
    }

    /// The desktop's `io::RawMesh`: a model as read from a file, already in
    /// the app's axes and units.
    #[derive(Default, Debug)]
    pub struct RawMesh {
        pub name: String,
        pub positions: Vec<Vec3>,
        pub indices: Vec<u32>,
        pub real_size: bool,
        pub groups: Vec<u16>,
        pub colours: Vec<crate::mesh::paint::Rgba8>,
        pub polys: Option<Polys>,
        pub uvs: Vec<glam::Vec2>,
    }

    /// The desktop's `io::groups::imported_mesh`: a mesh read from a file,
    /// built with its quads, UVs and colours.
    pub fn imported_mesh(raw: RawMesh) -> Mesh {
        let with_uvs = !raw.uvs.is_empty() && raw.uvs.len() == raw.indices.len();
        let painted = !raw.colours.is_empty() && raw.colours.len() == raw.positions.len();
        let paint = painted.then(|| painted::imported_stack(raw.colours));
        if raw.polys.is_some() || with_uvs {
            return crate::io::build_imported(raw.positions, raw.indices, raw.polys, with_uvs.then_some(raw.uvs), paint);
        }
        match paint {
            Some(stack) => crate::mesh::paint::built_with(raw.positions, raw.indices, None, &raw.groups, Some(&stack)),
            None => Mesh::new(raw.positions, raw.indices, None),
        }
    }

    /// The smallest and largest a model that states its size is believed to
    /// be across, in app units: half a millimetre and fifty metres (`io.rs`).
    const SMALLEST_REAL: f32 = 0.05;
    const LARGEST_REAL: f32 = 5_000.0;

    /// The desktop's `io::place_imports`: one scale for every object a file
    /// held (only where its size cannot be believed), and one position.
    pub fn place_imports(objects: &mut [RawMesh], first_half_size: Option<Vec3>) -> Vec3 {
        let bounds_of = |points: &[Vec3]| {
            points.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), &p| (lo.min(p), hi.max(p)))
        };
        let (mut lo, mut hi) = objects
            .iter()
            .map(|raw| bounds_of(&raw.positions))
            .fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), (a, b)| (lo.min(a), hi.max(b)));
        let across = (hi - lo).max_element();
        let believable = (SMALLEST_REAL..=LARGEST_REAL).contains(&across);
        if !objects.iter().all(|raw| raw.real_size) || !believable {
            if let Some(first) = first_half_size {
                let target = (first.x + first.y + first.z) / 3.0 * 2.0;
                let model = ((hi - lo) * 0.5).max_element();
                if model > 1e-5 && target > 1e-5 {
                    let scale = target / model;
                    for p in objects.iter_mut().flat_map(|raw| raw.positions.iter_mut()) {
                        *p *= scale;
                    }
                    (lo, hi) = (lo * scale, hi * scale);
                }
            }
        }
        -(lo + hi) * 0.5
    }

    /// The desktop's (`io.rs`): exact welding, with where each point went
    /// and the first point of each place.
    fn weld_exact_mapped(positions: Vec<Vec3>) -> (Vec<Vec3>, Vec<u32>, Vec<u32>) {
        let mut map: HashMap<[u32; 3], u32> = HashMap::with_capacity(positions.len());
        let mut out: Vec<Vec3> = Vec::new();
        let mut first = Vec::new();
        let mut remap = Vec::with_capacity(positions.len());
        for (i, p) in positions.iter().enumerate() {
            // +0.0 so that -0.0 and 0.0 weld together.
            let key = [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
            let id = *map.entry(key).or_insert_with(|| {
                out.push(*p);
                first.push(i as u32);
                out.len() as u32 - 1
            });
            remap.push(id);
        }
        (out, remap, first)
    }

    /// The desktop's (`io.rs`): a triangle with three different corners, all numbers.
    fn sound_triangle(positions: &[Vec3], t: &[u32]) -> bool {
        t[0] != t[1] && t[1] != t[2] && t[0] != t[2] && t.iter().all(|&i| positions[i as usize].is_finite())
    }

    fn nothing_to_export(mesh: &Mesh) -> Result<()> {
        if mesh.indices.is_empty() {
            return Err("There is nothing to export: this object has no surface.".into());
        }
        Ok(())
    }

    /// The desktop's (`io.rs`): the points as an export writes them, mended
    /// where some are not numbers.
    struct Points<'a> {
        positions: Cow<'a, [Vec3]>,
        normals: Cow<'a, [Vec3]>,
    }

    fn export_points(mesh: &Mesh) -> (Points<'_>, usize) {
        let Some((positions, _)) = repair::mended(mesh) else {
            return (Points { positions: Cow::Borrowed(&mesh.positions), normals: Cow::Borrowed(&mesh.normals) }, 0);
        };
        let normals = vertex_normals(&positions, &mesh.indices);
        let points = Points { positions: Cow::Owned(positions), normals: Cow::Owned(normals) };
        (points, repair::damaged_points(mesh))
    }

    fn vertex_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
        let mut sums = vec![Vec3::ZERO; positions.len()];
        for t in indices.chunks_exact(3) {
            let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
            if a.max(b).max(c) >= positions.len() {
                continue;
            }
            let area_normal = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
            for i in [a, b, c] {
                sums[i] += area_normal;
            }
        }
        sums.into_iter().map(|n| n.normalize_or_zero()).collect()
    }

    /// What the web's "file" is called: there is no disk, the bytes go to
    /// [`take_written`].
    pub const OUT_NAME: &str = "golem-sculpt.fbx";

    thread_local! {
        static WRITTEN: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    /// The desktop writes a file beside the old one and swaps it in; here
    /// the whole file is gathered in memory, to be downloaded.
    pub(crate) fn replace_file(_path: &Path, write: impl FnOnce(&mut Vec<u8>) -> std::io::Result<()>) -> Result<()> {
        let mut bytes = Vec::new();
        write(&mut bytes).map_err(|e| format!("Could not write the file: {e}"))?;
        WRITTEN.with(|w| *w.borrow_mut() = bytes);
        Ok(())
    }

    /// The bytes the last write made.
    pub fn take_written() -> Vec<u8> {
        WRITTEN.with(|w| std::mem::take(&mut *w.borrow_mut()))
    }

    /// Local time as year-month-day_hourminutesecond, as the desktop's.
    pub(crate) fn time_stamp() -> String {
        #[cfg(target_arch = "wasm32")]
        {
            let d = js_sys::Date::new_0();
            format!(
                "{:04}-{:02}-{:02}_{:02}{:02}{:02}",
                d.get_full_year(),
                d.get_month() + 1,
                d.get_date(),
                d.get_hours(),
                d.get_minutes(),
                d.get_seconds()
            )
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            "1970-01-01_000000".to_owned()
        }
    }
}

#[cfg(not(test))]
pub(crate) use desk::painted;
