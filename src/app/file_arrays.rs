//! A check, in the running app, that arrays go through a file: the Array tool
//! holds some (`--demo-tool array:several` makes three), the scene is saved
//! as a `.sclp`, opened, undone, redone and added, and each step prints what
//! came of it. The file code itself is `app/tools/files.rs` and `io/tool_data.rs`,
//! with their own tests; this is the part only the running app can run:
//!
//! ```text
//! sculpt.exe --screenshot out.png --demo-tool array:several --demo-files array
//! ```
//!
//! The picture is the reopened scene, with the Array tool still up. What it
//! writes goes under the temp folder.

use std::path::Path;

use sculpt_array::{ArrayState, state_from_saved};

use super::Running;
use crate::doc::Action;
use crate::io::{self, Details};

impl Running {
    /// The array the tools hold for each object of the scene, row by row, as a file would
    /// keep it.
    pub(super) fn kept_arrays(&self) -> Vec<Option<ArrayState>> {
        let mut details = Details::default();
        self.doc.tools.keep_in_file(&self.doc.scene, &mut details);
        (0..self.doc.scene.objects.len())
            .map(|row| details.get_tool_object(row, io::slot::ARRAY).and_then(|saved| state_from_saved(&saved)))
            .collect()
    }

    /// With arrays made (`--demo-tool array:several`): saves the scene, opens the file,
    /// undoes and redoes the open, adds the file to the scene, and prints a line per check.
    pub(super) fn arrays_play(&mut self, folder: &Path) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        // The camera is still gliding to where the demo sent it: settle it, so the file keeps that view.
        self.doc.camera.update(10.0);
        self.sync_view();
        self.tools_frame();
        let arrays = self.kept_arrays();
        let rows: Vec<usize> = (0..arrays.len()).filter(|&row| arrays[row].is_some()).collect();
        println!("     {} objects, arrays on rows {rows:?}", arrays.len());
        say("the Array tool holds arrays", !rows.is_empty());
        if arrays.len() > 1 {
            say("on an object that is not the first too", rows.iter().any(|&row| row > 0));
        }
        let kinds = |arrays: &[Option<ArrayState>]| -> Vec<String> {
            arrays.iter().flatten().map(|a| format!("{:?} x{}", a.settings.kind, a.settings.count)).collect()
        };
        println!("     {}", kinds(&arrays).join(", "));
        let old_ids: Vec<u64> = self.doc.scene.objects.iter().map(|o| o.id).collect();

        // Save: the arrays are in the file, on the rows of their objects.
        let path = folder.join("arrays.sclp");
        let saved = self.save_scene(&path);
        say("the scene saves", saved.is_ok() && path.is_file());
        self.doc.path = Some(path.clone());
        let file = io::load_scene(&path);
        let in_file: Vec<usize> = file.as_ref().map_or(Vec::new(), |scene| {
            (0..scene.objects.len())
                .filter(|&row| scene.details.get_tool_object(row, io::slot::ARRAY).is_some())
                .collect()
        });
        say("the file has an array on each of those rows and no other", in_file == rows);

        // Open: new objects, the same arrays on the same rows, live.
        self.load(path.clone(), true);
        let now: Vec<u64> = self.doc.scene.objects.iter().map(|o| o.id).collect();
        say(
            "opening it gives the scene new objects",
            now.len() == old_ids.len() && now.iter().all(|id| !old_ids.contains(id)),
        );
        let reopened = self.kept_arrays();
        say("with the same arrays on the same rows", reopened == arrays);
        say("the opened file is the document, and clean", !self.unsaved());
        say("and the tool has no steps of its own left to answer Z before the open", !self.doc.tools.can_undo());

        // Undo: the scene as it was, with the arrays it had; redo: the file's again.
        self.act(Action::Undo);
        let undone: Vec<u64> = self.doc.scene.objects.iter().map(|o| o.id).collect();
        say("undoing the open brings the old objects back", undone == old_ids);
        say("with the arrays they had", self.kept_arrays() == arrays);
        self.act(Action::Redo);
        say(
            "redoing it brings the file's objects and their arrays",
            self.kept_arrays() == arrays && self.doc.scene.objects.len() == now.len(),
        );

        // Add: the file's objects join the scene, arrays and all; the scene's own are untouched.
        self.load(path.clone(), false);
        let both = self.kept_arrays();
        let count = arrays.len();
        say("adding the file doubles the objects", both.len() == 2 * count);
        say("and the scene's own arrays are as they were", both[..count] == arrays[..]);
        say("the added objects have the file's arrays", both[count..] == arrays[..]);
        self.act(Action::Undo);
        say("one undo takes the added objects and their arrays away again", self.kept_arrays() == arrays);

        // What an export of the selected object writes: it and every copy of its array.
        // (The tool says its copies in its frame, which has not come round yet in a run like this.)
        self.sync_view();
        self.tools_frame();
        let places = self.export_places();
        let selected = self.kept_arrays()[self.doc.scene.index_of(self.doc.scene.active_id()).unwrap_or(0)].clone();
        let sides = 1 + self.doc.scene.active().mirror.map_or(0, |m| m.copy_count());
        let wanted = selected.map_or(sides, |a| a.settings.count as usize * sides);
        println!("     the selected object exports as {} places{}", places.len(), self.export_note());
        say("an export writes the object, its mirror and every copy of its array", places.len() == wanted);
        let obj = folder.join("arrays.obj");
        let named: Vec<(&str, glam::Mat4)> = places.iter().map(|(n, m)| (n.as_str(), *m)).collect();
        let written = io::export_obj_placed(&self.doc.mesh, &named, &obj);
        let blocks =
            std::fs::read_to_string(&obj).map_or(0, |text| text.lines().filter(|l| l.starts_with("o ")).count());
        say("as one block each in the OBJ", written.is_ok() && blocks == places.len());
        let _ = std::fs::remove_file(&obj);

        // Leave the file open, as the picture is of it: its arrays on screen with the tool up.
        self.load(path, true);
        self.doc.camera.update(10.0);
        self.sync_view();
        say("opened again for the picture, with every array on its own object", self.kept_arrays() == arrays);
    }
}
