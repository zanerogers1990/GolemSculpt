//! Export OBJ and Export PLY, and what every export is told of the scene's
//! paint (`io::ExportPaint`): the Base Colour unpainted surface goes out as,
//! the palette's names and swatches, and the FBX option for game shaders.
//! (Export FBX is `export_fbx.rs`, STL `print_check.rs`.)
//!
//! Each button asks where to put the file and then calls a `_to` routine
//! with the path, which is all the Claude bridge and the tests call: a
//! scripted run never opens the save box.

use std::path::Path;

use super::Running;
use crate::io;

impl Running {
    /// What an export writes of the scene's paint.
    pub(super) fn export_paint(&self) -> io::ExportPaint {
        let o = &self.doc.opts;
        let mut paint = io::ExportPaint::of(o.base_color, &o.paint);
        paint.weights_in_uvs = o.fbx_material_weights;
        paint
    }

    /// Words for a status line about an export of the active object: that its paint went with it.
    fn with_paint(&self) -> &'static str {
        if self.doc.mesh.has_paint() { ", paint included," } else { "" }
    }

    /// The Export OBJ button: asks where to put the file, and writes it.
    pub(super) fn export_obj_action(&mut self) {
        let file = io::export_file_name(&self.doc.scene.active().name, "obj");
        let pick = rfd::FileDialog::new().add_filter("Wavefront OBJ", &["obj"]).set_file_name(file).save_file();
        if let Some(path) = pick {
            match self.export_obj_to(&path) {
                Ok(text) => self.status(text, true),
                Err(e) => self.status(e, false),
            }
        }
    }

    /// Writes the active object as an OBJ: its own place, so an instance
    /// exports as its own copy, and its live mirror copies, which are part of
    /// the model on screen. A painted object's vertices carry their colour.
    pub(super) fn export_obj_to(&mut self, path: &Path) -> Result<String, String> {
        let name = self.doc.scene.active().name.clone();
        let places = self.export_places();
        let places: Vec<(&str, glam::Mat4)> = places.iter().map(|(n, m)| (n.as_str(), *m)).collect();
        // Without the points and loose vertices dynamic topology's merges leave.
        let tidy = crate::scene::tidied(&self.doc.mesh);
        let written = io::export_obj_painted(tidy.as_ref().unwrap_or(&self.doc.mesh), &places, &self.export_paint(), path);
        let (result, mended) = Self::split_mended(written);
        result?;
        Ok(format!("Exported \"{name}\"{}{} to {}{mended}", self.export_note(), self.with_paint(), path.display()))
    }

    /// The Export PLY button: asks where to put the file, and writes it.
    pub(super) fn export_ply_action(&mut self) {
        let file = io::export_file_name(&self.doc.scene.active().name, "ply");
        let pick = rfd::FileDialog::new().add_filter("PLY", &["ply"]).set_file_name(file).save_file();
        if let Some(path) = pick {
            match self.export_ply_to(&path) {
                Ok(text) => self.status(text, true),
                Err(e) => self.status(e, false),
            }
        }
    }

    /// Writes the active object, where it stands and with its live copies,
    /// as a binary PLY (`io/ply.rs`): with its colour and material weights
    /// when it is painted.
    pub(super) fn export_ply_to(&mut self, path: &Path) -> Result<String, String> {
        let name = self.doc.scene.active().name.clone();
        let models: Vec<glam::Mat4> = self.export_places().into_iter().map(|(_, m)| m).collect();
        let tidy = crate::scene::tidied(&self.doc.mesh);
        let written = io::export_ply_placed(tidy.as_ref().unwrap_or(&self.doc.mesh), &models, &self.export_paint(), path);
        let (result, mended) = Self::split_mended(written);
        result?;
        Ok(format!("Exported \"{name}\"{}{} to {}{mended}", self.export_note(), self.with_paint(), path.display()))
    }
}
