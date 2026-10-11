//! Hosted tools in `.sclp` files: what a tool keeps for an object goes into
//! that object's entry of the file, and comes back to the same object.
//!
//! The tools key what they hold by `ObjectId`, which is the scene's and lasts
//! a session, while a file keeps objects by their row in the list. This is the
//! one place the two meet: on a save each tool is asked about each object (by
//! id) and the answer goes under the object's row; on a load the rows of the
//! file are matched to the ids the objects were given, and each tool is handed
//! what it kept for them. The file code (`io::Details`) and the toolkit learn
//! nothing about what is inside; a tool's id is the name its data goes under.

use std::hash::{Hash, Hasher};

use sculpt_toolkit::{Arrival, Loss, ObjectId, Saved};

use super::Tools;
use crate::io::Details;
use crate::scene::Scene;

impl Tools {
    /// Puts what every tool keeps for each object of `scene` in `details`, under
    /// the object's row and the tool's id.
    pub fn keep_in_file(&self, scene: &Scene, details: &mut Details) {
        for (info, slot) in self.infos.iter().zip(&self.slots) {
            let Some(tool) = slot else { continue };
            for (row, object) in scene.objects.iter().enumerate() {
                if let Some(saved) = tool.save_object(ObjectId(object.id)) {
                    details.set_tool_object(row, info.id, &saved);
                }
            }
        }
    }

    /// Hands every tool what a file kept for its objects. `rows` pairs a row of
    /// the file with the id its object has in the scene; a row that did not
    /// become an object is left out, and a tool is told even when the file kept
    /// nothing of its own, since a file that replaced the scene ends what it was doing
    /// (`Tool::load_objects`).
    pub fn restore_from_file(&mut self, rows: &[(usize, u64)], details: &Details, arrival: Arrival) {
        for (info, slot) in self.infos.iter().zip(&mut self.slots) {
            let Some(tool) = slot else { continue };
            let objects = rows
                .iter()
                .filter_map(|&(row, id)| Some((ObjectId(id), details.get_tool_object(row, info.id)?)))
                .collect();
            tool.load_objects(objects, arrival);
        }
    }

    /// Feeds `hasher` what every tool would keep in a file for the object with
    /// this id (what `keep_in_file` would write for it), for the fingerprint
    /// that tells a scene has changed in a way that is not an undo step
    /// (app/unsaved.rs): an array reshaped, say.
    pub fn hash_kept(&self, object: u64, hasher: &mut impl Hasher) {
        for (index, slot) in self.slots.iter().enumerate() {
            if let Some(saved) = slot.as_ref().and_then(|tool| tool.save_object(ObjectId(object))) {
                index.hash(hasher);
                hash_saved(&saved, hasher);
            }
        }
        // Where this object's part ends, so that what one object keeps is
        // never read as the next one's.
        usize::MAX.hash(hasher);
    }

    /// What each tool holds that is in no file and that `loss` would cost
    /// (`Tool::scaffold_at_risk`), in the order of the tools' buttons.
    pub fn scaffolds_at_risk(&self, loss: Loss) -> Vec<String> {
        self.slots.iter().flatten().filter_map(|tool| tool.scaffold_at_risk(loss)).collect()
    }

    /// The rows of the scene as the file just read became its objects: every
    /// object, row by row. For a file that replaced the scene.
    pub fn rows_of(scene: &Scene) -> Vec<(usize, u64)> {
        scene.objects.iter().enumerate().map(|(row, o)| (row, o.id)).collect()
    }
}

/// A saved value into a hasher, kind and all: `[1, 2]` and `"12"` are not the same.
fn hash_saved(saved: &Saved, hasher: &mut impl Hasher) {
    std::mem::discriminant(saved).hash(hasher);
    match saved {
        Saved::Null => {}
        Saved::Bool(value) => value.hash(hasher),
        Saved::Number(value) => value.to_bits().hash(hasher),
        Saved::Text(text) => text.hash(hasher),
        Saved::List(items) => {
            items.len().hash(hasher);
            items.iter().for_each(|item| hash_saved(item, hasher));
        }
        Saved::Fields(fields) => {
            fields.len().hash(hasher);
            for (name, value) in fields {
                name.hash(hasher);
                hash_saved(value, hasher);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use glam::{Quat, Vec3};
    use sculpt_array::{ArrayKind, ArraySettings, ArrayState};
    use super::*;
    use crate::app::install;
    use crate::io::{self, SavedObject, slot};
    use crate::mesh::Mesh;
    use crate::mesh::primitives::{self, Primitive};

    fn ball() -> Mesh {
        primitives::build(Primitive::Sphere, 400, 0.5)
    }

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join("sculpt_tool_files_test").join(name);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// A scene of `count` balls, each its own mesh.
    fn scene_of(count: usize) -> (Scene, Mesh) {
        let mut slot = ball();
        let mut scene = Scene::new("Ball 1");
        for i in 1..count {
            scene.add(&format!("Ball {}", i + 1), ball(), &mut slot);
        }
        (scene, slot)
    }

    /// What `Running::save_scene` writes, without a window.
    fn save(scene: &Scene, slot: &Mesh, tools: &Tools, path: &Path) {
        let mut ids: Vec<u64> = Vec::new();
        let objects: Vec<SavedObject> = scene
            .objects
            .iter()
            .map(|o| {
                let mesh = ids.iter().position(|&id| id == o.mesh).unwrap_or_else(|| {
                    ids.push(o.mesh);
                    ids.len() - 1
                });
                SavedObject {
                    name: o.name.clone(),
                    visible: o.visible,
                    mesh,
                    position: o.position,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                }
            })
            .collect();
        let meshes: Vec<&Mesh> = ids.iter().map(|&id| scene.mesh(id, slot).unwrap()).collect();
        let mut details = Details::default();
        scene.keep_mirrors(&mut details);
        tools.keep_in_file(scene, &mut details);
        io::save_scene_with(&meshes, &objects, scene.index_of(scene.active_id()).unwrap(), &details, path).unwrap();
    }

    /// What opening a file does: `install`, then the tools are told.
    fn open(path: &Path, tools: &mut Tools) -> Scene {
        let saved = io::load_scene(path).unwrap();
        let (mut scene, mut slot) = (Scene::new("Placeholder"), ball());
        let details = install(&mut scene, &mut slot, saved);
        tools.restore_from_file(&Tools::rows_of(&scene), &details, Arrival::Replaced);
        scene
    }

    fn array(kind: ArrayKind, count: i32) -> ArrayState {
        let mut settings = ArraySettings::new(kind);
        settings.count = count;
        settings.sweep = if kind == ArrayKind::Radial { -200.0 } else { 360.0 };
        ArrayState { settings, points: vec![Vec3::ZERO, Vec3::new(-2.0, 0.5, 1.0), Vec3::new(1.0, 1.0, 1.0)] }
    }

    /// Gives the tools these arrays, as a file would, by the ids of `scene`'s rows.
    fn give(tools: &mut Tools, scene: &Scene, arrays: &[(usize, ArrayState)]) {
        let mut details = Details::default();
        for (row, state) in arrays {
            details.set_tool_object(*row, "array", &sculpt_array::state_to_saved(state));
        }
        tools.restore_from_file(&Tools::rows_of(scene), &details, Arrival::Added);
    }

    fn kept(tools: &Tools, scene: &Scene) -> Vec<Option<ArrayState>> {
        let mut details = Details::default();
        tools.keep_in_file(scene, &mut details);
        (0..scene.objects.len())
            .map(|row| details.get_tool_object(row, "array").and_then(|saved| sculpt_array::state_from_saved(&saved)))
            .collect()
    }

    #[test]
    fn the_array_tools_id_is_the_name_the_file_format_has_spoken_for() {
        let tools = Tools::default();
        assert!(tools.info_index(slot::ARRAY).is_some(), "an array is kept under {:?}", slot::ARRAY);
    }

    #[test]
    fn arrays_are_written_under_their_objects_rows_and_read_back_onto_the_objects_of_those_rows() {
        let dir = folder("rows");
        let (scene, slot) = scene_of(4);
        let mut tools = Tools::default();
        let arrays = [(1, array(ArrayKind::Radial, 6)), (3, array(ArrayKind::Curve, 9))];
        give(&mut tools, &scene, &arrays);
        let before = kept(&tools, &scene);
        assert_eq!(before.iter().map(Option::is_some).collect::<Vec<_>>(), [false, true, false, true]);

        let path = dir.join("arrays.sclp");
        save(&scene, &slot, &tools, &path);

        // The JSON has them on the rows' own entries and nowhere else.
        let loaded = io::load_scene(&path).unwrap();
        let rows: Vec<bool> = (0..4).map(|row| loaded.details.get_tool_object(row, "array").is_some()).collect();
        assert_eq!(rows, [false, true, false, true]);

        // A new session's tools, a new scene with new ids: the same arrays on the same rows.
        let mut fresh = Tools::default();
        let opened = open(&path, &mut fresh);
        assert_eq!(kept(&fresh, &opened), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The bytes of the JSON section of a `.sclp`, found the way any reader finds it: past the
    /// magic, block by block (a tag, a length, the body).
    fn json_of(path: &Path) -> serde_json::Value {
        let bytes = std::fs::read(path).unwrap();
        let mut at = 8;
        while at + 12 <= bytes.len() {
            let length = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
            if &bytes[at..at + 4] == b"INFO" {
                return serde_json::from_slice(&bytes[at + 12..at + 12 + length]).unwrap();
            }
            at += 12 + length;
        }
        panic!("no INFO block");
    }

    #[test]
    fn a_file_with_arrays_differs_from_one_without_only_by_the_added_field() {
        let dir = folder("additive");
        let (scene, slot) = scene_of(3);
        let (none, mut with) = (Tools::default(), Tools::default());
        give(&mut with, &scene, &[(0, array(ArrayKind::Line, 4)), (2, array(ArrayKind::Curve, 6))]);
        let (plain, arrayed) = (dir.join("plain.sclp"), dir.join("arrayed.sclp"));
        save(&scene, &slot, &none, &plain);
        save(&scene, &slot, &with, &arrayed);

        // Every block but the last (the JSON) is byte for byte the same file.
        let (a, b) = (std::fs::read(&plain).unwrap(), std::fs::read(&arrayed).unwrap());
        let info_at = |bytes: &[u8]| bytes.windows(4).rposition(|w| w == b"INFO").unwrap();
        assert_eq!(a[..info_at(&a)], b[..info_at(&b)], "magic and meshes");

        // And the JSON is the same once the added field is taken away: a reader that steps over fields
        // it does not know (the notes above SCENE_MAGIC in io.rs) reads it as it read a file without.
        let (mut old, mut new) = (json_of(&plain), json_of(&arrayed));
        let entries = new["objects"].as_array().unwrap();
        assert_eq!(entries.iter().map(|e| e.get("array").is_some()).collect::<Vec<_>>(), [true, false, true]);
        for json in [&mut old, &mut new] {
            json.as_object_mut().unwrap().remove("saved");
            for entry in json["objects"].as_array_mut().unwrap() {
                entry.as_object_mut().unwrap().remove("array");
            }
        }
        assert_eq!(old, new);

        // As the app reads it, too: the same objects, in the same places.
        let (old, new) = (io::load_scene(&plain).unwrap(), io::load_scene(&arrayed).unwrap());
        assert_eq!(old.objects.len(), new.objects.len());
        assert!(
            old.objects
                .iter()
                .zip(&new.objects)
                .all(|(a, b)| (&a.name, a.mesh, a.position) == (&b.name, b.mesh, b.position))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_saved_before_arrays_opens_with_none() {
        let dir = folder("older");
        let (scene, slot) = scene_of(2);
        let path = dir.join("old.sclp");
        save(&scene, &slot, &Tools::default(), &path);
        let mut tools = Tools::default();
        give(&mut tools, &scene, &[(0, array(ArrayKind::Line, 4))]);
        let opened = open(&path, &mut tools);
        assert_eq!(opened.objects.len(), 2);
        assert!(kept(&tools, &opened).iter().all(Option::is_none), "no array appears from a file that has none");
        assert!(io::load_scene(&path).unwrap().details.get_tool_object(0, "array").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hostile_array_entry_costs_the_array_and_nothing_else() {
        let (scene, _) = scene_of(3);
        let mut details = Details::default();
        let wild = |kind: i32, count: f64| {
            Saved::fields([
                ("kind", kind.into()),
                ("count", Saved::Number(count)),
                ("points", Saved::List(vec![Saved::numbers(&[0.0; 3]), Saved::numbers(&[f32::NAN, 0.0, 0.0])])),
            ])
        };
        details.set_tool_object(0, "array", &wild(1, 5.0));
        details.set_tool_object(1, "array", &Saved::from("nonsense"));
        details.set_tool_object(2, "array", &sculpt_array::state_to_saved(&array(ArrayKind::Line, 1_000_000)));
        // A row past the end of the object list does not exist: nothing to give it to.
        details.set_tool_object(40, "array", &sculpt_array::state_to_saved(&array(ArrayKind::Line, 3)));
        let mut tools = Tools::default();
        tools.restore_from_file(&Tools::rows_of(&scene), &details, Arrival::Replaced);
        let read = kept(&tools, &scene);
        assert!(read[0].is_none() && read[1].is_none());
        assert_eq!(read[2].as_ref().map(|a| a.settings.count), Some(ArraySettings::MAX_COUNT));
        assert_eq!(read.len(), 3);
    }

    #[test]
    fn an_array_on_an_instance_is_kept_on_the_instances_row_and_the_instance_stays_linked() {
        let dir = folder("instance");
        let (mut scene, mut slot) = scene_of(2);
        let first = scene.objects[0].id;
        scene.select(first, &mut slot);
        let instance = scene.instance_active();
        let row = scene.index_of(instance).unwrap();
        assert!(row > 0 && scene.objects[row].mesh == scene.objects[0].mesh, "an instance of the first object");

        // The Unity app never put an array on an instance; this app may, and so may a file.
        let mut tools = Tools::default();
        give(&mut tools, &scene, &[(row, array(ArrayKind::Line, 4))]);
        let path = dir.join("instance.sclp");
        save(&scene, &slot, &tools, &path);

        let mut fresh = Tools::default();
        let opened = open(&path, &mut fresh);
        let arrays = kept(&fresh, &opened);
        assert_eq!(
            arrays.iter().map(Option::is_some).collect::<Vec<_>>(),
            kept(&tools, &scene).iter().map(Option::is_some).collect::<Vec<_>>()
        );
        assert!(arrays[row].is_some() && arrays[0].is_none(), "on the instance, not on the object it shows");
        assert_eq!(opened.objects[row].mesh, opened.objects[0].mesh, "still linked");

        // Added to a scene, the instance and its array come in together.
        let added = scene_of(1).0;
        let (mut target, mut slot) = (added, ball());
        let mut tools = Tools::default();
        let loaded = io::load_scene(&path).unwrap();
        let (details, count, before) = (loaded.details.clone(), loaded.objects.len(), target.objects.len());
        let rows = crate::app::files::append(&mut target, &mut slot, loaded);
        assert_eq!(rows.len(), count);
        tools.restore_from_file(&rows, &details, Arrival::Added);
        let now = kept(&tools, &target);
        assert_eq!(now.len(), before + count);
        assert_eq!(now.iter().flatten().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tool with work in hand that is in no file: what `scaffold_at_risk` is for.
    struct Scaffolded(&'static str);

    impl sculpt_toolkit::Tool for Scaffolded {
        fn info(&self) -> sculpt_toolkit::Info {
            sculpt_toolkit::Info { id: "scaffolded", name: "Scaffolded", icon: "prim_cube", tip: "", hotkey: None }
        }

        fn scaffold_at_risk(&self, loss: Loss) -> Option<String> {
            // Work that stands in the world: only closing loses it.
            (loss == Loss::Closing || self.0.contains("on the model")).then(|| self.0.to_owned())
        }
    }

    #[test]
    fn the_host_gathers_what_every_tool_says_it_would_lose_in_the_order_of_their_buttons() {
        let mut tools = Tools::default();
        for loss in [Loss::Closing, Loss::SceneReplaced] {
            assert!(tools.scaffolds_at_risk(loss).is_empty(), "the tools as they start hold nothing");
        }
        // Two tools with work, one before and one after tools that have none.
        let last = tools.slots.len() - 1;
        tools.slots[1] = Some(Box::new(Scaffolded("3 things on the model")));
        tools.slots[last] = Some(Box::new(Scaffolded("2 things in the world")));
        assert_eq!(tools.scaffolds_at_risk(Loss::Closing), ["3 things on the model", "2 things in the world"]);
        // Each tool says for itself what a new scene costs.
        assert_eq!(tools.scaffolds_at_risk(Loss::SceneReplaced), ["3 things on the model"]);
    }

    fn kept_hash(tools: &Tools, scene: &Scene) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for object in &scene.objects {
            tools.hash_kept(object.id, &mut hasher);
        }
        hasher.finish()
    }

    #[test]
    fn what_the_tools_would_keep_for_an_object_hashes_the_same_until_it_changes() {
        let (scene, _) = scene_of(3);
        let mut tools = Tools::default();
        let none = kept_hash(&tools, &scene);
        assert_eq!(kept_hash(&Tools::default(), &scene), none, "nothing kept, the same from one moment to the next");
        give(&mut tools, &scene, &[(1, array(ArrayKind::Line, 4))]);
        let with = kept_hash(&tools, &scene);
        assert_ne!(with, none, "an array is something the file would keep");
        assert_eq!(kept_hash(&tools, &scene), with);
        give(&mut tools, &scene, &[(1, array(ArrayKind::Line, 5))]);
        let more = kept_hash(&tools, &scene);
        assert_ne!(more, with, "one copy more is another file");
        // The same array on another object is another file too.
        let mut other = Tools::default();
        give(&mut other, &scene, &[(2, array(ArrayKind::Line, 5))]);
        assert_ne!(kept_hash(&other, &scene), more);
    }

    #[test]
    fn a_saved_value_hashes_by_kind_as_well_as_by_content() {
        let hash = |saved: &Saved| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            hash_saved(saved, &mut hasher);
            hasher.finish()
        };
        let values = [
            Saved::Null,
            Saved::Bool(false),
            Saved::Number(0.0),
            Saved::Text(String::new()),
            Saved::List(Vec::new()),
            Saved::Fields(Vec::new()),
            Saved::numbers(&[1.0, 2.0]),
            Saved::List(vec![Saved::numbers(&[1.0]), Saved::numbers(&[2.0])]),
            Saved::fields([("count", 5.into())]),
            Saved::fields([("copies", 5.into())]),
        ];
        let hashes: Vec<u64> = values.iter().map(hash).collect();
        for (i, a) in hashes.iter().enumerate() {
            assert!(hashes[i + 1..].iter().all(|b| a != b), "{:?} hashes as another value does", values[i]);
        }
        assert_eq!(hash(&Saved::numbers(&[1.0, 2.0])), hashes[6], "and the same value the same way twice");
    }

    #[test]
    fn adding_a_file_gives_its_arrays_to_the_added_objects_only() {
        let (mut scene, mut slot) = scene_of(2);
        let mut tools = Tools::default();
        give(&mut tools, &scene, &[(0, array(ArrayKind::Line, 3))]);
        let own = kept(&tools, &scene);

        // The file's second object becomes the scene's third; its first was not taken (a row that is skipped).
        let added = scene.add("From the file", ball(), &mut slot);
        let mut details = Details::default();
        details.set_tool_object(0, "array", &sculpt_array::state_to_saved(&array(ArrayKind::Line, 7)));
        details.set_tool_object(1, "array", &sculpt_array::state_to_saved(&array(ArrayKind::Radial, 5)));
        tools.restore_from_file(&[(1, added)], &details, Arrival::Added);

        let now = kept(&tools, &scene);
        assert_eq!(now[..2], own[..], "the scene's own arrays are as they were");
        assert_eq!(now[2].as_ref().map(|a| a.settings.kind), Some(ArrayKind::Radial));
    }
}
