//! Export FBX: every selected object, where it stands and with its live
//! copies, in one Autodesk FBX for Blender, Unity and the like (`io/fbx.rs`).
//!
//! What goes into the file is what is selected, which is the Objects list's
//! own to say (Ctrl or Shift + click on a row adds one): one object selected
//! is a file of that object, and several are a file of them all, each an
//! object of its own under its own name.

use std::path::Path;

use glam::Mat4;

use super::Running;
use crate::io;
use crate::mesh::Mesh;
use crate::scene::Scene;

/// One object of an export: its name, its mesh, and the places it is written
/// (itself, then its live copies) with the names they go under.
struct Part<'a> {
    name: &'a str,
    mesh: &'a Mesh,
    places: Vec<(String, Mat4)>,
}

/// What Export FBX writes: the selected objects, in the list's order whatever
/// order they were picked in, and how many of them have no surface to write.
/// `slot` is `Doc::mesh`.
fn parts<'a>(scene: &'a Scene, slot: &'a Mesh) -> (Vec<Part<'a>>, usize) {
    let mut parts = Vec::new();
    let mut left_out = 0;
    for object in scene.objects.iter().filter(|o| scene.is_selected(o.id)) {
        match scene.mesh(object.mesh, slot).filter(|mesh| !mesh.indices.is_empty()) {
            Some(mesh) => parts.push(Part { name: &object.name, mesh, places: scene.export_places(object) }),
            None => left_out += 1,
        }
    }
    (parts, left_out)
}

/// The objects of an export as its status line names them: `"Ball"`,
/// `"Ball" and "Cube"`, and past three of them `"Ball", "Cube" and 3 more`.
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

/// The name the save box offers, less its time and ending: the object's, or
/// with several selected the active one's and how many go with it.
fn file_stem(scene: &Scene) -> String {
    let name = &scene.active().name;
    match scene.selected().len() - 1 {
        0 => name.clone(),
        others => format!("{name}_and_{others}_more"),
    }
}

impl Running {
    /// The Export FBX button: asks where to put the file, and writes it.
    /// (The save box waits for a person, so a scripted run does not press this.)
    pub(super) fn export_fbx_action(&mut self) {
        let file = io::export_file_name(&file_stem(&self.doc.scene), "fbx");
        let pick = rfd::FileDialog::new().add_filter("Autodesk FBX", &["fbx"]).set_file_name(file).save_file();
        if let Some(path) = pick {
            match self.export_fbx_to(&path) {
                Ok(text) => self.status(text, true),
                Err(e) => self.status(e, false),
            }
        }
    }

    /// Writes the file, and returns the line that says what went where, or
    /// why nothing was written.
    pub(super) fn export_fbx_to(&mut self, path: &Path) -> Result<String, String> {
        let scene = &self.doc.scene;
        let (parts, left_out) = parts(scene, &self.doc.mesh);
        if parts.is_empty() {
            let whose = if left_out > 1 { "no selected object has" } else { "this object has no" };
            return Err(format!("There is nothing to export: {whose} surface."));
        }
        // Each object's own place, so an instance exports as its own copy, and
        // its live copies, which are part of the model on screen.
        let places: Vec<Vec<(&str, Mat4)>> =
            parts.iter().map(|part| part.places.iter().map(|(n, m)| (n.as_str(), *m)).collect()).collect();
        let objects: Vec<(&Mesh, &[(&str, Mat4)])> =
            parts.iter().zip(&places).map(|(part, places)| (part.mesh, places.as_slice())).collect();
        let (result, mended) = Self::split_mended(io::export_fbx_painted(&objects, &self.export_paint(), path));
        result?;

        let names: Vec<&str> = parts.iter().map(|part| part.name).collect();
        let mut what = match names.as_slice() {
            [_] => format!("{}{}", named(&names), self.export_note()),
            _ => {
                let copies = if parts.iter().any(|part| part.places.len() > 1) { " with their live copies" } else { "" };
                format!("{} objects ({}){copies}", names.len(), named(&names))
            }
        };
        if parts.iter().any(|part| part.mesh.has_paint()) {
            what += ", paint included";
            if self.doc.opts.fbx_material_weights {
                what += " (material weights in UV sets 1 to 4)";
            }
            what += ",";
        }
        let left = match left_out {
            0 => String::new(),
            1 => ". 1 selected object has no surface and was left out".to_owned(),
            n => format!(". {n} selected objects have no surface and were left out"),
        };
        Ok(format!("Exported {what} to {}{mended}{left}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    #[test]
    fn an_export_takes_every_selected_object_in_the_lists_order_with_its_copies() {
        let mut slot = primitives::build(Primitive::Sphere, 1_000, 0.5);
        let mut scene = Scene::new("Ball");
        let ball = scene.active_id();
        let block = scene.add("Block", primitives::build(Primitive::Cube, 500, 0.5), &mut slot);
        let cone = scene.add("Tall Cone", primitives::build(Primitive::Cone, 500, 0.5), &mut slot);
        let empty = scene.add("Empty", Mesh::new(Vec::new(), Vec::new(), None), &mut slot);
        scene.get_mut(cone).unwrap().position = Vec3::new(3.0, 0.0, 0.0);
        scene.set_mirror(ball, 1, Vec3::ZERO);

        // One selected: that object alone, and the box offers its name.
        scene.select(block, &mut slot);
        let (one, left_out) = parts(&scene, &slot);
        assert_eq!((one.len(), left_out), (1, 0));
        assert_eq!((one[0].name, one[0].mesh.positions.len()), ("Block", slot.positions.len()));
        assert_eq!(file_stem(&scene), "Block");

        // Picked last to first: written in the list's order, the active object's mesh
        // from the slot and the others' from where the scene keeps them.
        scene.select(cone, &mut slot);
        scene.select_also(empty);
        scene.select_also(ball);
        let (several, left_out) = parts(&scene, &slot);
        assert_eq!(left_out, 1, "the one with no surface");
        assert_eq!(several.iter().map(|part| part.name).collect::<Vec<_>>(), ["Ball", "Tall Cone"]);
        let names = |part: &Part| part.places.iter().map(|place| place.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&several[0]), ["Ball", "Ball_MirrorX"]);
        assert_eq!(names(&several[1]), ["Tall_Cone"]);
        assert_eq!(several[1].places[0].1, scene.get(cone).unwrap().matrix());
        assert!(std::ptr::eq(several[1].mesh, &slot), "the active object's mesh is the one being sculpted");
        assert_ne!(several[0].mesh.positions.len(), several[1].mesh.positions.len());
        assert_eq!(file_stem(&scene), "Tall Cone_and_2_more");
    }

    #[test]
    fn the_status_line_names_what_went() {
        assert_eq!(named(&["Ball"]), "\"Ball\"");
        assert_eq!(named(&["Ball", "Block"]), "\"Ball\" and \"Block\"");
        assert_eq!(named(&["Ball", "Block", "Cone"]), "\"Ball\", \"Block\" and \"Cone\"");
        assert_eq!(named(&["Ball", "Block", "Cone", "Rod", "Ring"]), "\"Ball\", \"Block\" and 3 more");
    }
}
