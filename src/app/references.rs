//! Reference pictures on the app's side (see references.rs): Add Picture...
//! and a picture added from a path with no file box (the Claude bridge, the
//! scripted checks), Delete, the pictures handed to the renderer every frame,
//! the pictures a file brings, and the bridge's commands.
//!
//! A file's pictures come in through `see` (app/files.rs), with the look and
//! the view: an Open, a recovery copy put back and a New Scene each replace
//! the scene's pictures with the file's (none, for a file without), and
//! Undo across one of those gives the scene before it its own pictures back,
//! as it gives it its own look. Import adds a file's pictures to the scene's.
//! A file is read in full before anything changes; an image in it that will
//! not decode costs that picture alone, and the status line says so.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use super::Running;
use crate::doc::Action;
use crate::io;
use crate::references::{self, Ask, Facing, Picture, Placement};

/// "1 reference picture", "3 reference pictures".
fn pictures(count: usize) -> String {
    if count == 1 { "1 reference picture".to_owned() } else { format!("{count} reference pictures") }
}

/// The words the bridge and the scripted checks use for a facing.
fn facing_named(name: &str) -> Option<Facing> {
    Facing::ALL.into_iter().find(|f| f.name().eq_ignore_ascii_case(name.trim()))
}

impl Running {
    pub(super) fn references_act(&mut self, ask: Ask) {
        match ask {
            Ask::Add => self.add_reference_asking(),
            Ask::Delete(id) => self.delete_reference(id),
        }
    }

    /// Add Picture...: the file box, then the picture chosen.
    fn add_reference_asking(&mut self) {
        let pick = rfd::FileDialog::new()
            .set_title("Add Reference Picture")
            .add_filter("Pictures", references::EXTENSIONS)
            .pick_file();
        let Some(path) = pick else { return };
        match self.add_reference(&path) {
            Ok(said) => self.status(said, true),
            Err(e) => self.status(e, false),
        }
    }

    /// Stands the picture at `path` in the scene, selected: facing the axis
    /// view the camera is in (the front, if it is in none), as tall as the
    /// scene, its middle on the scene's and just behind it. Gives back what
    /// to say of it. The image is read in full first; the scene is not
    /// touched if it will not read.
    pub(super) fn add_reference(&mut self, path: &Path) -> Result<String, String> {
        let (name, file) = references::read_file(path)?;
        let scene = &self.doc.scene.references;
        let (file, image) = match scene.same_image(&file) {
            // The same photo again (for the other side): held once.
            Some(shared) => shared,
            None => {
                let image = references::decode(&file).map_err(|e| format!("{name}: {e}"))?;
                (Arc::from(file), Arc::new(image))
            }
        };
        let view = self.doc.camera.saved_view();
        let facing = Facing::of_view(view.yaw, view.pitch).unwrap_or_default();
        let place = Placement::fitted(facing, self.doc.scene.bounds(&self.doc.mesh));
        let (w, h) = image.dimensions();
        self.doc.scene.references.add(Picture::new(name.clone(), file, image, place));
        self.redraw = true;
        Ok(format!(
            "Added {name} ({w} x {h}) as a reference picture facing the {} view. It is in the Reference Images card \
             (Scene panel, Objects tab).",
            facing.name().to_lowercase()
        ))
    }

    fn delete_reference(&mut self, id: u64) {
        if let Some(gone) = self.doc.scene.references.remove(id) {
            self.status(format!("Deleted the reference picture {}.", gone.name), true);
            self.redraw = true;
        }
    }

    /// Hands the scene's pictures to the renderer, as the camera sees them
    /// this frame.
    pub(super) fn references_frame(&mut self) {
        self.renderer.references.show(&self.gpu, &self.view, &self.doc.scene.references.pictures);
    }

    /// The scene's pictures replaced by those `details` holds (an opened
    /// file's, or across an Open the scene's own again): none, for a file
    /// that holds none.
    pub(super) fn references_see(&mut self, details: &io::Details) {
        let (pictures, unread) = references::restored(details, &self.doc.scene.references);
        let scene = &mut self.doc.scene.references;
        scene.selected = pictures.first().map(|p| p.id);
        scene.pictures = pictures;
        self.unread_note(unread);
        self.redraw = true;
    }

    /// Import of a `.sclp`: its pictures join the scene's.
    pub(super) fn references_add_from(&mut self, details: &io::Details) {
        let (pictures, unread) = references::restored(details, &self.doc.scene.references);
        for picture in pictures {
            self.doc.scene.references.add(picture);
        }
        self.unread_note(unread);
    }

    /// Says, in the log and on the status line, that pictures a file held
    /// could not be read. (An Open or an Import says what it did after this,
    /// on the status line; the log keeps it.)
    fn unread_note(&mut self, unread: usize) {
        if unread > 0 {
            let was = if unread == 1 { "was" } else { "were" };
            let said = format!("{} in the file could not be read, and {was} left out.", pictures(unread));
            log::warn!("{said}");
            self.status(said, false);
        }
    }

    // ---- the Claude bridge

    /// The bridge's commands for reference pictures (`command` with these
    /// names): `None` for a name that is not one of them.
    pub(super) fn references_command(&mut self, name: &str, args: &Value) -> Option<Result<String, String>> {
        let done = match name {
            "addpicture" => self.add_picture_call(args),
            "listpictures" => Ok(self.pictures_listed().to_string()),
            "editpicture" => self.edit_picture_call(args),
            "deletepicture" => self.picture_named(args).map(|id| {
                self.delete_reference(id);
                self.ui.status.0.clone()
            }),
            _ => return None,
        };
        Some(done)
    }

    /// `add_picture`: the image at `path` (a full path on this computer's own
    /// drives, as `open_file` takes), then any settings given, as
    /// `edit_picture` takes them. A `facing` turns it to that view instead of
    /// the camera's.
    fn add_picture_call(&mut self, args: &Value) -> Result<String, String> {
        let (path, _) = Self::path_named(args, "path")?;
        if !path.is_file() {
            return Err(format!("There is no file at {}.", path.display()));
        }
        let said = self.add_reference(&path)?;
        let id = self.doc.scene.references.selected.ok_or("The picture was not added.")?;
        if let Some(facing) = args.get("facing").and_then(Value::as_str) {
            let facing = facing_named(facing).ok_or_else(|| format!("There is no facing \"{facing}\"."))?;
            let fitted = Placement::fitted(facing, self.doc.scene.bounds(&self.doc.mesh));
            if let Some(picture) = self.doc.scene.references.get_mut(id) {
                picture.place = fitted;
            }
        }
        self.edit_picture(id, args)?;
        self.status(said.clone(), true);
        Ok(format!("{said} {}", self.picture_state(id)))
    }

    fn edit_picture_call(&mut self, args: &Value) -> Result<String, String> {
        let id = self.picture_named(args)?;
        self.edit_picture(id, args)?;
        Ok(self.picture_state(id).to_string())
    }

    /// The settings `args` gives, on picture `id`: `facing` (keeping where
    /// it stands), `size`, `across`, `up` and `depth` in the scene's units,
    /// `opacity` in percent, `flip`, `in_front` and `show`.
    fn edit_picture(&mut self, id: u64, args: &Value) -> Result<(), String> {
        let facing = match args.get("facing").and_then(Value::as_str) {
            Some(name) => Some(facing_named(name).ok_or_else(|| format!("There is no facing \"{name}\"."))?),
            None => None,
        };
        let number = |key: &str| args.get(key).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite());
        let flag = |key: &str| args.get(key).and_then(Value::as_bool);
        let Some(picture) = self.doc.scene.references.get_mut(id) else { return Err("That picture is gone.".into()) };
        let place = &mut picture.place;
        place.facing = facing.unwrap_or(place.facing);
        if let Some(size) = number("size") {
            place.size = size.max(1e-4);
        }
        place.across = number("across").unwrap_or(place.across);
        place.up = number("up").unwrap_or(place.up);
        place.depth = number("depth").unwrap_or(place.depth);
        if let Some(opacity) = number("opacity") {
            place.opacity = (opacity / 100.0).clamp(0.0, 1.0);
        }
        place.flip = flag("flip").unwrap_or(place.flip);
        place.in_front = flag("in_front").unwrap_or(place.in_front);
        place.shown = flag("show").unwrap_or(place.shown);
        self.redraw = true;
        Ok(())
    }

    /// The picture `args` names as `picture`: its number in the list (1 for
    /// the first) or its name.
    fn picture_named(&self, args: &Value) -> Result<u64, String> {
        let list = &self.doc.scene.references.pictures;
        if list.is_empty() {
            return Err("The scene has no reference pictures.".into());
        }
        let wanted = args.get("picture").ok_or("Name the picture: its number in list_pictures (1 for the first) or its name.")?;
        let found = match wanted.as_u64() {
            Some(number) => number.checked_sub(1).and_then(|i| list.get(i as usize)),
            None => wanted.as_str().and_then(|name| list.iter().find(|p| p.name == name)),
        };
        found.map(|p| p.id).ok_or_else(|| format!("There is no picture {wanted}; the scene has {}.", pictures(list.len())))
    }

    fn picture_state(&self, id: u64) -> Value {
        let list = &self.doc.scene.references.pictures;
        let Some((row, picture)) = list.iter().enumerate().find(|(_, p)| p.id == id) else { return Value::Null };
        let place = &picture.place;
        let (w, h) = picture.image.dimensions();
        json!({
            "picture": row + 1,
            "name": picture.name,
            "pixels": [w, h],
            "facing": place.facing.name().to_lowercase(),
            "size": place.size,
            "width": picture.width(),
            "across": place.across,
            "up": place.up,
            "depth": place.depth,
            "centre": place.centre().to_array(),
            "opacity": (place.opacity * 100.0).round(),
            "flip": place.flip,
            "in_front": place.in_front,
            "show": place.shown,
        })
    }

    /// `list_pictures`: every picture with its settings, in the card's order.
    fn pictures_listed(&self) -> Value {
        let list = &self.doc.scene.references.pictures;
        Value::Array(list.iter().map(|p| self.picture_state(p.id)).collect())
    }

    // ---- scripted checks

    /// `--demo-references <picture>[,<facing>][,ortho][,in-front][,<opacity %>][,orbit=<yaw>:<pitch>]`:
    /// the camera turned to that axis view (orthographic with `ortho`) and
    /// framed on the scene, then the picture added as Add Picture... adds
    /// one, with no file box. `play` works the pictures through saving,
    /// opening, Undo and Import and prints a line per check.
    pub(super) fn references_demo(&mut self, spec: &str) {
        if spec == "play" {
            return self.references_play();
        }
        let mut words = spec.split(',');
        let path = words.next().unwrap_or_default().to_owned();
        let (mut in_front, mut opacity, mut ortho, mut facing, mut orbit) = (false, None, false, None, None);
        for word in words.map(str::trim) {
            let turn = word.strip_prefix("orbit=").map(|by| by.split_once(':').unwrap_or((by, "0")));
            match word {
                "ortho" => ortho = true,
                "in-front" => in_front = true,
                _ if turn.is_some() => orbit = turn.and_then(|(yaw, pitch)| Some((yaw.parse::<f32>().ok()?, pitch.parse::<f32>().ok()?))),
                other => match (facing_named(other), other.parse::<f32>()) {
                    (Some(f), _) => facing = Some(f),
                    (None, Ok(percent)) => opacity = Some(percent),
                    _ => eprintln!("--demo-references: no such word as {other:?}"),
                },
            }
        }
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        if let Some(facing) = facing {
            let camera = &mut self.doc.camera;
            (camera.yaw, camera.pitch) = facing.view();
            camera.ortho = ortho;
            camera.frame(self.doc.scene.bounds(&self.doc.mesh), aspect, false);
            self.sync_view();
        }
        match self.add_reference(Path::new(&path)) {
            Ok(said) => {
                println!("{said}");
                let args = json!({ "in_front": in_front, "opacity": opacity.unwrap_or(if in_front { 50.0 } else { 100.0 }) });
                if let Some(id) = self.doc.scene.references.selected {
                    let _ = self.edit_picture(id, &args);
                }
                self.status(said, true);
            }
            Err(e) => {
                eprintln!("{e}");
                self.status(e, false);
            }
        }
        // Then, with `orbit=<yaw>:<pitch>`, turned that far round the model
        // in perspective: at an angle to the picture, or round behind it.
        if let Some((yaw, pitch)) = orbit {
            let camera = &mut self.doc.camera;
            (camera.yaw, camera.pitch, camera.ortho) = (camera.yaw + yaw, pitch.clamp(-89.0, 89.0), false);
            camera.frame(self.doc.scene.bounds(&self.doc.mesh), aspect, false);
            self.sync_view();
        }
    }

    /// Works the pictures the way a sculptor would, through the routines the
    /// card and the File tab run, and prints what came of each step. Its
    /// files go in a scratch folder of the run's own (`prefs::scratch`).
    fn references_play(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let folder = crate::prefs::scratch("sculpt_references_play");
        let _ = std::fs::create_dir_all(&folder);
        // A front photo (a tall gradient) and a side one, as PNG files.
        let (front, side, file) = (folder.join("front.png"), folder.join("side.png"), folder.join("pictures.sclp"));
        let gradient = image::RgbaImage::from_fn(60, 120, |x, y| image::Rgba([(x * 4) as u8, (y * 2) as u8, 160, 255]));
        let written = gradient.save(&front).is_ok() && image::RgbaImage::from_pixel(50, 120, image::Rgba([40, 200, 90, 255])).save(&side).is_ok();
        say("(set up) two pictures written to the scratch folder", written);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        let mark = self.history_mark();

        // From the right view a new picture faces the right; from the front, the front.
        (self.doc.camera.yaw, self.doc.camera.pitch) = (-90.0, 0.0);
        let added = self.add_reference(&side);
        let first = self.doc.scene.references.selected().map(|p| p.place);
        say("a picture added in the right view faces the right", added.is_ok() && first.is_some_and(|p| p.facing == Facing::Right));
        say("and stands behind the model, seen from there", first.is_some_and(|p| p.centre().x < bounds.min.x));
        (self.doc.camera.yaw, self.doc.camera.pitch) = (0.0, 0.0);
        self.add_reference(&front).ok();
        let second = self.doc.scene.references.selected().map(|p| (p.place, p.aspect()));
        say("one added in the front view faces the front", second.is_some_and(|(p, _)| p.facing == Facing::Front));
        say(
            "as tall as the model, its middle on the model's, its width from its image",
            second.is_some_and(|(p, aspect)| {
                (p.size - (bounds.max.y - bounds.min.y)).abs() < 1e-4
                    && (p.centre().y - (bounds.min.y + bounds.max.y) * 0.5).abs() < 1e-4
                    && (aspect - 0.5).abs() < 1e-6
            }),
        );
        say("the scene's bounds leave the pictures out", self.doc.scene.bounds(&self.doc.mesh).max == bounds.max);
        say("adding pictures is unsaved work", self.history_mark() != mark && self.unsaved());

        // A setting changed counts, and put back it no longer does.
        let id = self.doc.scene.references.selected.unwrap_or_default();
        let before = self.history_mark();
        let set = |app: &mut Self, opacity: f32| {
            if let Some(p) = app.doc.scene.references.get_mut(id) {
                p.place.opacity = opacity;
            }
        };
        set(self, 0.3);
        let changed = self.history_mark() != before;
        set(self, 1.0);
        say("a changed setting is a change to save, and put back is none", changed && self.history_mark() == before);

        // Saved and opened again: both pictures, image and settings.
        if let Some(p) = self.doc.scene.references.get_mut(id) {
            (p.place.in_front, p.place.opacity, p.place.flip) = (true, 0.5, true);
        }
        let saved = self.save_scene(&file);
        self.doc.path = Some(file.clone());
        let on_disk = io::load_scene(&file).map(|s| references::saved(&s.details).len()).unwrap_or(0);
        say("Save writes both pictures into the file", saved.is_ok() && on_disk == 2 && !self.unsaved());
        let wanted: Vec<(String, Placement)> = self.doc.scene.references.pictures.iter().map(|p| (p.name.clone(), p.place)).collect();
        self.delete_reference(id);
        say("Delete takes a picture out", self.doc.scene.references.pictures.len() == 1 && self.unsaved());
        self.load(file.clone(), true);
        let back: Vec<(String, Placement)> = self.doc.scene.references.pictures.iter().map(|p| (p.name.clone(), p.place)).collect();
        say("Open brings the file's pictures back as they were saved", back == wanted && !self.unsaved());
        // Undo across the Open: the scene before it, with its own one picture.
        self.act(Action::Undo);
        say("Undo of the Open gives the scene before it its own pictures", self.doc.scene.references.pictures.len() == 1);
        self.act(Action::Redo);
        say("and Redo the file's", self.doc.scene.references.pictures.len() == 2);
        // Import adds a sculpt's pictures to the scene's.
        self.load(file.clone(), false);
        say("Import adds the file's pictures to the scene's", self.doc.scene.references.pictures.len() == 4);

        // The Claude bridge's commands, as a call through the socket runs them.
        let command = |app: &mut Self, args: Value| app.call("command", &args);
        let path = side.to_string_lossy().into_owned();
        let added = command(self, json!({ "name": "add_picture", "path": path, "facing": "left", "opacity": 40 }));
        let last = self.doc.scene.references.pictures.last().map(|p| p.place);
        say(
            "add_picture adds one facing as asked, with its settings",
            added.is_ok() && last.is_some_and(|p| p.facing == Facing::Left && (p.opacity - 0.4).abs() < 1e-6),
        );
        let refused = command(self, json!({ "name": "add_picture", "path": "side.png" }));
        say("and refuses a path that is not a full one", refused.is_err() && self.doc.scene.references.pictures.len() == 5);
        let listed = command(self, json!({ "name": "list_pictures" }));
        let count = listed.as_ref().ok().and_then(|text| serde_json::from_str::<Value>(text).ok()).and_then(|v| v.as_array().map(Vec::len));
        say("list_pictures lists them all", count == Some(5));
        let edited = command(self, json!({ "name": "edit_picture", "picture": 5, "in_front": true, "facing": "top" }));
        let last = self.doc.scene.references.pictures.last().map(|p| p.place);
        say("edit_picture changes the settings named", edited.is_ok() && last.is_some_and(|p| p.in_front && p.facing == Facing::Top));
        let deleted = command(self, json!({ "name": "delete_picture", "picture": "side.png" }));
        say("delete_picture takes the one named out", deleted.is_ok() && self.doc.scene.references.pictures.len() == 4);
        let shown = self.doc.scene.references.pictures.iter().filter(|p| p.place.shown).count();
        println!("{} pictures in the scene, {shown} shown: {}", self.doc.scene.references.pictures.len(), self.pictures_listed());
    }
}
