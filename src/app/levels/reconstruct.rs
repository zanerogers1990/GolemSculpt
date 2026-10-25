//! Reconstruct Subdiv on the app's side: the Subdivision card's button (and
//! the bridge's `reconstruct_subdiv`), for a model that came in as the
//! finest level of a mesh subdivided in ZBrush or Blender. What it does to
//! the mesh is `mesh/levels/reconstruct.rs`.
//!
//! Like Subdivide it is one undo step that keeps the old mesh whole, and it
//! leaves the object on the level it was already showing, the finest, so
//! nothing on screen moves: Page Down is what shows the levels found.

use std::time::Instant;

use crate::app::Running;
use crate::mesh::levels;
use crate::printable::grouped;

impl Running {
    /// Finds the levels below the active object, a quad model that was made
    /// by subdividing, and keeps it on the finest of them.
    pub(in crate::app) fn reconstruct_subdiv(&mut self) {
        let started = Instant::now();
        // Without what dynamic topology's merges left: levels number every vertex.
        let tidy = crate::scene::tidied(&self.doc.mesh);
        let (mesh, found) = match levels::reconstructed(tidy.as_ref().unwrap_or(&self.doc.mesh)) {
            Ok(made) => made,
            Err(why) => return self.status(why, false),
        };
        let name = self.doc.scene.active().name.clone();
        let count = found.below + 1;
        let lower = if found.below == 1 { "level" } else { "levels" };
        let faces = mesh.polys.as_ref().map_or(0, |polys| polys.start.len().saturating_sub(1));
        let text = format!(
            "Rebuilt {} lower {lower} of \"{name}\": level 1 has {} faces and level {count}, the one it is on, {} \
             ({:.0} ms). Page Down steps down.",
            found.below,
            grouped(found.coarsest_faces),
            grouped(faces),
            started.elapsed().as_secs_f64() * 1000.0
        );
        self.replace_mesh(mesh, true);
        self.levels_seen();
        self.toast("Reconstruct Subdiv");
        self.status(text, true);
    }
}
