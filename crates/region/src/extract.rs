//! The Extract tool: the masked area of the selected object pulled out as its
//! own solid with thickness (an armour plate, a strap, a piece of clothing).
//!
//! The preview is the point. Extraction has seven settings that work on each
//! other and makes a brand new object, so doing it blind would mean making an
//! object, looking at it, deleting it and guessing again. While the tool is
//! up a see-through shell shows what the settings give, rebuilt as a slider
//! moves; Accept turns that shell into a real object as one undo step, and
//! putting the tool away discards it. The source is never touched.
//!
//! Nothing here takes the pointer: a drag orbits, as everywhere in the app.

use std::sync::Arc;

use egui::Color32;
use glam::Vec3;
use rayon::prelude::*;
use sculpt_cut::{ExtractSettings, extract};
use sculpt_geom::TriMesh;
use sculpt_toolkit::draw::smooth_normals;
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};
use sculpt_toolkit::{
    Command, Ctx, Draw, Event, Info, Job, Key, MeshData, MeshId, MeshView, NewObject, ObjectId, Placement, Preview,
    PreviewData, Style, Tool,
};

/// The preview shell: the Unity app's own blue, see-through enough to judge
/// how the piece sits on the body under it.
const PREVIEW: Color32 = Color32::from_rgba_premultiplied(35, 86, 115, 115);
const CARD: Color32 = Color32::from_rgb(77, 191, 255);
const TITLE: &str = "Extract (from Mask)";

/// Above this many source triangles the shell is worked out as a background
/// job (about 50 ms per million), and a new object this size is baked there.
const JOB_TRIANGLES: usize = 150_000;

/// How often the source is looked at again for a mask or shape that changed
/// under the preview (an undo, say). Nothing tells a tool when vertices or
/// mask values change, so it is a timer rather than every frame.
const LOOK_SECONDS: f64 = 0.25;

/// What a preview was built from: when any of it differs from what is in the
/// scene now, the preview is out of date.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Source {
    object: ObjectId,
    mesh: MeshId,
    settings: ExtractSettings,
    stamp: Stamp,
}

/// A fingerprint of a mesh's mask and vertex positions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    vertices: usize,
    mask: u64,
    shape: u64,
}

impl Stamp {
    /// Summed as integers, so the answer does not depend on how the work
    /// was shared out between threads the way a float sum would.
    fn of(mesh: &MeshView) -> Stamp {
        let weight = |i: usize| (i as u64).wrapping_mul(2).wrapping_add(1);
        let mask = mesh
            .mask
            .par_iter()
            .enumerate()
            .with_min_len(1 << 14)
            .map(|(i, m)| (m.to_bits() as u64).wrapping_mul(weight(i)))
            .reduce(|| 0, u64::wrapping_add);
        let shape = mesh
            .positions
            .par_iter()
            .enumerate()
            .with_min_len(1 << 14)
            .map(|(i, p)| {
                let bits = p.x.to_bits() as u64 ^ (p.y.to_bits() as u64).rotate_left(21) ^ (p.z.to_bits() as u64).rotate_left(42);
                bits.wrapping_mul(weight(i))
            })
            .reduce(|| 0, u64::wrapping_add);
        Stamp { vertices: mesh.vertex_count(), mask, shape }
    }
}

/// The source's arrays, copied for a job (which cannot borrow the scene).
struct Copied {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
    mask: Vec<f32>,
}

/// A shell on its way into the scene.
struct Made {
    name: String,
    placement: Placement,
    mesh: MeshData,
    triangles: usize,
}

pub struct Extract {
    settings: ExtractSettings,
    /// The shell, in the source object's own space.
    preview: Preview,
    /// What the shell on screen was built from.
    shown: Option<Source>,
    triangles: usize,
    /// Why there is no shell, when there is none.
    error: Option<String>,
    /// A shell being worked out off the interface thread, and what from.
    building: Option<(Source, Job<PreviewData>)>,
    /// The copy the last job worked from, kept while the tool is up and the
    /// source stays as it was: dragging a slider on a large mesh then pays
    /// for the copy once, not once per rebuild.
    copied: Option<((ObjectId, MeshId, Stamp), Arc<Copied>)>,
    /// An accepted shell being baked.
    adding: Option<Job<Made>>,
    next_look: f64,
    pub(crate) job_above: usize,
}

impl Default for Extract {
    fn default() -> Extract {
        Extract {
            settings: ExtractSettings::default(),
            preview: Preview::new(),
            shown: None,
            triangles: 0,
            error: None,
            building: None,
            copied: None,
            adding: None,
            next_look: 0.0,
            job_above: JOB_TRIANGLES,
        }
    }
}

/// The shell for one mesh, with the normals the preview is drawn with.
fn shell(mesh: &MeshView, settings: &ExtractSettings) -> Result<PreviewData, String> {
    let shell = extract(mesh.positions, mesh.normals, mesh.indices, mesh.mask, settings)?;
    Ok(with_normals(shell))
}

fn with_normals(shell: TriMesh) -> PreviewData {
    let normals = smooth_normals(&shell.positions, &shell.indices);
    PreviewData { positions: shell.positions, normals, indices: shell.indices }
}

impl Extract {
    pub fn settings(&self) -> &ExtractSettings {
        &self.settings
    }

    /// How many triangles the shell on screen has; 0 when there is none.
    pub fn preview_triangles(&self) -> usize {
        self.triangles
    }

    /// Why there is no shell to accept, when there is none.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// New settings; the preview follows on the next frame.
    pub fn set_settings(&mut self, settings: ExtractSettings) {
        self.settings = settings;
    }

    fn show(&mut self, source: Source, result: Result<PreviewData, String>) {
        self.shown = Some(source);
        match result {
            Ok(data) => {
                self.triangles = data.indices.len() / 3;
                self.error = None;
                self.preview.set_parts(data);
            }
            // The session stays open with nothing in it: the shell comes
            // back by itself once there is a mask to extract.
            Err(why) => {
                self.triangles = 0;
                self.error = Some(why);
                self.preview.clear();
            }
        }
    }

    /// Brings the preview up to date with the selected object, its mask and
    /// the settings. Cheap when nothing changed.
    pub(crate) fn refresh(&mut self, cx: &mut Ctx) {
        if self.building.is_some() {
            return;
        }
        let scene = cx.scene();
        let (Some(info), Some(mesh)) = (scene.active_info(), scene.active_mesh()) else { return };
        let aimed = self.shown.is_some_and(|s| s.object == info.id && s.mesh == info.mesh && s.settings == self.settings);
        if aimed && cx.time < self.next_look {
            // Looked a moment ago: come back when the next look is due.
            cx.repaint();
            return;
        }
        self.next_look = cx.time + LOOK_SECONDS;
        let source = Source { object: info.id, mesh: info.mesh, settings: self.settings, stamp: Stamp::of(&mesh) };
        if self.shown == Some(source) {
            return;
        }
        if mesh.triangle_count() <= self.job_above {
            let result = shell(&mesh, &self.settings);
            return self.show(source, result);
        }
        let key = (source.object, source.mesh, source.stamp);
        let copied = match &self.copied {
            Some((kept, copied)) if *kept == key => copied.clone(),
            _ => Arc::new(Copied {
                positions: mesh.positions.to_vec(),
                normals: mesh.normals.to_vec(),
                indices: mesh.indices.to_vec(),
                mask: mesh.mask.to_vec(),
            }),
        };
        self.copied = Some((key, copied.clone()));
        let settings = self.settings;
        let job = cx.spawn("Extract Preview", move |_| {
            let source = &*copied;
            extract(&source.positions, &source.normals, &source.indices, &source.mask, &settings).map(with_normals)
        });
        self.building = Some((source, job));
    }

    /// Turns the shell on screen into a real object: one undo step. It lands
    /// exactly where the preview showed it, and becomes the selected object,
    /// since it is nearly always what is worked on next.
    pub(crate) fn accept(&mut self, cx: &mut Ctx) {
        if self.adding.is_some() {
            return;
        }
        let info = self.shown.and_then(|source| cx.scene().info(source.object));
        let Some(info) = info.filter(|_| !self.preview.is_empty()) else {
            let why = self.error.clone().unwrap_or_else(|| "Nothing to extract yet.".to_owned());
            return cx.status(why, false);
        };
        let (name, placement) = (format!("{} Extract", info.name), info.placement);
        let data = self.preview.data();
        let mesh = TriMesh::new(data.positions.clone(), data.indices.clone());
        let triangles = mesh.triangle_count();
        if triangles <= self.job_above {
            self.add(cx, Made { name, placement, mesh: mesh.into(), triangles });
        } else {
            self.adding = Some(cx.spawn("Extract", move |job| {
                job.progress("Building");
                Ok(Made { name, placement, mesh: job.bake(mesh, None), triangles })
            }));
        }
        cx.finish();
    }

    fn add(&mut self, cx: &mut Ctx, made: Made) {
        let Made { name, placement, mesh, triangles } = made;
        let receipt = cx.commit("Extract", vec![Command::Add(NewObject::new(name, mesh).placed(placement))]);
        let added = receipt.added.first().and_then(|id| cx.scene().info(*id)).map(|info| info.name.to_owned());
        match added {
            Some(name) => {
                let text = format!("Extracted \"{name}\": {} triangles. Undo (Ctrl+Z) removes it.", theme::count(triangles));
                cx.status(text, true);
            }
            None => cx.status("The extract could not be added to the scene.", false),
        }
    }

    /// Waits for whatever is in flight: for `demo` and tests.
    pub(crate) fn settle(&mut self, cx: &mut Ctx) {
        if let Some((source, job)) = self.building.take() {
            let result = job.wait();
            self.show(source, result);
        }
        if let Some(job) = self.adding.take() {
            match job.wait() {
                Ok(made) => self.add(cx, made),
                Err(why) => cx.status(why, false),
            }
        }
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        match (&self.error, self.building.is_some()) {
            (_, true) => theme::line(ui, "Working out the preview...", theme::HINT()),
            (Some(why), _) => theme::line(ui, why, theme::ERROR),
            (None, _) if self.triangles > 0 => {
                theme::line(ui, &format!("Preview: {} tris. Accept to keep.", theme::count(self.triangles)), theme::OK);
            }
            _ => theme::line(ui, "Mask an area first. Accept makes it a new object.", theme::HINT()),
        }
    }

    fn sliders(&mut self, ui: &mut egui::Ui, cx: &Ctx) {
        let s = &mut self.settings;
        let percent = |v: f32| format!("{:.1}%", v * 100.0);
        let passes = |v: f32| format!("{}", v.round() as u32);
        let amount = |v: f32| format!("{v:.2}");

        let r = theme::slider(ui, "Thickness", &mut s.thickness_fraction, 0.002..=0.5, percent);
        theme::tip(r, cx.tips, "How thick the extracted shell is, as a fraction of the object's size.");
        let r = theme::slider(ui, "Offset (sink <-> float)", &mut s.offset_fraction, -0.25..=0.25, percent);
        theme::tip(r, cx.tips, "Moves the extracted shell in or out relative to the masked surface.");
        let r = theme::slider(ui, "Edge Falloff (slab <-> feathered)", &mut s.falloff_amount, 0.0..=1.0, amount);
        theme::tip(r, cx.tips, "How the shell thins toward its border: 0 is a flat slab edge, higher feathers it thinner.");

        let mut border = s.border_smoothing as f32;
        let r = theme::slider(ui, "Border Smoothing", &mut border, 0.0..=20.0, passes);
        theme::tip(r, cx.tips, "Smoothing passes applied to the extracted shell's border.");
        s.border_smoothing = border.round().clamp(0.0, 20.0) as u32;

        let mut surface = s.surface_smoothing as f32;
        let r = theme::slider(ui, "Surface Smoothing", &mut surface, 0.0..=20.0, passes);
        theme::tip(r, cx.tips, "Smoothing passes applied across the whole extracted surface.");
        s.surface_smoothing = surface.round().clamp(0.0, 20.0) as u32;

        let r = theme::slider(ui, "Shrinkwrap (inner face to body)", &mut s.shrinkwrap, 0.0..=1.0, amount);
        theme::tip(r, cx.tips, "Pulls the shell's inner face back onto the original body's surface.");
        let r = theme::slider(ui, "Mask Threshold", &mut s.mask_threshold, 0.05..=0.95, amount);
        theme::tip(r, cx.tips, "How strongly a vertex must be masked to be included in the extraction.");

        let r = theme::checkbox(ui, &mut s.invert_region, "Extract Unmasked Instead", ACCENT());
        theme::tip(r, cx.tips, "Extracts the unmasked area instead of the masked one.");
    }
}

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn Tool> {
    Box::new(Extract::default())
}

impl Tool for Extract {
    fn info(&self) -> Info {
        Info {
            id: "extract",
            name: "Extract",
            icon: "brush_layer",
            tip: "Pull the masked area out as its own solid with thickness: a plate, a strap, a piece of clothing.",
            hotkey: None,
        }
    }

    fn enter(&mut self, cx: &mut Ctx) {
        self.shown = None;
        self.next_look = 0.0;
        cx.status("The shell shows what Accept will make from the mask. Change the settings to reshape it.", true);
    }

    /// Putting the tool away discards the preview. The source was never
    /// changed, so there is nothing to roll back.
    fn leave(&mut self, _cx: &mut Ctx) {
        self.building = None;
        self.copied = None;
        self.shown = None;
        self.triangles = 0;
        self.preview.clear();
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::KeyDown(Key::Enter) => {
                self.accept(cx);
                true
            }
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        // An accepted shell being baked lands whether or not the tool is
        // still up.
        if let Some(result) = self.adding.as_mut().and_then(Job::poll) {
            self.adding = None;
            match result {
                Ok(made) => self.add(cx, made),
                Err(why) => cx.status(why, false),
            }
        }
        if !cx.active {
            return;
        }
        if let Some(result) = self.building.as_mut().and_then(|(_, job)| job.poll())
            && let Some((source, _)) = self.building.take()
        {
            self.show(source, result);
        }
        self.refresh(cx);
        // The shell is in the source's own space: drawn through the source's
        // placement it sits where the new object will.
        let placed = self.shown.and_then(|source| cx.scene().info(source.object)).map(|info| info.matrix());
        if let Some(model) = placed {
            draw.instances(&self.preview, Style::shell(PREVIEW), &[model]);
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let summary = if self.triangles > 0 { format!("{} tris", theme::count(self.triangles)) } else { String::new() };
        theme::category(ui, TITLE, CARD, &summary, true, |ui| {
            theme::hint(ui, "The masked area of the selected object, as a new solid. The see-through shell is the preview.");
            // Above the sliders, so the way out of the tool never scrolls
            // out of sight.
            self.status_line(ui);
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                ui.add_enabled_ui(self.triangles > 0 && self.adding.is_none(), |ui| {
                    let style = ButtonStyle { active: Some(ACCENT()), badge: "Enter", ..Default::default() };
                    let r = theme::button(ui, w, "Accept", style);
                    if theme::tip(r, cx.tips, "Turns the preview into a real, separate object. One undo step.").clicked() {
                        self.accept(cx);
                    }
                });
                let style = ButtonStyle { badge: "Esc", ..Default::default() };
                let r = theme::button(ui, w, "Cancel", style);
                if theme::tip(r, cx.tips, "Discards the preview without creating anything.").clicked() {
                    cx.finish();
                }
            });
            self.sliders(ui, cx);
            theme::hint(ui, "To change the mask, go back to Sculpt (M paints it), or use the Mask tool, then come back.");
        });
    }

    /// The preview on a masked patch of the selected object (masked here if
    /// nothing is). `made` goes on to accept it.
    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        crate::demo::extract(self, cx, what);
    }
}
