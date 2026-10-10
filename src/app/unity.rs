//! A check, in the running app, of what opening a sculpt from the Unity app
//! leads to: that saving goes to a `.sclp` and nowhere near the Unity file,
//! and that the `.sclp` then holds the same scene. The reader itself is
//! `unity_import.rs`, with its own tests; this is the app's side of it, which
//! only the app can run:
//!
//! ```text
//! sculpt.exe --screenshot out.png --demo-files unity some.sculpt
//! ```
//!
//! It prints a line per check, as `--demo-files play` does. The `.sclp` it
//! saves goes under the temp folder, and is removed again.

use std::hash::{Hash, Hasher};
use std::path::Path;

use rayon::prelude::*;

use super::Running;
use crate::camera::SavedView;
use crate::doc::Action;
use crate::io;
use crate::mesh::Mesh;
use crate::unity_import;

/// One number for a mesh that does not depend on the order of its vertices
/// or triangles (building a mesh chooses its own) and changes if any of them
/// does: every vertex by its position and mask, every triangle by where its
/// corners are, which way round, and whether it is hidden.
fn mesh_print(mesh: &Mesh) -> (usize, usize, usize, u64) {
    let hashed = |value: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value(&mut hasher);
        hasher.finish()
    };
    let bits = |v: u32| mesh.positions[v as usize].to_array().map(|c| (c + 0.0).to_bits());
    let vertices = (0..mesh.vertex_count() as u32)
        .into_par_iter()
        .map(|v| hashed(&|h| (bits(v), mesh.mask[v as usize].to_bits()).hash(h)))
        .reduce(|| 0, u64::wrapping_add);
    let triangles = mesh
        .indices
        .par_chunks_exact(3)
        .enumerate()
        .map(|(t, corners)| {
            let mut corners = [bits(corners[0]), bits(corners[1]), bits(corners[2])];
            // Turned so the least corner comes first: the same triangle, the same side up.
            let least = (0..3).min_by_key(|&k| corners[k]).unwrap_or(0);
            corners.rotate_left(least);
            hashed(&|h| (corners, mesh.is_triangle_hidden(t as u32)).hash(h))
        })
        .reduce(|| 0, u64::wrapping_add);
    (
        mesh.vertex_count(),
        mesh.triangle_count(),
        mesh.hidden_triangle_count(),
        vertices.wrapping_mul(31).wrapping_add(triangles),
    )
}

/// Everything a file is meant to keep, as the app holds it.
#[derive(PartialEq, Debug)]
struct Kept {
    /// Name, visibility, placement, which mesh (by the row that first shows
    /// it) and live mirror of each object, in list order.
    objects: Vec<String>,
    active: usize,
    meshes: Vec<(usize, usize, usize, u64)>,
    look: String,
    shading: io::Shading,
    view: SavedView,
    symmetry: String,
}

impl Running {
    fn kept(&self) -> Kept {
        let scene = &self.doc.scene;
        let mut first_rows: Vec<(u64, usize)> = Vec::new();
        let mut objects = Vec::new();
        let mut meshes = Vec::new();
        for (row, o) in scene.objects.iter().enumerate() {
            let first = match first_rows.iter().find(|(mesh, _)| *mesh == o.mesh) {
                Some(&(_, first)) => first,
                None => {
                    first_rows.push((o.mesh, row));
                    meshes.push(scene.mesh(o.mesh, &self.doc.mesh).map_or((0, 0, 0, 0), mesh_print));
                    row
                }
            };
            objects.push(format!(
                "{:?} {} {:?} {:?} {:?} mesh of row {first} {:?}",
                o.name, o.visible, o.position, o.rotation, o.scale, o.mirror
            ));
        }
        let opts = &self.doc.opts;
        // Every object's own, in list order, and whether the guides show.
        let symmetry: Vec<_> = scene.objects.iter().map(|o| scene.symmetry(o.id)).collect();
        let matcap =
            self.ui.assets.matcaps.get(opts.matcap).map_or(String::new(), |m| format!("{}/{}", m.category, m.name));
        Kept {
            objects,
            active: scene.index_of(scene.active_id()).unwrap_or(0),
            meshes,
            look: serde_json::to_string(&opts.look).unwrap_or_default(),
            shading: io::Shading::of(opts, matcap),
            view: self.doc.camera.saved_view(),
            symmetry: format!("{symmetry:?} guides {}", opts.symmetry_guides),
        }
    }

    /// With a Unity sculpt opened from the command line: saves it as the app
    /// would, opens what was saved, and prints what came of each step.
    pub(super) fn unity_play(&mut self, folder: &Path) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let Some(sculpt) = self.doc.path.clone().filter(|path| unity_import::is_unity(path)) else {
            println!("FAIL no Unity sculpt was opened: name a .sculpt file on the command line");
            return;
        };
        let stamp = |path: &Path| std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()));
        let untouched = stamp(&sculpt);
        println!("     {}", self.ui.status.0);
        say("the Unity sculpt is the document, with nothing unsaved", !self.unsaved());
        let before = self.kept();
        println!(
            "     {} objects, {} meshes, {} triangles",
            before.objects.len(),
            before.meshes.len(),
            before.meshes.iter().map(|m| m.1).sum::<usize>()
        );

        // Saving to the Unity file itself is refused, and leaves it alone.
        let refused = self.save_scene(&sculpt);
        say("a save aimed at the Unity file is refused", refused.as_ref().is_err_and(|e| e.contains(".sclp")));
        say(
            "and leaves it exactly as it was",
            stamp(&sculpt) == untouched && !sculpt.with_extension("sculpt.tmp").exists(),
        );

        // What Save does instead: the same name as a .sclp.
        let saved_to = folder.join(unity_import::save_name(&sculpt));
        let saved = self.save_scene(&saved_to);
        say(
            "saving as a .sclp of the same name works",
            saved.is_ok() && saved_to.is_file() && !unity_import::is_unity(&saved_to),
        );
        if saved.is_err() {
            return;
        }
        self.doc.path = Some(saved_to.clone());
        say("the scene is then saved, and still what was opened", !self.unsaved() && self.kept() == before);

        // Disturb what a file keeps, then open the .sclp: all of it is back.
        self.doc.camera.yaw += 77.0;
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [true, true, true]);
        self.doc.opts.look.brightness = 0.123;
        self.load(saved_to.clone(), true);
        let after = self.kept();
        say("the .sclp opens", self.doc.path.as_deref() == Some(saved_to.as_path()) && !self.unsaved());
        say(
            "with the same objects, in the same places, linked and mirrored the same",
            after.objects == before.objects && after.active == before.active,
        );
        say("the same meshes, masks and hidden faces, to the bit", after.meshes == before.meshes);
        say(
            "the same material, finish, lighting and effects",
            after.look == before.look && after.shading == before.shading,
        );
        say("the same view", after.view == before.view);
        say("every object's own mirror axes, radial symmetry and symmetry space", after.symmetry == before.symmetry);
        let scene = &self.doc.scene;
        for o in &scene.objects {
            println!("     symmetry of {:?}: {}", o.name, scene.symmetry(o.id).summary());
        }
        if after != before {
            println!("     before: {:?}\n     after:  {:?}", before.objects, after.objects);
        }

        // A sculpt that cannot be read changes nothing: the first few
        // thousand bytes of this one, as they are and stamped as a newer format.
        let mut head = Vec::new();
        if let Ok(file) = std::fs::File::open(&sculpt) {
            use std::io::Read;
            let _ = file.take(4096).read_to_end(&mut head);
        }
        for (what, version, says) in
            [("cut short", None, "is damaged"), ("from a newer Unity format", Some(3i32), "newer version")]
        {
            let broken = folder.join(format!("{what}.sculpt"));
            if let Some(version) = version.filter(|_| head.len() >= 12) {
                head[8..12].copy_from_slice(&version.to_le_bytes());
            }
            let written = std::fs::write(&broken, &head);
            self.load(broken, true);
            let refused = written.is_ok() && self.ui.status.0.contains(says);
            let unchanged =
                self.kept() == after && self.doc.path.as_deref() == Some(saved_to.as_path()) && !self.unsaved();
            say(&format!("a sculpt {what} is refused in words, and the scene stays as it was"), refused && unchanged);
        }

        // Import: the sculpt's objects join the scene, and nothing else of it is used.
        let count = self.doc.scene.objects.len();
        self.doc.opts.look.brightness = 0.123;
        self.load(sculpt.clone(), false);
        let objects = &self.doc.scene.objects;
        say("adding the Unity sculpt brings its objects in beside the others", objects.len() == 2 * count);
        if objects.len() == 2 * count {
            let (own, added) = objects.split_at(count);
            let scene = &self.doc.scene;
            let placed = own.iter().zip(added).all(|(a, b)| {
                (a.visible, a.position, a.rotation, a.scale, a.mirror, scene.symmetry(a.id))
                    == (b.visible, b.position, b.rotation, b.scale, b.mirror, scene.symmetry(b.id))
            });
            // Rows that shared a mesh still share one, and it is a new one.
            let linked = (0..count)
                .all(|i| (0..count).all(|j| (own[i].mesh == own[j].mesh) == (added[i].mesh == added[j].mesh)))
                && added.iter().all(|a| own.iter().all(|o| o.mesh != a.mesh));
            let mut names: Vec<&str> = objects.iter().map(|o| o.name.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            say(
                "where they were, linked, mirrored and each with its own symmetry as they were, under names of their own",
                placed && linked && names.len() == 2 * count,
            );
        }
        say(
            "the document is still the .sclp, now with unsaved work, and its look stayed",
            self.doc.path.as_deref() == Some(saved_to.as_path())
                && self.unsaved()
                && self.doc.opts.look.brightness == 0.123,
        );
        println!("     {}", self.ui.status.0);
        self.act(Action::Undo);
        say("one undo takes them out again", self.doc.scene.objects.len() == count);

        say("and the Unity sculpt was only ever read", stamp(&sculpt) == untouched);
        let _ = std::fs::remove_file(&saved_to);
    }
}
