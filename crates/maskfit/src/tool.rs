//! The Mask Fit tool on screen: the head to fit, the two lengths, the marks
//! while it is up, and Refit Inside as one undo step.
//!
//! The look at the shell (`fit::survey`) is a background job, redone when the
//! shell, the head, where either stands or a setting changes. Nothing tells a
//! tool when vertices change, so the shell is looked at again on a timer, as
//! Extract does. The marks are drawn from the last look; Refit uses it too,
//! so it moves exactly the points the marks were worked out from.

use std::sync::Arc;

use egui::Color32;
use glam::{Mat4, Vec3};
use rayon::prelude::*;
use sculpt_geom::{TriMesh, units};
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};
use sculpt_toolkit::{
    Command, Ctx, Draw, Event, Info, Job, Key, MeshData, MeshId, MeshView, ObjectId, Preview, PreviewData, Scene, Style,
    Tool,
};

use crate::fit::{self, Face, Settings, Survey};

const TITLE: &str = "Mask Fit";
const CARD: Color32 = Color32::from_rgb(235, 170, 110);
/// Thin wall, on the outside.
const THIN: Color32 = Color32::from_rgb(230, 60, 50);
/// The inside too close to the face, seen through the shell.
const CLOSE: Color32 = Color32::from_rgba_premultiplied(120, 70, 10, 150);

/// How often the shell is looked at again for a change nothing announced.
const LOOK_SECONDS: f64 = 0.4;
/// How far from the head a point may be and still be taken for the inside,
/// in millimetres (more if the Gap asks for more): a sculpt can push the
/// inside well off the face before Refit puts it back.
const REACH_MM: f32 = 30.0;
/// How far a wall is looked for, in millimetres (more if Min Wall asks).
const WALL_REACH_MM: f32 = 12.0;
/// How far the marks are lifted off the surface, so they are not lost in it.
const LIFT_MM: f32 = 0.08;

/// What a look was taken of: when any of it differs from the scene now, the
/// marks are out of date.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Source {
    shell: ObjectId,
    shell_mesh: MeshId,
    head: ObjectId,
    head_mesh: MeshId,
    shell_place: Mat4,
    head_place: Mat4,
    settings: Settings,
    stamp: u64,
}

/// The lengths of the settings in the shell's own space.
#[derive(Clone, Copy, Debug)]
struct Local {
    gap: f32,
    min_wall: f32,
    reach: f32,
    wall_reach: f32,
    lift: f32,
}

impl Local {
    fn of(settings: Settings, shell_place: Mat4) -> Local {
        let (scale, _, _) = shell_place.to_scale_rotation_translation();
        let scale = scale.abs().max_element().max(1e-12);
        let local = |mm: f32| units::from_millimetres(mm) / scale;
        Local {
            gap: local(settings.gap_mm),
            min_wall: local(settings.min_wall_mm),
            reach: local(REACH_MM.max(settings.gap_mm * 3.0)),
            wall_reach: local(WALL_REACH_MM.max(settings.min_wall_mm * 3.0)),
            lift: local(LIFT_MM),
        }
    }
}

/// A finished look: the survey, the head it was against, and the marks.
struct Looked {
    face: Face,
    survey: Survey,
    local: Local,
    thin: PreviewData,
    close: PreviewData,
}

/// A fingerprint of a mesh's vertex positions, summed as integers so the
/// answer does not depend on how the work was shared between threads.
fn stamp(mesh: &MeshView) -> u64 {
    let shape = mesh
        .positions
        .par_iter()
        .enumerate()
        .with_min_len(1 << 14)
        .map(|(i, p)| {
            let bits = p.x.to_bits() as u64 ^ (p.y.to_bits() as u64).rotate_left(21) ^ (p.z.to_bits() as u64).rotate_left(42);
            bits.wrapping_mul((i as u64).wrapping_mul(2).wrapping_add(1))
        })
        .reduce(|| 0, u64::wrapping_add);
    shape ^ (mesh.vertex_count() as u64).rotate_left(7)
}

/// The triangles `marked` picks, as a mesh of their own, lifted off the
/// surface along the normals.
fn marks(positions: &[Vec3], normals: &[Vec3], indices: &[u32], lift: f32, marked: impl Fn(usize) -> bool) -> PreviewData {
    let mut out = PreviewData::default();
    let mut slot = vec![u32::MAX; positions.len()];
    for t in indices.chunks_exact(3) {
        if !t.iter().any(|&v| marked(v as usize)) {
            continue;
        }
        for &v in t {
            let v = v as usize;
            if slot[v] == u32::MAX {
                slot[v] = out.positions.len() as u32;
                let n = normals.get(v).copied().unwrap_or(Vec3::ZERO).normalize_or_zero();
                out.positions.push(positions[v] + n * lift);
                out.normals.push(n);
            }
            out.indices.push(slot[v]);
        }
    }
    out
}

/// The shell's arrays and the head's, copied for a job.
struct Copied {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
    head_positions: Vec<Vec3>,
    head_indices: Vec<u32>,
    to_shell: Mat4,
    local: Local,
}

fn look(c: &Copied) -> Looked {
    let face = Face::new(&c.head_positions, &c.head_indices, c.to_shell);
    let l = c.local;
    let survey = fit::survey(&c.positions, &c.normals, &c.indices, &face, l.reach, l.wall_reach);
    let thin = marks(&c.positions, &c.normals, &c.indices, l.lift, |v| survey.is_thin(v, l.min_wall));
    let close = marks(&c.positions, &c.normals, &c.indices, -l.lift, |v| survey.is_close(v, l.gap));
    Looked { face, survey, local: l, thin, close }
}

#[derive(Default)]
pub struct MaskFit {
    settings: Settings,
    /// The head the card names; `None` until one is picked (then the one
    /// overlapping the shell most is used).
    picked: Option<ObjectId>,
    shown: Option<(Source, Arc<Looked>)>,
    building: Option<(Source, Job<Looked>)>,
    thin_preview: Preview,
    close_preview: Preview,
    /// Why there is nothing to show, when there is nothing.
    error: Option<String>,
    next_look: f64,
}

/// The objects that could be the head: any other with a mesh of its own.
fn candidates(scene: &dyn Scene) -> Vec<(ObjectId, String)> {
    let active = scene.active();
    let Some(mine) = scene.info(active).map(|i| i.mesh) else { return Vec::new() };
    scene
        .objects()
        .into_iter()
        .filter(|&id| id != active)
        .filter_map(|id| scene.info(id).filter(|i| i.mesh != mine).map(|i| (id, i.name.to_owned())))
        .collect()
}

/// How much two objects' boxes in the scene overlap, as the volume shared.
fn overlap(scene: &dyn Scene, a: ObjectId, b: ObjectId) -> f32 {
    let (a, b) = (scene.bounds(a), scene.bounds(b));
    let size = (a.max.min(b.max) - a.min.max(b.min)).max(Vec3::ZERO);
    size.x * size.y * size.z
}

impl MaskFit {
    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings.fixed();
    }

    /// The last finished look, if it is of the scene as it is now.
    pub fn survey(&self) -> Option<&Survey> {
        self.shown.as_ref().map(|(_, l)| &l.survey)
    }

    /// The head the shell is checked against: the one picked, if it is still
    /// there, else the other object overlapping the shell most.
    pub fn head(&self, scene: &dyn Scene) -> Option<ObjectId> {
        let others = candidates(scene);
        if let Some(p) = self.picked.filter(|p| others.iter().any(|(id, _)| id == p)) {
            return Some(p);
        }
        let active = scene.active();
        others
            .iter()
            .map(|(id, _)| (*id, overlap(scene, active, *id)))
            .filter(|(_, v)| *v > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// What a look now would be taken of; or why there can be none.
    fn source(&self, scene: &dyn Scene) -> Result<Source, &'static str> {
        let shell = scene.active_info().ok_or("Select the mask's shell.")?;
        let head = self.head(scene).ok_or(
            "Needs the head the mask is worn on, overlapping the mask: import it (Mask Head.stl) or move it under the mask.",
        )?;
        let head_info = scene.info(head).ok_or("The head has gone.")?;
        let mesh = scene.active_mesh().ok_or("Select the mask's shell.")?;
        Ok(Source {
            shell: shell.id,
            shell_mesh: shell.mesh,
            head,
            head_mesh: head_info.mesh,
            shell_place: shell.matrix(),
            head_place: head_info.matrix(),
            settings: self.settings,
            stamp: stamp(&mesh),
        })
    }

    pub(crate) fn refresh(&mut self, cx: &mut Ctx) {
        if self.building.is_some() {
            return;
        }
        if cx.time < self.next_look {
            cx.repaint();
            return;
        }
        self.next_look = cx.time + LOOK_SECONDS;
        let source = match self.source(cx.scene()) {
            Ok(s) => s,
            Err(why) => {
                self.shown = None;
                self.thin_preview.clear();
                self.close_preview.clear();
                self.error = Some(why.to_owned());
                return;
            }
        };
        if self.shown.as_ref().is_some_and(|(s, _)| *s == source) {
            return;
        }
        let scene = cx.scene();
        let (Some(mesh), Some(head)) = (scene.active_mesh(), scene.mesh(source.head)) else { return };
        let copied = Copied {
            positions: mesh.positions.to_vec(),
            normals: mesh.normals.to_vec(),
            indices: mesh.indices.to_vec(),
            head_positions: head.positions.to_vec(),
            head_indices: head.indices.to_vec(),
            to_shell: source.shell_place.inverse() * source.head_place,
            local: Local::of(source.settings, source.shell_place),
        };
        let job = cx.spawn("Mask Fit", move |_| Ok::<Looked, String>(look(&copied)));
        self.building = Some((source, job));
    }

    fn show(&mut self, source: Source, looked: Looked) {
        self.error = (looked.survey.inside_count() == 0).then(|| {
            "No part of the selected object faces the head closely enough to be a mask's inside. Select the shell, \
             with the head under it."
                .to_owned()
        });
        self.thin_preview.set_parts(looked.thin.clone());
        self.close_preview.set_parts(looked.close.clone());
        self.shown = Some((source, Arc::new(looked)));
    }

    /// Moves the inside to the head plus the Gap: one undo step.
    pub(crate) fn refit(&mut self, cx: &mut Ctx) {
        let current = self.source(cx.scene());
        let looked = match (&self.shown, current) {
            (Some((s, l)), Ok(now)) if *s == now => l.clone(),
            (_, Err(why)) => return cx.status(why, false),
            _ => return cx.status("Still checking the mask; Refit again in a moment.", false),
        };
        if looked.survey.inside_count() == 0 {
            return cx.status(self.error.clone().unwrap_or_default(), false);
        }
        let Some((source, _)) = &self.shown else { return };
        let source = *source;
        let Some(mesh) = cx.scene().mesh(source.shell) else { return };
        let l = looked.local;
        let moved = fit::refit(mesh.positions, mesh.mask, mesh.hidden_vertices, &looked.survey, &looked.face, l.gap, l.reach);
        let count = moved.iter().zip(mesh.positions).filter(|(a, b)| a != b).count();
        let data = MeshData::masked(TriMesh::new(moved, mesh.indices.to_vec()), mesh.mask.to_vec());
        let receipt = cx.commit("Refit Inside", vec![Command::Replace { object: source.shell, mesh: data }]);
        if receipt.changed {
            let gap = units::format(units::from_millimetres(source.settings.gap_mm), cx.unit, true);
            cx.status(format!("Refitted the inside: {count} points now sit {gap} off the head. Undo (Ctrl+Z) puts them back."), true);
        } else {
            cx.status("The inside could not be refitted.", false);
        }
        self.next_look = 0.0;
    }

    /// Waits for a look in flight: for tests.
    pub fn settle(&mut self, cx: &mut Ctx) {
        self.next_look = 0.0;
        self.refresh(cx);
        if let Some((source, job)) = self.building.take()
            && let Ok(looked) = job.wait()
        {
            self.show(source, looked);
        }
    }

    pub fn refit_now(&mut self, cx: &mut Ctx) {
        self.refit(cx);
    }

    fn report(&self, ui: &mut egui::Ui, cx: &Ctx) {
        if self.building.is_some() && self.shown.is_none() {
            return theme::line(ui, "Checking the mask...", theme::HINT());
        }
        if let Some(why) = &self.error {
            return theme::line(ui, why, theme::ERROR);
        }
        let Some((source, looked)) = &self.shown else { return };
        let l = looked.local;
        let (scale, _, _) = source.shell_place.to_scale_rotation_translation();
        let world = |v: f32| units::format(v * scale.abs().max_element(), cx.unit, true);
        let thin = looked.survey.thin_count(l.min_wall);
        let close = looked.survey.close_count(l.gap);
        let thinnest = looked.survey.thinnest().map_or("none found".to_owned(), world);
        if thin == 0 {
            theme::line(ui, &format!("Wall fine everywhere: thinnest {thinnest}."), theme::OK);
        } else {
            theme::line(ui, &format!("Thinnest wall {thinnest}; {thin} points under Min Wall (red)."), THIN);
        }
        if close > 0 {
            theme::line(ui, &format!("{close} inside points too close to the face (orange): Refit Inside."), CLOSE);
        }
    }

    fn sliders(&mut self, ui: &mut egui::Ui, cx: &Ctx) {
        let unit = cx.unit;
        let slider = |ui: &mut egui::Ui, label: &str, mm: &mut f32, range: std::ops::RangeInclusive<f32>| {
            let to = |mm: f32| units::to_unit(units::from_millimetres(mm), unit);
            let mut v = to(*mm);
            let readout = |v: f32| units::format(units::from_unit(v, unit), unit, true);
            let r = theme::slider(ui, label, &mut v, to(*range.start())..=to(*range.end()), readout);
            if r.changed() {
                *mm = units::to_millimetres(units::from_unit(v, unit)).clamp(*range.start(), *range.end());
            }
            r
        };
        let s = &mut self.settings;
        let r = slider(ui, "Gap", &mut s.gap_mm, Settings::GAP_RANGE);
        theme::tip(r, cx.tips, "How far the inside sits off the skin: room for comfort and padding. Double tap to type a length.");
        let r = slider(ui, "Min Wall", &mut s.min_wall_mm, Settings::WALL_RANGE);
        theme::tip(r, cx.tips, "The thinnest wall you want to print. Thinner places show red. Double tap to type a length.");
    }
}

/// A ball of `radius`, triangles facing out (or in, `inward`), keeping only
/// the triangles whose corners all lie in front of `front` along +Z.
fn ball(radius: f32, inward: bool, front: f32) -> TriMesh {
    let (rings, segs) = (48usize, 72usize);
    let mut p = vec![Vec3::Y * radius];
    for i in 1..rings {
        let th = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..segs {
            let ph = std::f32::consts::TAU * j as f32 / segs as f32;
            p.push(Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin()) * radius);
        }
    }
    p.push(-Vec3::Y * radius);
    let at = |i: usize, j: usize| (1 + (i - 1) * segs + j % segs) as u32;
    let last = (p.len() - 1) as u32;
    let mut t = Vec::new();
    for j in 0..segs {
        t.extend([0, at(1, j + 1), at(1, j)]);
        t.extend([last, at(rings - 1, j), at(rings - 1, j + 1)]);
    }
    for i in 1..rings - 1 {
        for j in 0..segs {
            t.extend([at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
            t.extend([at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
        }
    }
    if inward {
        t.chunks_exact_mut(3).for_each(|c| c.swap(1, 2));
    }
    let t: Vec<u32> = t.chunks_exact(3).filter(|c| c.iter().all(|&i| p[i as usize].z > front * radius)).flatten().copied().collect();
    TriMesh::new(p, t)
}

impl Tool for MaskFit {
    fn info(&self) -> Info {
        Info {
            id: "maskfit",
            name: "Mask Fit",
            icon: "tool_mask",
            tip: "Check a mask to wear against the head it goes on, and put its inside back to fit after sculpting.",
            hotkey: None,
        }
    }

    fn enter(&mut self, cx: &mut Ctx) {
        self.shown = None;
        self.next_look = 0.0;
        cx.status("Red: wall thinner than Min Wall. Orange: inside too close to the face. Refit Inside fixes the fit.", true);
    }

    fn leave(&mut self, _cx: &mut Ctx) {
        self.building = None;
        self.shown = None;
        self.thin_preview.clear();
        self.close_preview.clear();
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::KeyDown(Key::Enter) => {
                self.refit(cx);
                true
            }
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        if !cx.active {
            return;
        }
        if let Some(result) = self.building.as_mut().and_then(|(_, job)| job.poll())
            && let Some((source, _)) = self.building.take()
        {
            match result {
                Ok(looked) => self.show(source, looked),
                Err(why) => self.error = Some(why),
            }
        }
        self.refresh(cx);
        let placed = self.shown.as_ref().and_then(|(s, _)| cx.scene().info(s.shell)).map(|info| info.matrix());
        if let Some(model) = placed {
            draw.instances(&self.thin_preview, Style::solid(THIN), &[model]);
            draw.instances(&self.close_preview, Style::shell(CLOSE).on_top(), &[model]);
        }
    }

    /// A head (a ball 200 mm across) and the front of a shell over it, its
    /// wall 2.5 mm but pushed in to a millimetre over the brow and its inside
    /// pressed onto the face at the chin, so both marks show.
    fn demo(&mut self, cx: &mut Ctx, _what: &str) {
        let r = units::from_millimetres(100.0);
        let mm = units::from_millimetres(1.0);
        let head = ball(r, false, -2.0);
        let mut shell = ball(r + 3.0 * mm, true, 0.15);
        let outer = ball(r + 5.5 * mm, false, 0.15);
        let base = shell.positions.len() as u32;
        let inner_count = shell.positions.len();
        shell.positions.extend(outer.positions);
        shell.indices.extend(outer.indices.iter().map(|i| i + base));
        for (v, p) in shell.positions.iter_mut().enumerate() {
            let d = p.normalize();
            if v >= inner_count && (d - Vec3::new(0.0, 0.45, 0.89).normalize()).length() < 0.25 {
                *p = d * (r + 4.0 * mm);
            }
            if v < inner_count && (d - Vec3::new(0.0, -0.5, 0.87).normalize()).length() < 0.2 {
                *p = d * (r + 0.5 * mm);
            }
        }
        let receipt = cx.commit(
            "Mask Fit demo",
            vec![Command::Add(sculpt_toolkit::NewObject::new("Head", head)), Command::Add(sculpt_toolkit::NewObject::new("Mask", shell))],
        );
        if let Some(&mask) = receipt.added.last() {
            cx.commit("Select", vec![Command::Select { object: mask }]);
            cx.frame_view(cx.scene().bounds(mask));
        }
        self.settle(cx);
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let summary = self.shown.as_ref().and_then(|(_, l)| l.survey.thinnest()).map_or(String::new(), |t| {
            let (scale, _, _) = self.shown.as_ref().map(|(s, _)| s.shell_place.to_scale_rotation_translation()).unwrap_or_default();
            format!("wall {}", units::format(t * scale.abs().max_element(), cx.unit, true))
        });
        theme::category(ui, TITLE, CARD, &summary, true, |ui| {
            theme::hint(ui, "Select the mask's shell. The head it is worn on is checked against it.");
            self.report(ui, cx);
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                let ready = self.shown.as_ref().is_some_and(|(_, l)| l.survey.inside_count() > 0);
                ui.add_enabled_ui(ready, |ui| {
                    let style = ButtonStyle { active: Some(ACCENT()), badge: "Enter", ..Default::default() };
                    let r = theme::button(ui, w, "Refit Inside", style);
                    let tip = "Moves the inside of the mask back to the head plus the Gap. The outside, your sculpting, \
                               stays. One undo step (Ctrl+Z). Subdivision levels of the shell are not kept.";
                    if theme::tip(r, cx.tips, tip).clicked() {
                        self.refit(cx);
                    }
                });
                let style = ButtonStyle { badge: "Esc", ..Default::default() };
                let r = theme::button(ui, w, "Done", style);
                if theme::tip(r, cx.tips, "Puts the tool away. Nothing is changed by looking.").clicked() {
                    cx.finish();
                }
            });
            let scene = cx.scene();
            let others = candidates(scene);
            let head = self.head(scene);
            let shown = head.and_then(|h| others.iter().find(|(id, _)| *id == h)).map_or("Choose the head", |(_, n)| n.as_str());
            let mut chosen = None;
            ui.horizontal(|ui| {
                ui.label("Head");
                egui::ComboBox::from_id_salt("maskfit_head").selected_text(shown).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                    for (id, name) in &others {
                        if ui.selectable_label(head == Some(*id), name).clicked() {
                            chosen = Some(*id);
                        }
                    }
                });
            });
            if chosen.is_some() {
                self.picked = chosen;
                self.next_look = 0.0;
            }
            self.sliders(ui, cx);
            theme::hint(ui, "Making a mask: mask the area on the head, Extract it, sculpt the shell, then come here.");
        });
    }
}
