//! Export FBX: the selected object, where it stands and with its live
//! copies, as an Autodesk FBX for Blender, Unity and the like (`io/fbx.rs`).

use std::path::Path;

use super::Running;
use crate::io;

impl Running {
    /// The Export FBX button: asks where to put the file, and writes it.
    /// (The save box waits for a person, so a scripted run does not press this.)
    pub(super) fn export_fbx_action(&mut self) {
        let file = io::export_file_name(&self.doc.scene.active().name, "fbx");
        let pick = rfd::FileDialog::new().add_filter("Autodesk FBX", &["fbx"]).set_file_name(file).save_file();
        if let Some(path) = pick {
            match self.export_fbx_to(&path) {
                Ok(text) => self.status(text, true),
                Err(e) => self.status(e, false),
            }
        }
    }

    /// Writes the file, and returns the line that says where it went, or why
    /// nothing was written.
    pub(super) fn export_fbx_to(&mut self, path: &Path) -> Result<String, String> {
        let name = self.doc.scene.active().name.clone();
        // The active object's own place, so an instance exports as its own copy,
        // and its live copies, which are part of the model on screen.
        let places = self.export_places();
        let places: Vec<(&str, glam::Mat4)> = places.iter().map(|(n, m)| (n.as_str(), *m)).collect();
        let (result, mended) = Self::split_mended(io::export_fbx_placed(&self.doc.mesh, &places, path));
        result?;
        Ok(format!("Exported \"{name}\"{} to {}{mended}", self.export_note(), path.display()))
    }
}
