//! Open and Save FBX in the web app: what the File card's buttons and a
//! dropped `.fbx` do with the desktop's reader and writer (`crate::fbx`).

use glam::Mat4;

use super::GolemApp;
use crate::mesh::Mesh;
use crate::scene::Props;

/// What a saved FBX is called.
const FILE_NAME: &str = "golem-sculpt.fbx";

/// Whether `name` is an FBX file.
pub fn is_fbx(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".fbx")
}

/// The objects of a save as its status line names them: `"Ball"`,
/// `"Ball" and "Cube"`, and past three of them `"Ball", "Cube" and 3 more`
/// (the desktop's `export_fbx.rs`).
fn named(names: &[&str]) -> String {
    let quoted = |name: &&str| format!("\"{name}\"");
    match names {
        [] => String::new(),
        [only] => quoted(only),
        [first, second] => format!("{} and {}", quoted(first), quoted(second)),
        [first, second, third] => format!("{}, {} and {}", quoted(first), quoted(second), quoted(third)),
        [first, second, rest @ ..] => format!("{}, {} and {} more", quoted(first), quoted(second), rest.len()),
    }
}

impl GolemApp {
    /// An FBX picked or dropped: each object in it a new object of the
    /// scene, under its own name (the file's for one it does not name),
    /// standing together as they did in the program that wrote them.
    pub(super) fn open_fbx(&mut self, name: &str, bytes: &[u8]) {
        let first_half_size = self.objects.first().map(|o| self.mesh_of(o).bounds()).filter(|b| !b.is_empty()).map(|b| b.size() * 0.5);
        let (objects, at) = match crate::fbx::read(bytes, first_half_size) {
            Ok(read) => read,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem).to_owned();
        let count = objects.len();
        let mut tris = 0;
        for (i, o) in objects.into_iter().enumerate() {
            tris += o.mesh.live_triangle_count();
            let props = Props { position: at, ..Props::named(if o.name.is_empty() { stem.clone() } else { o.name }) };
            // The last one read is the one worked on, as with any file opened.
            self.add_object(o.mesh, props, i + 1 == count);
        }
        self.frame_model(true);
        let what = if count == 1 { "as a new object".to_owned() } else { format!("as {count} new objects") };
        self.status = format!("Opened {name}: {} triangles, {what}.", super::group_digits(tris));
    }

    /// Save FBX: every object shown, where it stands, with its live mirror
    /// copies, in one binary FBX (as the desktop's Export FBX writes the
    /// objects selected; Save OBJ and STL here write every object shown).
    pub(super) fn save_fbx(&mut self) {
        if self.sculptor.is_active() {
            return;
        }
        // Each object's mesh as it goes to a file, and its places with their names.
        let mut tidied: Vec<(String, Option<Mesh>, Vec<(String, Mat4)>, u64)> = Vec::new();
        for o in self.objects.iter().filter(|o| o.visible) {
            let mesh = self.mesh_of(o);
            if mesh.indices.is_empty() {
                continue;
            }
            let name = o.name.replace(' ', "_");
            let mut places = vec![(name.clone(), o.matrix())];
            if let Some(mirror) = o.mirror {
                places.extend(mirror.copies().iter().map(|c| (format!("{name}_Mirror{}", c.axis_name()), o.placed_matrix(c.flip_mask))));
            }
            tidied.push((o.name.clone(), super::tidy(mesh), places, o.id));
        }
        if tidied.is_empty() {
            self.status = "Every object is hidden: show one to save it.".into();
            return;
        }
        let meshes: Vec<&Mesh> = tidied
            .iter()
            .map(|(_, t, _, id)| match t {
                Some(m) => m,
                None => self.mesh_of(self.objects.iter().find(|o| o.id == *id).expect("an object just listed")),
            })
            .collect();
        let places: Vec<Vec<(&str, Mat4)>> = tidied.iter().map(|(_, _, p, _)| p.iter().map(|(n, m)| (n.as_str(), *m)).collect()).collect();
        let objects: Vec<(&Mesh, &[(&str, Mat4)])> = meshes.iter().zip(&places).map(|(m, p)| (*m, p.as_slice())).collect();
        let (bytes, mended) = match crate::fbx::write(&objects) {
            Ok(written) => written,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        crate::browser::download(FILE_NAME, &bytes, "application/octet-stream");
        let names: Vec<&str> = tidied.iter().map(|(n, ..)| n.as_str()).collect();
        let copies = if tidied.iter().any(|(_, _, p, _)| p.len() > 1) { " with their live copies" } else { "" };
        let what = match names.as_slice() {
            [_] if copies.is_empty() => named(&names),
            [_] => format!("{} with its live copies", named(&names)),
            _ => format!("{} objects ({}){copies}", names.len(), named(&names)),
        };
        let mended = match mended {
            0 => String::new(),
            1 => ". 1 point that was not a number was mended in the file".to_owned(),
            n => format!(". {n} points that were not numbers were mended in the file"),
        };
        self.status = format!("Saved {what} to {FILE_NAME}{mended}.");
    }
}
