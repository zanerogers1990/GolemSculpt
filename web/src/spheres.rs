//! SSpheres on the web: the desktop's blockout tool (`src/ssphere/`, compiled
//! in as it is), with the app's side of it (`src/app/spheres.rs`) and its
//! panel (`src/ui/spheres.rs`) written again for the web page.
//!
//! Sketch a form out of connected spheres, watch a skin wrap over them, then
//! Make Skin turns it into an ordinary object to sculpt. The rig keeps its own
//! undo history (Ctrl+Z steps it back while the tool is up, then falls
//! through to the scene's), and is not kept with the scene, as on the
//! desktop. The armature and the skin are drawn by the desktop's own layer
//! (`spheres_layer.rs`).
//!
//! Left out: the tool's part of the hold-Space wheel (the panel and keys do
//! the same).

use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, Key, Modifiers, PointerButton, Pos2, Sense, Stroke as Line, Ui, vec2};
use glam::{Vec2, Vec3};

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use super::{Drag, GolemApp, Mode, Step};
use crate::gizmo::{self, Frame as ArrowFrame, Orientation, Target};
use crate::mesh::Mesh;
use crate::render::spheres_layer::{Draw, SkinLook};
use crate::scene::{Object, Props};
use crate::ssphere::skin::{self, Skin};
use crate::ssphere::{Body, Ctx, EditMode, Hit, MIN_NODE_RADIUS, Press, SSpheres};

/// Largest radius the size sliders reach; the wheel and Scale mode go further.
const SLIDER_MAX_RADIUS: f32 = 2.0;
/// Two F presses closer than this frame the spheres.
const DOUBLE_TAP: f64 = 0.4;
/// Pixels of a smooth (trackpad) scroll that count as one wheel notch.
const PX_PER_NOTCH: f32 = 50.0;

/// The tool and what the web keeps beside it.
#[derive(Default)]
pub(crate) struct Spheres {
    pub tool: SSpheres,
    /// The live skin as last handed to the renderer, with its generation.
    drawn_skin: Option<(u64, Arc<Skin>)>,
    /// A drag of one of the gizmo's arrows on the selected sphere.
    arrow: Option<gizmo::Drag>,
    /// S is held: the next sphere's size follows the pointer.
    gauging: bool,
    last_f: Option<f64>,
    /// The tool was up last frame (to put it away when it no longer is).
    was_up: bool,
    /// The pointer is over the viewport, as of this frame.
    inside: bool,
    /// Scroll not yet a whole notch.
    scroll: f32,
}

/// Where a ray from the camera first meets a shown object, its normal there
/// (world) and which object (the desktop's `Scene::pick`). `skip` is the skin
/// standing aside for the live one.
fn surface(objects: &[Object], mesh: &Mesh, active: u64, skip: Option<u64>, origin: Vec3, dir: Vec3) -> Option<Hit> {
    let mut best: Option<(Hit, f32)> = None;
    for o in objects.iter().filter(|o| o.visible && Some(o.id) != skip) {
        let own = if o.id == active { mesh } else { o.mesh.as_ref().unwrap_or(mesh) };
        for (_, m) in o.placements() {
            let inv = m.inverse();
            let (lo, ld) = (inv.transform_point3(origin), inv.transform_vector3(dir));
            let len = ld.length();
            if len < 1e-12 || !len.is_finite() {
                continue;
            }
            let Some(h) = own.raycast(lo, ld / len) else { continue };
            let point = m.transform_point3(h.pos);
            let normal = inv.transpose().transform_vector3(h.normal).normalize_or_zero();
            let along = (point - origin).dot(dir);
            if best.as_ref().is_none_or(|(_, b)| along < *b) {
                best = Some((Hit { point, normal, body: Body::of(o) }, along));
            }
        }
    }
    best.map(|(hit, _)| hit)
}

fn fmt_count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.0}k", n as f32 / 1e3),
        _ => format!("{:.1}M", n as f32 / 1e6),
    }
}

fn line(ui: &mut Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(11.0)).color(color)).wrap());
}

/// A slider over a radius where equal travel is an equal proportional change.
fn radius_slider(ui: &mut Ui, label: &str, radius: f32) -> Option<f32> {
    let mut log = radius.log2();
    let range = MIN_NODE_RADIUS.log2()..=SLIDER_MAX_RADIUS.log2();
    theme::slider(ui, label, &mut log, range, |v| format!("{:.3}", v.exp2())).changed().then(|| log.exp2())
}

impl GolemApp {
    /// Runs `f` on the tool with what its gestures need from the app.
    fn with_spheres<R>(&mut self, f: impl FnOnce(&mut SSpheres, &Ctx) -> R) -> R {
        let up = self.mode == Mode::Spheres;
        let (objects, mesh, active) = (&self.objects, &self.mesh, self.active);
        let tool = &mut self.spheres.tool;
        // A gesture must see the rig where its object is now.
        tool.follow(|id| objects.iter().find(|o| o.id == id).map(Body::of));
        let skip = if up && !tool.rig.is_empty() { tool.skin_object } else { None };
        let surface = |origin: Vec3, dir: Vec3| surface(objects, mesh, active, skip, origin, dir);
        let body = objects.iter().find(|o| o.id == active).map(Body::of);
        let cx = Ctx { view: &self.view, pivot: self.camera.pivot, surface: &surface, body };
        f(tool, &cx)
    }

    /// The first sphere of a rig is sized off what is already in the scene.
    fn spheres_size_from_scene(&mut self) {
        let scene = self.scene_bounds();
        if !scene.is_empty() {
            self.spheres.tool.default_radius = ((scene.size() * 0.5).element_sum() / 3.0 * 0.35).clamp(0.02, 1.0);
        }
    }

    /// Brings the tool up (or puts it away).
    pub(crate) fn toggle_spheres(&mut self) {
        if self.drag != Drag::None {
            return;
        }
        if self.mode == Mode::Spheres {
            self.mode = Mode::Sculpt;
            self.status = "Sculpt".into();
        } else {
            self.mode = Mode::Spheres;
            self.status = "SSpheres".into();
        }
    }

    /// New SSphere Rig: brings the tool up and places the first sphere where
    /// the camera is looking, on the selected object's mirror plane, resting
    /// on the object if it is in the way (and then attached to it). Never
    /// clears spheres that are already there.
    pub(crate) fn spheres_new_rig(&mut self, now: f64) {
        if self.drag != Drag::None {
            return;
        }
        self.mode = Mode::Spheres;
        self.spheres_size_from_scene();
        let was_attached = self.spheres.tool.attached_to();
        let started = self.with_spheres(|s, cx| {
            s.tick(now);
            s.start_new_rig(cx)
        });
        let landed_on = self.spheres.tool.attached_to().filter(|&id| was_attached != Some(id));
        let model = landed_on.and_then(|id| self.objects.iter().find(|o| o.id == id)).map(|o| o.name.clone());
        let text = match (started, model) {
            (false, _) => "There are spheres already: drag off one, or use Clear All to start over.".to_owned(),
            (true, Some(name)) => format!(
                "Started a rig with one sphere, resting on \"{name}\" and attached to it. Drag off the sphere to pull out the next one."
            ),
            (true, None) => "Started a rig with one sphere. Drag off it to pull out the next one.".to_owned(),
        };
        self.spheres.tool.say(text);
    }

    /// The gizmo's arrows on the selected sphere, in Move mode.
    fn spheres_arrows(&self) -> Option<(Target, ArrowFrame)> {
        let (target, handles) = self.spheres.tool.gizmo()?.target();
        let frame = ArrowFrame::on(&[target], Orientation::default(), handles, &self.view, self.transform_ppp())?;
        Some((target, frame))
    }

    /// An arrow under pixel `px`, unless a sphere is there: the sphere is the
    /// sphere, and the arrows are the arrows only where they stick out past it.
    fn spheres_arrow_at(&mut self, px: Vec2) -> Option<(gizmo::Handle, Target, ArrowFrame)> {
        let (target, frame) = self.spheres_arrows()?;
        let handle = frame.pick(&self.view, px, self.transform_ppp())?;
        if self.with_spheres(|s, cx| s.claims_pointer(cx, px)) {
            return None;
        }
        Some((handle, target, frame))
    }

    /// A press on the viewport while the tool is up.
    pub(super) fn spheres_press(&mut self, px: Vec2, button: PointerButton, mods: Modifiers) -> Drag {
        if button == PointerButton::Primary {
            if let Some((handle, target, frame)) = self.spheres_arrow_at(px) {
                if self.spheres.tool.begin_gizmo_drag() {
                    self.spheres.arrow = Some(gizmo::Drag::on(handle, frame, &[target], &self.view, px));
                    return Drag::Tool;
                }
            }
        }
        let delete = button == PointerButton::Secondary || mods.ctrl || mods.command;
        match self.with_spheres(|s, cx| s.press(cx, px, mods.shift, delete)) {
            Press::Drag => Drag::Tool,
            // Empty space: the camera turns, unless it turns out to be a click.
            Press::Empty if button == PointerButton::Primary => Drag::Orbit,
            Press::Empty | Press::Done => Drag::None,
        }
    }

    /// The pointer moved. True when the tool used the movement.
    pub(super) fn spheres_moved(&mut self, px: Vec2) -> bool {
        if self.mode != Mode::Spheres {
            return false;
        }
        if self.spheres.tool.gauge_move(px) {
            return true;
        }
        if self.drag == Drag::Tool {
            if let Some(mut arrow) = self.spheres.arrow.take() {
                arrow.update(&self.view, px);
                self.spheres.tool.gizmo_drag_to(arrow.placements()[0].position);
                self.spheres.arrow = Some(arrow);
                return true;
            }
        }
        matches!(self.drag, Drag::Tool | Drag::Orbit) && self.with_spheres(|s, cx| s.drag_to(cx, px))
    }

    pub(super) fn spheres_released(&mut self) {
        if self.mode != Mode::Spheres {
            return;
        }
        if self.spheres.arrow.take().is_some() {
            self.spheres.tool.end_gizmo_drag();
            return;
        }
        if matches!(self.drag, Drag::Tool | Drag::Orbit) {
            self.with_spheres(|s, cx| s.release(cx));
        }
    }

    /// The wheel while the tool is up: over a sphere (or while one is being
    /// dragged) it resizes it, one step a notch. True when the wheel was the
    /// tool's, so the view does not zoom too.
    pub(super) fn spheres_wheel(&mut self, events: &[egui::Event]) -> bool {
        if self.mode != Mode::Spheres {
            return false;
        }
        let s = &mut self.spheres;
        let on_sphere = s.tool.hovered.is_some() || s.tool.is_dragging();
        if !on_sphere {
            s.scroll = 0.0;
            return false;
        }
        for event in events {
            if let egui::Event::MouseWheel { unit, delta, .. } = event {
                s.scroll += match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Page => delta.y * 3.0,
                    egui::MouseWheelUnit::Point => delta.y / PX_PER_NOTCH,
                };
            }
        }
        while s.scroll.abs() >= 1.0 {
            let notch = s.scroll.signum();
            s.scroll -= notch;
            s.tool.wheel(notch);
        }
        true
    }

    /// The tool's keys: 1 to 4 the modes, S held sizes the next sphere, F F
    /// frames the spheres, A the solid preview, X symmetry, Delete, Esc. The
    /// brush keys have nothing to act on while it is up, so they are taken
    /// too. Ctrl shortcuts stay the app's.
    pub(super) fn spheres_keys(&mut self, ctx: &egui::Context) {
        let idle = self.drag == Drag::None;
        let now = ctx.input(|i| i.time);
        let pointer = self.pointer_px.unwrap_or(self.view.origin + self.view.size * 0.5);
        let pressed =|key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key) || i.consume_key(Modifiers::SHIFT, key));
        let modes = [Key::Num1, Key::Num2, Key::Num3, Key::Num4];
        for (i, key) in modes.into_iter().enumerate() {
            if pressed(key) && idle {
                self.spheres.tool.set_mode(EditMode::ALL[i]);
                self.status = EditMode::ALL[i].name().into();
            }
        }
        if pressed(Key::S) && idle && !self.spheres.gauging {
            self.spheres.tool.gauge_begin(pointer);
            self.spheres.gauging = true;
        }
        if self.spheres.gauging && !ctx.input(|i| i.key_down(Key::S)) {
            self.spheres.tool.gauge_end();
            self.spheres.gauging = false;
        }
        if pressed(Key::F) && idle {
            // F F frames, as when sculpting; a lone F has no meaning here.
            if self.spheres.last_f.is_some_and(|t| now - t < DOUBLE_TAP) {
                self.spheres.last_f = None;
                self.spheres_frame_view();
            } else {
                self.spheres.last_f = Some(now);
            }
        }
        if pressed(Key::A) && idle {
            let s = &mut self.spheres.tool;
            s.preview = !s.preview;
            self.status = if s.preview { "Solid Preview" } else { "Spheres" }.into();
        }
        if pressed(Key::X) && idle {
            let s = &mut self.spheres.tool;
            let on = !s.symmetry();
            let report = s.set_symmetry(on);
            s.say(report);
            self.status = if on { "Symmetry On" } else { "Symmetry Off" }.into();
        }
        if (pressed(Key::Delete) | pressed(Key::Backspace)) && idle {
            self.spheres.tool.delete_selected();
        }
        if pressed(Key::Escape) {
            // Mid-drag: put the rig back. Otherwise let go of the selection,
            // and failing that put the tool away (the rig stays).
            let had_arrow = self.spheres.arrow.take().is_some();
            let taken = self.spheres.tool.escape() || had_arrow;
            if !taken && idle {
                self.mode = Mode::Sculpt;
                self.status = "Sculpt".into();
            }
        }
        // The brush keys have nothing to act on while the tool is up.
        for key in [Key::D, Key::M, Key::R, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9, Key::Num0, Key::Space] {
            pressed(key);
        }
    }

    /// Frames the rig rather than the active object.
    fn spheres_frame_view(&mut self) {
        if self.spheres.tool.rig.is_empty() {
            return self.frame_model(true);
        }
        let aspect = (self.view.size.x / self.view.size.y.max(1.0)).max(0.1);
        let bounds = self.spheres.tool.bounds();
        self.camera.frame(bounds, aspect, true);
    }

    /// Ctrl+Z while the tool is up. The rig has its own history; with nothing
    /// left there, the press falls through to the scene's (false).
    pub(super) fn spheres_undo(&mut self, redo: bool) -> bool {
        if self.mode != Mode::Spheres {
            return false;
        }
        let s = &mut self.spheres.tool;
        if s.is_dragging() {
            // Swallowed: stepping the rig back under a live drag would leave
            // the drag holding start state for spheres that moved.
            return true;
        }
        if !(if redo { s.redo() } else { s.undo() }) {
            return false;
        }
        self.status = if redo { "Redo" } else { "Undo" }.into();
        true
    }

    /// Once a frame, before drawing: lets the tool follow the pointer and
    /// keep its skin current. `inside` is whether the pointer is over the
    /// viewport (not a panel); with `alt` held the camera has it.
    pub(super) fn spheres_frame(&mut self, inside: bool, alt: bool, now: f64) {
        let up = self.mode == Mode::Spheres;
        // With the tool put away too: an attached rig keeps up with its object.
        let objects = &self.objects;
        self.spheres.tool.follow(|id| objects.iter().find(|o| o.id == id).map(Body::of));
        if !up {
            if self.spheres.was_up {
                self.spheres.was_up = false;
                self.spheres.arrow = None;
                self.spheres.gauging = false;
                self.spheres.tool.leave();
            }
            return;
        }
        self.spheres.was_up = true;
        self.spheres.inside = inside;
        let pos = self.pointer_px.unwrap_or(Vec2::splat(-1e6));
        let on_arrow = self.drag == Drag::None && self.pointer_px.is_some_and(|px| self.spheres_arrow_at(px).is_some());
        let blocked = !inside || self.pointer_px.is_none() || alt || on_arrow || !matches!(self.drag, Drag::None | Drag::Tool);
        self.spheres_size_from_scene();
        self.with_spheres(|s, cx| {
            s.tick(now);
            s.hover(cx, pos, blocked);
            s.refresh_skin(false);
        });
    }

    /// What the renderer draws of the tool this frame.
    pub(super) fn spheres_draw(&mut self) -> Option<Draw> {
        if self.mode != Mode::Spheres {
            return None;
        }
        let s = &mut self.spheres;
        let skin = match s.tool.skin() {
            Some((skin, generation)) => {
                if s.drawn_skin.as_ref().is_none_or(|(g, _)| *g != generation) {
                    let copy = Skin {
                        positions: skin.positions.clone(),
                        normals: skin.normals.clone(),
                        indices: skin.indices.clone(),
                        stats: skin.stats,
                    };
                    s.drawn_skin = Some((generation, Arc::new(copy)));
                }
                s.drawn_skin.as_ref().map(|(g, skin)| (skin.clone(), *g))
            }
            None => None,
        };
        let look = if s.tool.preview { SkinLook::Solid } else { SkinLook::Shell };
        Some(Draw {
            view: self.view,
            armature: s.tool.armature(),
            skin,
            look,
            lift: s.tool.shell_lift(),
            model: s.tool.frame().matrix(),
        })
    }

    /// Whether object `id` is left out of the viewport: the skin made from the
    /// rig, while the rig is being edited. The live skin stands in for it.
    pub(crate) fn spheres_stand_in_for(&self, id: u64) -> bool {
        let s = &self.spheres.tool;
        self.mode == Mode::Spheres && !s.rig.is_empty() && s.skin_object == Some(id)
    }

    /// Draws the gizmo's arrows on the selected sphere.
    pub(super) fn draw_spheres_gizmo(&mut self, painter: &egui::Painter, ppp: f32) {
        if self.mode != Mode::Spheres {
            return;
        }
        let (frame, hot) = match &self.spheres.arrow {
            Some(d) => (d.shown_frame(), Some(d.handle)),
            None => {
                let Some((_, frame)) = self.spheres_arrows() else { return };
                let hot = self.pointer_px.filter(|_| self.drag == Drag::None).and_then(|px| self.spheres_arrow_at(px)).map(|(h, ..)| h);
                (frame, hot)
            }
        };
        let to_pt = |p: &Vec2| Pos2::new(p.x / ppp, p.y / ppp);
        let rgba = |c: [u8; 4]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        for shape in frame.shapes(&self.view, hot, ppp) {
            match shape {
                gizmo::Shape::Line { points, width, color } => {
                    let points: Vec<Pos2> = points.iter().map(to_pt).collect();
                    painter.add(egui::Shape::line(points, Line::new(width / ppp, rgba(color))));
                }
                gizmo::Shape::Fill { points, color } => {
                    let points: Vec<Pos2> = points.iter().map(to_pt).collect();
                    painter.add(egui::Shape::convex_polygon(points, rgba(color), Line::NONE));
                }
            }
        }
    }

    /// Bakes the skin into a real object: a new entry in the scene, active
    /// and ready to sculpt, with its pivot in the middle of the skin. A skin
    /// made earlier from the same spheres is hidden rather than deleted,
    /// since it may have been sculpted on. One undo step.
    fn spheres_make_skin(&mut self) {
        match self.spheres.tool.make_skin() {
            Ok(made) => {
                let skin = made.skin;
                let (tris, ms) = (skin.stats.triangles, skin.stats.ms);
                let mesh = Mesh::new(skin.positions, skin.indices, None);
                let earlier = self.spheres.tool.skin_object.and_then(|id| self.objects.iter().find(|o| o.id == id)).filter(|o| o.visible);
                let earlier = earlier.map(|o| (o.id, o.name.clone(), o.props()));
                let hidden = earlier.as_ref().map(|(id, _, before)| {
                    if let Some(o) = self.objects.iter_mut().find(|o| o.id == *id) {
                        o.visible = false;
                    }
                    Step::Props(vec![(*id, before.clone())])
                });
                let props = Props { position: made.position, rotation: made.rotation, scale: made.scale, ..Props::named("SSphere Mesh") };
                let id = self.add_object(mesh, props, true);
                // Hiding the earlier skin and adding this one are one step.
                if let Some(hidden) = hidden {
                    if let Some(added) = self.undo.pop() {
                        self.undo.push(Step::Many(vec![hidden, added]));
                    }
                }
                self.selected = vec![id];
                let s = &mut self.spheres.tool;
                s.skin_object = s.keep_rig.then_some(id);
                // Straight into sculpting what was just made.
                self.mode = Mode::Sculpt;
                let name = self.active_object().name.clone();
                let mut text = format!("Made \"{name}\" from the spheres: {tris} triangles in {ms:.0} ms.");
                text += &match earlier {
                    Some((_, old, _)) => format!(" The earlier skin \"{old}\" is hidden, not deleted."),
                    None => " Undo (Ctrl+Z) removes it.".to_owned(),
                };
                self.status = text;
            }
            Err(e) => {
                self.spheres.tool.say(e.clone());
                self.status = e;
            }
        }
    }

    /// New SSphere Rig, for wherever a model can be started from.
    pub(super) fn new_rig_button(&mut self, ui: &mut Ui) {
        let style = ButtonStyle { icon: self.ui.pictures.icon("tool_sspheres"), ..Default::default() };
        let r = theme::button(ui, ui.available_width(), "New SSphere Rig", style);
        if tip(
            r,
            self.ui.tips,
            "Starts a skeleton of spheres with one in the middle of the view. Pull more out of it, pose them, then turn them into an object to sculpt.",
        )
        .clicked()
        {
            let now = ui.input(|i| i.time);
            self.spheres_new_rig(now);
        }
    }

    /// The SSpheres button among the tools (after Transform, as on the desktop).
    pub(super) fn spheres_tool_button(&mut self, ui: &mut Ui) {
        let style = ButtonStyle {
            icon: self.ui.pictures.icon("tool_sspheres"),
            active: (self.mode == Mode::Spheres).then_some(ACCENT()),
            ..Default::default()
        };
        let help = "Block out a form with connected spheres, then turn it into a new object to sculpt.";
        if tip(theme::button(ui, ui.available_width(), "SSpheres", style), self.ui.tips, help).clicked() {
            self.toggle_spheres();
        }
    }

    /// The tool's panel: the edit modes, the rig's status and undo lines,
    /// symmetry, attaching the rig to an object, and the skin with Make Skin
    /// (the desktop's `ui/spheres.rs`).
    pub(super) fn spheres_panel(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let mut new_rig = false;
        let mut make_skin = false;
        let icons: Vec<_> = EditMode::ALL.iter().map(|m| self.ui.pictures.icon(m.icon())).collect();
        let rig_icon = self.ui.pictures.icon("tool_sspheres");
        let objects = &self.objects;
        let selected = objects.iter().find(|o| o.id == self.active).expect("the active object is in the list");
        let s = &mut self.spheres.tool;
        let count = s.rig.alive_count();
        let shown = s.skin().map(|(skin, _)| skin.stats);

        theme::category(ui, "Spheres", Color32::from_rgb(255, 153, 46), s.mode().name(), true, |ui| {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 4, 4.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for (mode, icon) in EditMode::ALL.into_iter().zip(&icons) {
                    let active = (s.mode() == mode).then_some(ACCENT());
                    let badge = mode.hotkey().to_string();
                    let r = theme::tile(ui, vec2(w, 46.0), *icon, mode.name(), &badge, active);
                    let text = format!("{} ({})\n{}", mode.name(), mode.hotkey(), mode.description());
                    if tip(r, tips, &text).clicked() {
                        s.set_mode(mode);
                    }
                }
            });
            theme::hint(ui, s.mode().description());

            // One-off results first, then what the rig is doing.
            if let Some(note) = s.note() {
                line(ui, note, theme::OK);
            } else if count == 0 {
                line(ui, "Click in the viewport to place the first sphere. Drag to size it.", theme::HINT());
            } else if let Some(error) = s.skin_error() {
                line(ui, error, theme::ERROR);
            } else if let Some(stats) = shown {
                let draft = if s.skin_is_draft() { " (live draft)" } else { "" };
                let text = format!("{count} spheres   skin {} tris{draft}", fmt_count(stats.triangles));
                line(ui, &text, theme::OK);
            } else {
                line(ui, &format!("{count} spheres."), theme::HINT());
            }
            match s.undo_summary() {
                Some((label, steps)) => line(ui, &format!("Undo (Ctrl+Z): {label}. {steps} step(s) held."), theme::HINT()),
                None => line(ui, "Sphere history empty.", theme::HINT()),
            }
            if count == 0 {
                let style = ButtonStyle { icon: rig_icon, ..Default::default() };
                let r = theme::button(ui, ui.available_width(), "New SSphere Rig", style);
                if tip(
                    r,
                    tips,
                    "Starts a skeleton of spheres with one in the middle of the view. Pull more out of it, pose them, then turn them into an object to sculpt.",
                )
                .clicked()
                {
                    new_rig = true;
                }
            }

            if let Some(radius) = s.selected_radius() {
                if let Some(radius) = radius_slider(ui, "Selected Sphere Size", radius) {
                    s.set_selected_radius(radius);
                }
            }
            if let Some(radius) = radius_slider(ui, "Next Sphere Size", s.next_radius()) {
                s.set_next_radius(radius);
            }
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                ui.add_enabled_ui(s.selected.is_some(), |ui| {
                    let style = ButtonStyle { badge: "Del", ..Default::default() };
                    let r = theme::button(ui, w, "Delete Sphere", style);
                    if tip(r, tips, "Deletes the selected sphere and everything below it. Right-click does the same.").clicked() {
                        s.delete_selected();
                    }
                });
                ui.add_enabled_ui(count > 0, |ui| {
                    let r = theme::button(ui, w, "Clear All", ButtonStyle::default());
                    if tip(r, tips, "Removes every sphere. Undo (Ctrl+Z) brings them back.").clicked() {
                        s.clear();
                        s.say(format!("Cleared {count} spheres. Undo (Ctrl+Z) brings them back."));
                    }
                });
            });
        });

        let summary = if s.symmetry() { "X" } else { "Off" };
        theme::category(ui, "Symmetry", Color32::from_rgb(242, 115, 158), summary, true, |ui| {
            let mut on = s.symmetry();
            let r = theme::checkbox(ui, &mut on, "Mirror Across X", theme::AXIS[0]);
            if tip(
                r,
                tips,
                "Mirrors the whole rig live across the red plane (X). Turning it off keeps both halves as real spheres you can edit separately.",
            )
            .changed()
            {
                let report = s.set_symmetry(on);
                s.say(report);
            }
            theme::hint(ui, "A sphere on the red plane is shared by both sides; one off it gets a twin.");
        });

        // The object the rig follows, if it is still in the scene.
        let attached = s.attached_to().and_then(|id| objects.iter().find(|o| o.id == id));
        let summary = attached.map_or("Free", |o| o.name.as_str());
        theme::category(ui, "Attach", Color32::from_rgb(102, 204, 230), summary, true, |ui| {
            match attached {
                Some(o) => {
                    let text = format!("Attached to \"{}\". The spheres follow it and mirror across its plane.", o.name);
                    line(ui, &text, theme::OK);
                }
                None => line(ui, "Not attached. A first sphere placed on a model attaches to that model.", theme::HINT()),
            }
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                let r = theme::button(ui, w, "Attach to Selected", ButtonStyle::default());
                if tip(
                    r,
                    tips,
                    "Ties the spheres to the selected object: they follow it when it is moved, turned or resized, and mirror across its plane. No sphere moves.",
                )
                .clicked()
                {
                    let already = s.attached_to() == Some(selected.id);
                    let plane_moved = s.attach(Body::of(selected));
                    let name = &selected.name;
                    s.say(if already && !plane_moved {
                        format!("Already attached to \"{name}\".")
                    } else if count == 0 {
                        format!("Attached to \"{name}\". The spheres will follow it and mirror across its plane.")
                    } else if plane_moved && s.symmetry() {
                        format!(
                            "Attached to \"{name}\". No sphere moved, but the mirror plane is now that object's, so the mirrored half has. Undo (Ctrl+Z) puts it back."
                        )
                    } else {
                        format!("Attached to \"{name}\". No sphere moved; they follow it from now on.")
                    });
                }
                ui.add_enabled_ui(attached.is_some(), |ui| {
                    let r = theme::button(ui, w, "Detach", ButtonStyle::default());
                    if tip(r, tips, "Lets go of the object. The spheres stay where they are.").clicked() && s.detach() {
                        s.say("Detached. The spheres stay where they are.");
                    }
                });
            });
            ui.add_enabled_ui(count > 0, |ui| {
                let r = theme::button(ui, ui.available_width(), "Re-centre Mirror Plane", ButtonStyle::default());
                if tip(r, tips, "Moves the red mirror plane onto the attached object (or the selected one) without moving any sphere.")
                    .clicked()
                {
                    let onto = attached.unwrap_or(selected);
                    let moved = s.recentre(Body::of(onto));
                    let name = &onto.name;
                    s.say(if !moved {
                        format!("The mirror plane is already on \"{name}\".")
                    } else if s.symmetry() {
                        format!("Mirror plane moved onto \"{name}\". No sphere moved; the mirrored half now mirrors across it.")
                    } else {
                        format!("Mirror plane moved onto \"{name}\". No sphere moved.")
                    });
                }
            });
            theme::about(
                ui,
                "An attached rig goes with its object when that is moved, turned or resized evenly. Stretching the object along one axis leaves the spheres as they are.",
            );
        });

        let summary = shown.map_or(String::new(), |stats| fmt_count(stats.triangles));
        theme::category(ui, "Skin", Color32::from_rgb(115, 217, 128), &summary, true, |ui| {
            tip(
                theme::checkbox(ui, &mut s.show_skin, "Show Skin", ACCENT()),
                tips,
                "Shows the surface the spheres make as a see-through shell that updates while you edit.",
            );
            tip(
                theme::checkbox(ui, &mut s.preview, "Solid Preview", ACCENT()),
                tips,
                "Shows the solid skin with the spheres hidden, to judge the final shape (A). Limbs can still be dragged.",
            );
            let k = &mut s.settings;
            tip(
                theme::slider(ui, "Skin Detail", &mut k.density, skin::MIN_DENSITY..=skin::MAX_DENSITY, |v| format!("{v:.2}x")),
                tips,
                "How many surface points the skin gets. 1 is always fine enough for the thinnest limb; higher is smoother but heavier to sculpt.",
            );
            tip(
                theme::slider(ui, "Joint Blend", &mut k.blend, 0.0..=1.0, |v| format!("{v:.2}")),
                tips,
                "How softly branches melt into the limb they grow from, and how much the inside of a bend is filled.",
            );
            tip(
                theme::slider(ui, "Chain Curve", &mut k.curve, 0.0..=1.0, |v| format!("{v:.2}")),
                tips,
                "How much chains flow in a curve between their spheres. 0 is straight links; sharp joints always stay straight with a rounded joint.",
            );
            let mut smoothing = k.smoothing as f32;
            if tip(
                theme::slider(ui, "Skin Smoothing", &mut smoothing, 0.0..=8.0, |v| format!("{:.0}", v.round())),
                tips,
                "Evens out the surface before it is snapped onto the exact smooth shape.",
            )
            .changed()
            {
                k.smoothing = smoothing.round() as u32;
            }

            theme::heading(ui, "Make It a Model");
            if let Some(tris) = s.final_triangles() {
                line(ui, &format!("About {} triangles at this detail.", fmt_count(tris)), theme::HINT());
            }
            tip(
                theme::checkbox(ui, &mut s.keep_rig, "Keep Spheres Afterwards", ACCENT()),
                tips,
                "Leaves the spheres in place, so you can change them later and make a fresh skin.",
            );
            tip(
                theme::checkbox(ui, &mut s.snap_to_surface, "First Sphere Rests on the Model", ACCENT()),
                tips,
                "The first sphere of a rig sits on the model under the cursor, touching it. Later spheres are free.",
            );
            ui.add_enabled_ui(count > 0, |ui| {
                let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
                if tip(
                    theme::button(ui, ui.available_width(), "Make Skin", style),
                    tips,
                    "Wraps a skin over the spheres and adds it to the scene as a new object you can sculpt.",
                )
                .clicked()
                {
                    make_skin = true;
                }
            });
            theme::about(
                ui,
                "The skin does not follow the spheres afterwards: change them and you need a new skin, so settle the overall shape before sculpting detail.",
            );
        });

        theme::category(ui, "SSphere Shortcuts", Color32::from_rgb(230, 204, 102), "", false, |ui| {
            let rows: &[(&str, &str)] = &[
                ("1 - 4", "Draw, Move, Scale, Rotate"),
                ("Drag off a sphere", "Pull out a new one"),
                ("Click empty space", "Add to the selected"),
                ("Drag a link", "Add a joint / pose"),
                ("Scroll on a sphere", "Resize it"),
                ("S + drag", "Size of the next sphere"),
                ("Right-click sphere", "Delete it and its branch"),
                ("Ctrl + click sphere", "Delete it and its branch"),
                ("Delete", "Delete the selected"),
                ("Shift + drag", "One sphere / whole branch"),
                ("Esc", "Cancel a drag, deselect"),
                ("A", "Solid preview"),
                ("X", "Symmetry"),
                ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
                ("Drag on background", "Orbit"),
                ("F F", "Frame the spheres"),
            ];
            for (key, what) in rows {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::hover());
                ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, key, theme::font(11.0), theme::KEY);
                ui.painter().text(rect.left_center() + vec2(118.0, 0.0), Align2::LEFT_CENTER, what, theme::font(11.0), theme::TEXT());
            }
        });

        if new_rig {
            let now = ui.input(|i| i.time);
            self.spheres_new_rig(now);
        }
        if make_skin {
            self.spheres_make_skin();
        }
    }
}
