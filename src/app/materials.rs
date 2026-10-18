//! The material library and the scene's palette, the app's side: reading
//! the materials folder and the custom shaders at start-up and keeping them
//! up to date, packing the palette for the renderer each time it changes,
//! drawing the Materials card's thumbnails, and doing what the card asks.
//! The library itself is `materials/library.rs`, the files `materials/files.rs`.
//!
//! The two file boxes (Import Material, Export Material) are opened here and
//! nowhere else, each over a routine that takes a path
//! ([`Running::import_material`], [`Running::export_material`]), which is
//! what tests and the Claude bridge call: nothing scripted may put a file
//! box on the user's screen.

use std::path::Path;
use std::time::Instant;

use glam::{Mat4, Vec2};
use serde_json::{Value, json};

use super::Running;
use crate::doc::Doc;
use crate::materials::library::Origin;
use crate::materials::{Ask, builtin, custom, files, palette, rows, thumbs};
use crate::render::DrawItem;

/// The mesh id the thumbnails' ball is drawn under: no scene ever reaches it.
const BALL_ID: u64 = u64::MAX - 0x6d61;
/// How many thumbnails are drawn in one frame, at most.
const THUMBS_PER_FRAME: usize = 2;

impl Doc {
    /// Makes library material `i` the one paint strokes lay: it joins the
    /// scene's palette the first time (its pattern frozen at the model's size
    /// now, see `materials::palette::ensure_in_palette`), and its slot is
    /// `opts.paint.active_material`. Gives back the slot.
    pub fn paint_with(&mut self, i: usize) -> Result<u8, String> {
        let size = crate::look::model_size(&self.scene, &self.mesh);
        let opts = &mut self.opts;
        let state = &opts.materials;
        let material = state.library.entries().get(i).ok_or("There is no such material.")?.material.clone();
        let slot = palette::ensure_in_palette(&mut opts.paint, &material, size, &state.shaders)?;
        opts.paint.active_material = Some(slot);
        Ok(slot)
    }

    /// The palette slots some object's paint uses (its local table).
    pub fn paint_slots_in_use(&self) -> Vec<u8> {
        let mut used = Vec::new();
        for o in &self.scene.objects {
            if let Some(mesh) = self.scene.mesh(o.mesh, &self.mesh) {
                for &slot in mesh.paint_table() {
                    if !used.contains(&slot) {
                        used.push(slot);
                    }
                }
            }
        }
        used
    }
}

impl Running {
    /// Start-up: reads the custom shaders and the user's materials into the
    /// library, the first time only.
    pub(super) fn materials_restore(&mut self) {
        let folder = self.files.materials.folder().map(Path::to_path_buf);
        let writable = self.files.materials.writable();
        let state = &mut self.doc.opts.materials;
        let mut notes = Vec::new();
        if state.shaders.folder().is_none() && folder.is_some() {
            state.shaders = custom::Shaders::new(folder.as_deref());
            if let (true, Some(dir)) = (writable, state.shaders.folder()) {
                if let Err(e) = custom::install_example(dir) {
                    log::warn!("could not put the example custom material in place: {e}");
                }
            }
            state.thumbs = thumbs::Cache::new(folder.as_deref(), writable);
            notes.extend(state.shaders.scan());
            notes.extend(state.shaders.list().iter().filter_map(|s| s.failure()));
            state.library.set_shaders(&state.shaders);
        }
        notes.extend(self.files.materials.load(&mut state.library));
        if !notes.is_empty() {
            self.status(notes.join(" "), false);
        }
    }

    /// Writes the user's materials that changed to the materials folder.
    /// Called with the settings, now and then and as the app closes.
    pub(super) fn materials_keep(&mut self) {
        self.files.materials.sync(&self.doc.opts.materials.library);
    }

    /// Once a frame, before the mesh changes go to the card: custom shaders
    /// changed on disk are read again, and the palette is packed for the
    /// renderer if it, the model's size or a shader changed.
    pub(super) fn materials_frame(&mut self) {
        let state = &mut self.doc.opts.materials;
        let changed = state.shaders.poll(Instant::now());
        if !changed.is_empty() {
            state.shaders_seen += 1;
            state.library.set_shaders(&state.shaders);
            palette::refresh_sources(&mut self.doc.opts.paint, &changed, &state.shaders);
            let notes: Vec<String> = changed
                .iter()
                .filter_map(|id| state.shaders.get(id))
                .map(|s| s.failure().unwrap_or_else(|| format!("Custom material \"{}\" was read again.", s.name())))
                .collect();
            let ok = state.shaders.list().iter().all(custom::Shader::ok);
            if !notes.is_empty() {
                self.status(notes.join(" "), ok);
            }
        }
        self.materials_pack();
        let said = std::mem::take(&mut self.renderer.painted.said);
        if !said.is_empty() {
            self.status(said.join(" "), false);
        }
    }

    /// Packs the palette for the renderer if anything it is packed from changed.
    fn materials_pack(&mut self) {
        let size = crate::look::model_size(&self.doc.scene, &self.doc.mesh);
        let opts = &mut self.doc.opts;
        let state = &mut opts.materials;
        let same = state
            .packed_from
            .as_ref()
            .is_some_and(|(paint, at, seen)| *paint == opts.paint && *at == size && *seen == state.shaders_seen);
        if same {
            return;
        }
        palette::carry_sources(&opts.paint, &mut state.shaders);
        let upload = rows::upload(&opts.paint, size, &state.shaders);
        self.renderer.painted.set_palette(&self.gpu, &upload);
        state.packed_from = Some((opts.paint.clone(), size, state.shaders_seen));
        self.redraw = true;
    }

    /// After the window's frame has gone out: draws up to two of the
    /// thumbnails the Materials card showed a row without, each in a
    /// submission of its own, keeping each as a picture file and a texture
    /// for the card. A picture kept from an earlier run is only read.
    pub(super) fn materials_thumbs(&mut self) {
        let keys_stale = self.ui.materials.keys.len() != self.doc.opts.materials.library.entries().len()
            || self.ui.materials.keys_seen != (self.doc.opts.materials.library.generation(), self.doc.opts.materials.shaders_seen);
        if keys_stale {
            let state = &self.doc.opts.materials;
            self.ui.materials.keys = state.library.entries().iter().map(|e| thumbs::key(&e.material, &state.shaders)).collect();
            self.ui.materials.keys_seen = (state.library.generation(), state.shaders_seen);
        }
        let wanted = std::mem::take(&mut self.ui.materials.wanted);
        let mut drawn = 0;
        for i in wanted {
            let Some(&key) = self.ui.materials.keys.get(i) else { continue };
            if self.ui.materials.textures.contains_key(&key) {
                continue;
            }
            let picture = match self.doc.opts.materials.thumbs.load(key) {
                Some(picture) => picture,
                None if drawn < THUMBS_PER_FRAME => {
                    drawn += 1;
                    let Some(picture) = self.draw_thumb(i) else { continue };
                    self.doc.opts.materials.thumbs.store(key, &picture);
                    picture
                }
                None => {
                    // More next frame.
                    self.redraw = true;
                    continue;
                }
            };
            let size = [picture.width() as usize, picture.height() as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, picture.as_raw());
            let texture = self.egui_ctx.load_texture(format!("material {key:016x}"), image, egui::TextureOptions::LINEAR);
            self.ui.materials.textures.insert(key, texture);
            self.redraw = true;
        }
    }

    /// Draws library material `i` on the ball, offscreen, in a submission of
    /// its own: the scene's palette is put back for the window's next frame.
    fn draw_thumb(&mut self, i: usize) -> Option<image::RgbaImage> {
        let state = &mut self.doc.opts.materials;
        let material = state.library.entries().get(i)?.material.clone();
        let ball = state.ball.get_or_insert_with(thumbs::painted_ball);
        let mut paint = crate::mesh::paint::palette::ScenePaint::default();
        palette::ensure_in_palette(&mut paint, &material, thumbs::BALL_SIZE, &state.shaders).ok()?;
        palette::carry_sources(&paint, &mut state.shaders);
        let upload = rows::upload(&paint, thumbs::BALL_SIZE, &state.shaders);

        let side = thumbs::DRAW_SIDE;
        let rig = self.ui.materials.rig.get_or_insert_with(|| crate::ui::materials::Rig::new(&self.gpu, side));
        let mut opts = thumbs::options();
        opts.paint.palette = paint.palette;
        // The window's shadow and reflection sizes, so the maps they share
        // are not made afresh for each thumbnail.
        opts.quality = self.doc.opts.quality;
        let mut camera = crate::camera::Camera::default();
        (camera.yaw, camera.pitch) = (-25.0, -18.0);
        camera.frame(ball.bounds(), 1.0, false);
        camera.distance *= 0.92;
        let view = camera.view(Vec2::ZERO, Vec2::splat(side as f32), camera.distance, 2.0);
        let globals = crate::bench_render::globals(&view, &opts, false);
        let look = opts.look.uniform(&thumbs::frame(&view, ball.bounds()));
        let items = [DrawItem { mesh: BALL_ID, model: Mat4::IDENTITY.to_cols_array_2d() }];

        let renderer = &mut self.renderer;
        let scene_palette = renderer.painted.upload().clone();
        let kept = (renderer.want_curvature, renderer.overlays, renderer.wire.enabled, renderer.groups.enabled);
        renderer.painted.set_palette(&self.gpu, &upload);
        (renderer.want_curvature, renderer.overlays, renderer.wire.enabled, renderer.groups.enabled) = (true, false, false, false);
        renderer.sync_mesh(&self.gpu, BALL_ID, ball, false);
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        // Each thumbnail is a still picture, never one in motion.
        let picture = std::mem::replace(&mut renderer.picture, true);
        renderer.swap_side(&mut rig.side);
        renderer.draw(&self.gpu, &mut encoder, &rig.view, (side, side), &globals, &look, &opts, &items);
        renderer.swap_side(&mut rig.side);
        renderer.picture = picture;
        self.gpu.queue.submit([encoder.finish()]);
        renderer.painted.set_palette(&self.gpu, &scene_palette);
        (renderer.want_curvature, renderer.overlays, renderer.wire.enabled, renderer.groups.enabled) = kept;
        renderer.retain_meshes(|id| id != BALL_ID);
        let picture = crate::render::read_texture(&self.gpu, &rig.texture);
        Some(thumbs::finish(&picture))
    }

    /// Does what the card asked and says what happened.
    pub(super) fn material_ask(&mut self, ask: Ask) {
        let said = match ask {
            Ask::Import => {
                let Some(path) = pick_to_import(self.files.materials.folder()) else { return };
                self.import_material(&path)
            }
            Ask::Export(i) => {
                let Some(entry) = self.doc.opts.materials.library.entries().get(i) else { return };
                let Some(path) = pick_to_export(&entry.material.name) else { return };
                self.export_material(i, &path)
            }
            other => self.material_edit(other),
        };
        match said {
            Ok(message) if message.is_empty() => {}
            Ok(message) => self.status(message, true),
            Err(why) => self.status(why, false),
        }
    }

    /// Every ask that needs no file box: what the Claude bridge may ask too.
    pub(super) fn material_edit(&mut self, ask: Ask) -> Result<String, String> {
        let size = crate::look::model_size(&self.doc.scene, &self.doc.mesh);
        let mut used = self.doc.paint_slots_in_use();
        if let Ask::Select(i) = ask {
            self.doc.paint_with(i)?;
            return Ok(String::new());
        }
        let opts = &mut self.doc.opts;
        let state = &mut opts.materials;
        let library = &mut state.library;
        let name = |library: &crate::materials::Library, i: usize| {
            library.entries().get(i).map(|e| e.material.name.clone()).ok_or("There is no such material.")
        };
        match ask {
            Ask::Select(_) => Ok(String::new()),
            Ask::SelectSlot(slot) => {
                opts.paint.palette.iter().find(|e| e.slot == slot).ok_or("There is no such palette entry.")?;
                opts.paint.active_material = Some(slot);
                Ok(String::new())
            }
            Ask::ColourOnly => {
                opts.paint.active_material = None;
                Ok("Painting lays colour only.".into())
            }
            Ask::Duplicate(i) => {
                let from = name(library, i)?;
                let new = library.duplicate(i).ok_or("There is no such material.")?;
                let new_name = library.entries()[new].material.name.clone();
                Ok(format!("{new_name} is a copy of {from}, and yours to change."))
            }
            Ask::Rename(i, to) => {
                let from = name(library, i)?;
                library.rename(i, &to)?;
                let to = library.entries()[i].material.name.clone();
                palette::follow_rename(&mut opts.paint, &from, &to);
                Ok(format!("{from} is now called {to}."))
            }
            Ask::Delete(i) => {
                let gone = name(library, i)?;
                if library.entries()[i].origin != Origin::User {
                    return Err(format!("{gone} comes with the app, and stays."));
                }
                let aside = self.files.materials.set_aside(library, i);
                library.delete(i)?;
                let kept = if aside.is_some() {
                    " A copy is in the materials folder, under deleted; Import Material brings it back."
                } else {
                    ""
                };
                Ok(format!("Deleted the material {gone}. What is painted with it stays as it is.{kept}"))
            }
            Ask::Set(i, material) => {
                let before = library.set(i, material)?;
                let after = library.entries()[i].material.clone();
                palette::follow_edit(&mut opts.paint, &before, &after, &state.shaders);
                Ok(String::new())
            }
            Ask::SaveLook => {
                let material = builtin::from_look(&opts.look, opts.base_color, opts.smoothness);
                let i = library.add(material);
                let made = library.entries()[i].material.name.clone();
                Ok(format!("Saved the Look's finish as the material {made}."))
            }
            Ask::SaveSlot(slot) => {
                let entry = opts.paint.palette.iter().find(|e| e.slot == slot).ok_or("There is no such palette entry.")?;
                let i = library.add(palette::material_of(entry));
                Ok(format!("{} is in your library now.", library.entries()[i].material.name))
            }
            Ask::RemoveUnused => {
                // The material picked to paint with stays, painted yet or not.
                used.extend(opts.paint.active_material);
                let gone = palette::remove_unused(&mut opts.paint, &used);
                Ok(match gone {
                    0 => "Every material in the palette is painted on something.".to_owned(),
                    1 => "Took 1 unused material out of the palette.".to_owned(),
                    n => format!("Took {n} unused materials out of the palette."),
                })
            }
            Ask::Replace(slot, i) => {
                let material = library.entries().get(i).ok_or("There is no such material.")?.material.clone();
                palette::replace(&mut opts.paint, slot, &material, &state.shaders)
            }
            Ask::Remeasure(slot) => palette::remeasure(&mut opts.paint, slot, size),
            Ask::ReloadShaders => {
                let notes = state.shaders.scan();
                state.shaders_seen += 1;
                library.set_shaders(&state.shaders);
                let ids: Vec<String> = state.shaders.list().iter().map(|s| s.id.clone()).collect();
                palette::refresh_sources(&mut opts.paint, &ids, &state.shaders);
                let failures: Vec<String> = state.shaders.list().iter().filter_map(|s| s.failure()).collect();
                let count = state.shaders.list().len();
                let mut said = vec![match count {
                    1 => "Read 1 custom material.".to_owned(),
                    n => format!("Read {n} custom materials."),
                }];
                said.extend(notes);
                said.extend(failures.iter().cloned());
                let text = said.join(" ");
                if failures.is_empty() { Ok(text) } else { Err(text) }
            }
            Ask::Import | Ask::Export(_) => Err("That needs a file to be named.".into()),
        }
    }

    /// Brings the material file at `path` into the library as one of the
    /// user's own, and picks it for painting.
    pub(super) fn import_material(&mut self, path: &Path) -> Result<String, String> {
        let library = &mut self.doc.opts.materials.library;
        let (_, said) = files::import(path, library)?;
        Ok(said)
    }

    /// Writes material `i` to `path` as it is now.
    pub(super) fn export_material(&mut self, i: usize, path: &Path) -> Result<String, String> {
        files::export(path, &self.doc.opts.materials.library, i)
    }

    /// The bridge's material commands: `materials` lists the library and the
    /// palette; `paint_material` picks what painting lays, by a library
    /// material's `material` name (which joins the palette) or a palette
    /// `slot` (`"none"` for colour only).
    pub(super) fn materials_command(&mut self, name: &str, args: &Value) -> Option<Result<String, String>> {
        match name {
            "materials" => Some(Ok(self.materials_listing().to_string())),
            "paintmaterial" => Some(self.bridge_paint_material(args)),
            _ => None,
        }
    }

    fn materials_listing(&self) -> Value {
        let state = &self.doc.opts.materials;
        let library: Vec<Value> = state
            .library
            .entries()
            .iter()
            .map(|e| {
                let origin = match e.origin {
                    Origin::BuiltIn => "built in",
                    Origin::Shader => "custom shader",
                    Origin::User => "yours",
                };
                let mut row = json!({ "name": e.material.name, "finish": e.material.kind().name(), "origin": origin });
                if let crate::materials::Settings::Custom(c) = &e.material.settings {
                    if let Some(why) = state.shaders.get(&c.id).and_then(|s| s.failure()) {
                        row["problem"] = json!(why);
                    }
                }
                row
            })
            .collect();
        let palette: Vec<Value> = self
            .doc
            .opts
            .paint
            .palette
            .iter()
            .map(|e| json!({ "slot": e.slot, "name": e.name, "from": palette::library_name(e), "reference": e.reference }))
            .collect();
        let picked = match self.doc.opts.paint.active_material {
            Some(slot) => json!(slot),
            None => Value::Null,
        };
        json!({ "library": library, "palette": palette, "active_material": picked,
                "show_polypaint": self.doc.opts.paint.show_polypaint, "show_materials": self.doc.opts.paint.show_materials })
    }

    fn bridge_paint_material(&mut self, args: &Value) -> Result<String, String> {
        if let Some(slot) = args.get("slot").and_then(Value::as_u64) {
            let slot = u8::try_from(slot).map_err(|_| "slot is from 1 to 255.")?;
            self.material_edit(Ask::SelectSlot(slot))?;
            return Ok(format!("Painting with palette slot {slot}."));
        }
        let wanted = args.get("material").and_then(Value::as_str).ok_or("paint_material needs a material name or a slot.")?;
        if wanted.trim().eq_ignore_ascii_case("none") {
            self.doc.opts.paint.active_material = None;
            return Ok("Painting with no material.".into());
        }
        let library = &self.doc.opts.materials.library;
        let i = library.named(wanted).ok_or_else(|| {
            let names: Vec<&str> = library.entries().iter().map(|e| e.material.name.as_str()).collect();
            format!("There is no material \"{wanted}\". The materials are: {}.", names.join(", "))
        })?;
        let slot = self.doc.paint_with(i)?;
        let name = self.doc.opts.materials.library.entries()[i].material.name.clone();
        Ok(format!("Painting with {name}, palette slot {slot}."))
    }
}

/// Asks which material file to import, starting in the materials folder's
/// `deleted` folder if there is one. A file box: never in a scripted run.
fn pick_to_import(folder: Option<&Path>) -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Import Material").add_filter("Materials", &["json"]);
    if let Some(folder) = folder.filter(|f| f.is_dir()) {
        dialog = dialog.set_directory(folder);
    }
    dialog.pick_file()
}

/// Asks where to write the material called `name`. A file box: never in a
/// scripted run.
fn pick_to_export(name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export Material")
        .add_filter("Materials", &["json"])
        .set_file_name(format!("{}.json", files::file_stem(name)))
        .save_file()
}
