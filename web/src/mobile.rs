//! Phones and fingers: the touch gestures, and the interface laid out for a
//! narrow screen.
//!
//! The gestures work on any screen a finger can reach:
//!
//! * one finger on the model sculpts; off it, turns the view;
//! * two fingers pinch to zoom and drag to pan. A stroke the first finger
//!   had begun is taken back when the second lands, so a pinch never leaves
//!   a dent;
//! * a quick tap with two fingers undoes, with three redoes.
//!
//! eframe hands a finger over twice: as pointer events and as touch events.
//! The page (index.html) stops the pointer events of fingers, so only the
//! touch events come through. eframe makes a press, moves and a release of
//! the first finger out of those, and makes the press and the release again
//! each time another finger lands or lifts. [`Touch::filter`] lets only the
//! first press and the last release through, so a gesture is one press to
//! egui and to the viewport, however many fingers it takes.
//!
//! On a narrow screen (a phone; [`compact`]) the two desktop panels give way
//! to a bar along the top (the menu, undo, redo, frame), two thumb sliders
//! down the left (size and strength), and two rows along the bottom: Sub,
//! Smooth, Mask and Mirror, and the brushes. The menu opens a sheet with
//! everything the panels have, as the desktop lays it out.

use std::collections::HashMap;

use eframe::egui::{self, Align2, Color32, Id, Order, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::ui::{self, theme, theme::ACCENT, theme::ButtonStyle};
use super::{Drag, GolemApp, Mode};
use crate::mesh::primitives::Primitive;
use crate::sculpt::library::Origin;
use crate::sculpt::Pace;

/// A tap with fingers is quicker than this, in seconds...
const TAP_SECONDS: f64 = 0.35;
/// ...and no finger travels further than this, in points.
const TAP_TRAVEL: f32 = 16.0;

/// Below this width or height, in points, the screen is laid out for a phone.
const NARROW: f32 = 720.0;
const LOW: f32 = 480.0;

/// The gap round the phone layout's pieces, and between them.
const EDGE: f32 = 8.0;
const GAP: f32 = 6.0;
/// The heights of the top bar, the row of switches and the brush strip.
const BAR: f32 = 44.0;
const SWITCHES: f32 = 46.0;
const STRIP: f32 = 58.0;
/// The width of a thumb slider, and the most each may be tall.
const THUMB: f32 = 46.0;
const THUMB_MOST: f32 = 230.0;

/// Whether the screen is laid out for a phone.
pub fn compact(ctx: &egui::Context) -> bool {
    let screen = ctx.content_rect();
    screen.width() < NARROW || screen.height() < LOW
}

/// The many-finger gesture under way.
struct Many {
    /// When the second finger landed.
    began: f64,
    /// The most fingers down at once.
    most: usize,
    /// Whether any finger has travelled: then it is no tap.
    moved: bool,
}

/// What the fingers and the phone layout keep between frames.
#[derive(Default)]
pub struct Touch {
    /// Each finger down: where it landed and where it is.
    fingers: HashMap<u64, (Pos2, Pos2)>,
    many: Option<Many>,
    /// A press of the first finger went through and its release has not.
    pressed: bool,
    /// Where the first finger was last seen, for a release eframe never sends.
    last: Pos2,
    /// A second finger landed: what the first one had begun is taken back.
    take_back: bool,
    /// A tap with this many fingers, to be undone (2) or redone (3).
    tapped: Option<usize>,
    /// The last finger lifted: the pointer is gone from the next frame on.
    gone_next: bool,
    /// The first finger's press, held back a frame (see `filter`).
    press_next: Option<egui::Event>,
    /// The Carve switch: brushes work the other way, as with Ctrl.
    pub invert: bool,
    /// The Smooth switch: brushes smooth, as with Shift.
    pub smooth: bool,
    /// The menu sheet is open, at this page.
    sheet: Option<Page>,
    /// The page the sheet was last left at.
    last_page: Page,
    /// The first visit's tour, at this step (see `phone_tour`).
    pub tour: Option<usize>,
    /// Where the tour's steps point, as drawn last frame: the model, the
    /// bottom rows, the thumb sliders, the view ball, the top bar.
    places: Vec<Rect>,
}

/// The menu sheet's pages: the everyday things, then the two desktop panels whole.
#[derive(Clone, Copy, Default, PartialEq)]
enum Page {
    #[default]
    Basics,
    Sculpt,
    Scene,
}

/// The tour's steps: what each points at (an index into `Touch::places`) and what it says.
const TOUR: [(usize, &str, &str); 5] = [
    (0, "Sculpt with one finger", "Draw on the model to build up clay where your finger goes."),
    (1, "Brushes", "Pick a brush down here. Carve digs in instead of building up; Smooth softens. Tap either again to turn it off."),
    (2, "Size and strength", "Slide these up for a bigger, stronger brush, and down for a smaller, gentler one."),
    (3, "Turn and zoom", "Drag this ball to turn the model around, or drag beside the model. Pinch with two fingers to zoom."),
    (4, "Undo and the rest", "Undo takes back a mistake. Menu has saving, finer detail, new shapes and every setting."),
];

impl Touch {
    /// Takes out of this frame's input what egui and the viewport should not
    /// see: the presses and releases eframe makes when a second, third...
    /// finger lands or lifts. Called before egui reads the input. True when
    /// another frame is wanted (to say the pointer has gone).
    pub fn filter(&mut self, raw: &mut egui::RawInput) -> bool {
        let now = raw.time.unwrap_or(0.0);
        let events = std::mem::take(&mut raw.events);
        let mut kept = Vec::with_capacity(events.len() + 1);
        // eframe says the pointer has gone in the same frame as the last
        // finger's release, and egui then finds nothing under the release: a
        // tap clicked nothing. The pointer goes a frame later instead.
        if std::mem::take(&mut self.gone_next) && self.fingers.is_empty() {
            kept.push(egui::Event::PointerGone);
        }
        // egui finds what a press lands on from where the pointer was the
        // frame before, and a finger was nowhere: the press comes a frame
        // after the finger is said to be there.
        kept.extend(self.press_next.take());
        // eframe puts the press or release it makes just before the touch
        // events of the finger that made it: what follows an event says
        // whether a finger made it.
        let touches_after = |k: usize| -> Vec<(egui::TouchId, egui::TouchPhase)> {
            events[k + 1..]
                .iter()
                .skip_while(|e| matches!(e, egui::Event::PointerGone))
                .map_while(|e| match e {
                    egui::Event::Touch { id, phase, .. } => Some((*id, *phase)),
                    _ => None,
                })
                .collect()
        };
        for (k, event) in events.iter().enumerate() {
            match event {
                egui::Event::PointerButton { pressed: true, pos, .. } => {
                    let after = touches_after(k);
                    let by_finger = after.iter().any(|(_, phase)| *phase == egui::TouchPhase::Start);
                    if by_finger {
                        // Only the gesture's first finger presses.
                        if !self.fingers.is_empty() || self.pressed {
                            continue;
                        }
                        self.pressed = true;
                        self.last = *pos;
                        kept.push(egui::Event::PointerMoved(*pos));
                        self.press_next = Some(event.clone());
                        continue;
                    }
                }
                egui::Event::PointerButton { pressed: false, .. } | egui::Event::PointerGone => {
                    let after = touches_after(k);
                    let lifting = after
                        .iter()
                        .filter(|(id, phase)| {
                            matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel) && self.fingers.contains_key(&id.0)
                        })
                        .count();
                    if lifting > 0 {
                        // Only the gesture's last finger lets go.
                        if self.fingers.len() > lifting {
                            continue;
                        }
                        if matches!(event, egui::Event::PointerButton { .. }) {
                            self.pressed = false;
                        } else {
                            self.gone_next = true;
                            continue;
                        }
                    }
                }
                egui::Event::PointerMoved(pos) if !self.fingers.is_empty() => self.last = *pos,
                egui::Event::Touch { id, phase, pos, .. } => {
                    let cancelled = self.finger(now, *id, *phase, *pos);
                    kept.push(event.clone());
                    // A cancelled touch (the browser took it) sends no release: make one.
                    if cancelled && self.fingers.is_empty() && self.pressed {
                        self.pressed = false;
                        let modifiers = egui::Modifiers::default();
                        let pos = self.last;
                        kept.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers });
                        self.gone_next = true;
                    }
                    continue;
                }
                _ => {}
            }
            kept.push(event.clone());
        }
        // A release in the same frame as its held-back press goes after it.
        if let Some(press) = self.press_next.take() {
            if let Some(at) = kept.iter().position(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. })) {
                kept.insert(at, press);
            } else {
                self.press_next = Some(press);
            }
        }
        raw.events = kept;
        self.gone_next || self.press_next.is_some()
    }

    /// One finger's touch event. True when it was cancelled and is gone.
    fn finger(&mut self, now: f64, id: egui::TouchId, phase: egui::TouchPhase, pos: Pos2) -> bool {
        match phase {
            egui::TouchPhase::Start => {
                self.fingers.insert(id.0, (pos, pos));
                let count = self.fingers.len();
                if count >= 2 {
                    if self.many.is_none() {
                        self.take_back = true;
                    }
                    // The first finger's press, if still held back, never happens.
                    if self.press_next.take().is_some() {
                        self.pressed = false;
                    }
                    let many = self.many.get_or_insert(Many { began: now, most: 0, moved: false });
                    many.most = many.most.max(count);
                }
                false
            }
            egui::TouchPhase::Move => {
                if let Some((from, at)) = self.fingers.get_mut(&id.0) {
                    *at = pos;
                    if from.distance(pos) > TAP_TRAVEL {
                        if let Some(many) = &mut self.many {
                            many.moved = true;
                        }
                    }
                }
                false
            }
            egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                self.fingers.remove(&id.0);
                if self.fingers.is_empty() {
                    if let Some(many) = self.many.take() {
                        let quick = now - many.began < TAP_SECONDS;
                        let still = !many.moved && phase == egui::TouchPhase::End;
                        if quick && still {
                            self.tapped = Some(many.most);
                        }
                    }
                }
                phase == egui::TouchPhase::Cancel
            }
        }
    }
}

impl GolemApp {
    /// What the fingers did since the last frame, before the viewport reads
    /// the pointer: a stroke a pinch began is taken back, and a tap with two
    /// or three fingers undoes or redoes.
    pub(super) fn touch_frame(&mut self) {
        if std::mem::take(&mut self.touch.take_back) {
            match self.drag {
                Drag::Sculpt => {
                    // As if it had never been: no undo step, nothing to save.
                    let view = self.local_view();
                    if let Some(delta) = self.sculptor.end(&mut self.mesh, &view) {
                        self.mesh.apply_delta(&delta, true);
                    }
                    self.sculptor.pace = Pace::Unpaced;
                    self.sculptor.pen_meter.lifted();
                    self.drag = Drag::None;
                }
                // The view is the fingers' now (the pinch, in sculpt_input).
                Drag::Orbit | Drag::Pan | Drag::Zoom => self.drag = Drag::None,
                // A tool's or the gizmo's drag is left to finish as it is.
                Drag::Tool | Drag::Gizmo | Drag::None => {}
            }
        }
        match self.touch.tapped.take() {
            Some(2) => self.undo_redo(true),
            Some(n) if n >= 3 => self.undo_redo(false),
            _ => {}
        }
    }

    /// Whether brushes smooth: Shift held, or the Smooth switch on.
    pub(super) fn smooth_held(&self, shift: bool) -> bool {
        shift || self.touch.smooth
    }

    // ---- the phone layout ----------------------------------------------------------

    /// The whole interface for one frame on a phone.
    pub(super) fn phone_interface(&mut self, root: &mut Ui) {
        let ctx = root.ctx().clone();
        let screen = ctx.content_rect().shrink(EDGE);
        sculpt_toolkit::floating::set_room(&ctx, screen);

        self.phone_top_bar(&ctx, screen);
        let top = screen.top() + BAR + GAP;
        // The view ball, without its rows (Fit View is on the bar): a drag on it turns the view.
        let ball = Rect::from_min_size(pos2(screen.right() - 108.0, top), vec2(108.0, 108.0));
        // The ball with the words under it, for the tour to point at.
        let ball_and_hint = Rect::from_min_size(ball.min, vec2(108.0, 124.0));
        self.view_gizmo(&ctx, ball.min, false);
        // Said under it, since nothing else shows that it can be dragged.
        ctx.layer_painter(egui::LayerId::new(Order::Background, Id::new("phone_ball_hint"))).text(
            pos2(ball.center().x, ball.bottom() + 2.0),
            Align2::CENTER_TOP,
            "drag to turn",
            theme::font(10.5),
            theme::HINT(),
        );

        let sculpting = self.mode == Mode::Sculpt;
        let rows = if sculpting { SWITCHES + GAP + STRIP } else { SWITCHES };
        let bottom = screen.bottom() - rows;
        self.phone_status(&ctx, pos2(screen.left() + THUMB + GAP * 2.0, top), screen.width() - THUMB - 108.0 - GAP * 4.0);
        if sculpting {
            self.thumb_sliders(&ctx, Rect::from_min_max(pos2(screen.left(), top + 4.0), pos2(screen.left() + THUMB, bottom - GAP * 2.0)));
        }
        self.phone_bottom(&ctx, Rect::from_min_max(pos2(screen.left(), bottom), screen.max));

        let model = Rect::from_min_max(pos2(screen.left() + THUMB + GAP, ball.bottom() + 20.0), pos2(screen.right(), bottom - GAP));
        let thumbs = Rect::from_min_max(pos2(screen.left(), top), pos2(screen.left() + THUMB, bottom - GAP * 2.0));
        let bar = Rect::from_min_size(screen.min, vec2(screen.width(), BAR));
        self.touch.places = vec![model, Rect::from_min_max(pos2(screen.left(), bottom), screen.max), thumbs, ball_and_hint, bar];

        if let Some(page) = self.touch.sheet {
            self.phone_sheet(root, screen, page);
        }
        if let Some(step) = self.touch.tour {
            self.phone_tour(&ctx, step);
        }
    }

    fn phone_top_bar(&mut self, ctx: &egui::Context, screen: Rect) {
        egui::Area::new(Id::new("phone_bar")).order(Order::Middle).fade_in(false).fixed_pos(screen.min).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(GAP, GAP);
            ui.horizontal(|ui| {
                let open = self.touch.sheet.is_some();
                let style = ButtonStyle { height: BAR, active: open.then_some(ACCENT()), ..Default::default() };
                if theme::button(ui, 76.0, if open { "Close" } else { "Menu" }, style).clicked() {
                    let page = if self.mode != Mode::Sculpt { Page::Sculpt } else { self.touch.last_page };
                    self.touch.sheet = if open { None } else { Some(page) };
                }
                let p = &self.ui.pictures;
                let (undo_icon, redo_icon) = (p.icon("undo"), p.icon("redo"));
                let (can_undo, can_redo) = (self.can_undo(), self.can_redo());
                ui.add_enabled_ui(can_undo, |ui| {
                    if theme::button(ui, 62.0, "Undo", ButtonStyle { icon: undo_icon, height: BAR, ..Default::default() }).clicked() {
                        self.undo_redo(true);
                    }
                });
                ui.add_enabled_ui(can_redo, |ui| {
                    if theme::button(ui, 62.0, "Redo", ButtonStyle { icon: redo_icon, height: BAR, ..Default::default() }).clicked() {
                        self.undo_redo(false);
                    }
                });
                let left = (screen.width() - ui.min_rect().width() - GAP).max(0.0);
                if left >= 66.0 {
                    ui.add_space(left - 66.0);
                    if theme::button(ui, 66.0, "Fit View", ButtonStyle { height: BAR, ..Default::default() }).clicked() {
                        self.frame_model(true);
                    }
                }
            });
        });
    }

    /// The brush in hand, the triangles, and the last word from the app, under the bar.
    fn phone_status(&mut self, ctx: &egui::Context, at: Pos2, width: f32) {
        egui::Area::new(Id::new("phone_status")).order(Order::Background).fade_in(false).interactable(false).fixed_pos(at).show(ctx, |ui| {
            ui.set_max_width(width.max(80.0));
            let s = &self.sculptor.settings;
            let brush = match self.mode {
                Mode::Tubes => self.tubes.name().to_owned(),
                Mode::Transform => "Transform".to_owned(),
                Mode::Spheres => "SSpheres".to_owned(),
                Mode::Sculpt if s.mask_mode => "Mask".to_owned(),
                Mode::Sculpt => self.sculptor.library.entries()[self.sculptor.library.selected(s)].name.clone(),
            };
            let mut words = vec![brush];
            if self.mode == Mode::Sculpt && self.touch.smooth {
                words.push("Smoothing".into());
            } else if self.mode == Mode::Sculpt && self.touch.invert {
                words.push("Carving".into());
            }
            let mirror: Vec<&str> = ["X", "Y", "Z"].into_iter().zip(s.symmetry).filter(|(_, on)| *on).map(|(a, _)| a).collect();
            if !mirror.is_empty() {
                words.push(format!("Mirror {}", mirror.join(" ")));
            }
            ui.label(egui::RichText::new(words.join("  ·  ")).font(theme::bold(13.0)).color(theme::TEXT()));
            ui.label(
                egui::RichText::new(format!("{} triangles", super::group_digits(self.mesh.live_triangle_count())))
                    .font(theme::font(11.0))
                    .color(theme::HINT()),
            );
            if !self.status.is_empty() {
                ui.label(egui::RichText::new(&self.status).font(theme::font(11.0)).color(theme::HINT()));
            }
        });
    }

    /// Size and Strength, tall and thin down the left, for a thumb: drag up
    /// for more. A drag moves the value from where it is, so a touch does
    /// not make it jump.
    fn thumb_sliders(&mut self, ctx: &egui::Context, room: Rect) {
        let each = ((room.height() - GAP * 3.0) / 2.0).clamp(80.0, THUMB_MOST);
        egui::Area::new(Id::new("phone_thumbs")).order(Order::Middle).fade_in(false).fixed_pos(room.min).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, GAP * 3.0);
            let height = ctx.content_rect().height() * ctx.pixels_per_point();
            let s = &mut self.sculptor.settings;
            // Size: on screen, from the smallest to the largest the slider offers;
            // on the model, doubling for equal travel.
            let (min, max) = s.size_range(height);
            let size = s.size().clamp(min, max);
            let (part, shown) = if s.screen_space_size {
                ((size - min) / (max - min).max(1e-6), format!("{size:.0}"))
            } else {
                ((size.log2() - min.log2()) / (max.log2() - min.log2()).max(1e-6), format!("{size:.2}"))
            };
            if let Some(part) = thumb(ui, "phone_size", each, "Size", &shown, part) {
                let value = if s.screen_space_size { min + part * (max - min) } else { (min.log2() + part * (max.log2() - min.log2())).exp2() };
                s.set_size(value, height);
            }
            let bs = s.current_mut();
            let shown = format!("{:.0}%", bs.strength * 100.0);
            if let Some(part) = thumb(ui, "phone_strength", each, "Strength", &shown, (bs.strength - 0.01) / 0.99) {
                bs.strength = 0.01 + part * 0.99;
            }
        });
    }

    /// The two rows along the bottom: the switches, and the brushes. With a
    /// tool up, the way back to the brushes and the tool's own cards.
    fn phone_bottom(&mut self, ctx: &egui::Context, room: Rect) {
        egui::Area::new(Id::new("phone_bottom")).order(Order::Middle).fade_in(false).fixed_pos(room.min).show(ctx, |ui| {
            ui.set_width(room.width());
            ui.spacing_mut().item_spacing = vec2(GAP, GAP);
            if self.mode != Mode::Sculpt {
                ui.horizontal(|ui| {
                    let w = (room.width() - GAP) / 2.0;
                    let icon = self.ui.pictures.icon("tool_sculpt");
                    if theme::button(ui, w, "Back to Sculpt", ButtonStyle { icon, height: SWITCHES, ..Default::default() }).clicked() {
                        self.mode = Mode::Sculpt;
                        self.touch.sheet = None;
                    }
                    if theme::button(ui, w, "Tool Settings", ButtonStyle { height: SWITCHES, ..Default::default() }).clicked() {
                        self.touch.sheet = Some(Page::Sculpt);
                    }
                });
                return;
            }
            let w = (room.width() - GAP * 3.0) / 4.0;
            ui.horizontal(|ui| {
                let p = &self.ui.pictures;
                let s = &mut self.sculptor.settings;
                let t = &mut self.touch;
                let style = |icon: &str, on: bool, color: Color32| ButtonStyle {
                    icon: p.icon(icon),
                    active: on.then_some(color),
                    height: SWITCHES,
                    ..Default::default()
                };
                // Carve and Smooth stay on until tapped again; one puts the other off.
                if theme::button(ui, w, "Carve", style("polarity_sub", t.invert, theme::SUB)).clicked() {
                    t.invert = !t.invert;
                    t.smooth &= !t.invert;
                }
                if theme::button(ui, w, "Smooth", style("brush_smooth", t.smooth, ACCENT())).clicked() {
                    t.smooth = !t.smooth;
                    t.invert &= !t.smooth;
                }
                if theme::button(ui, w, "Mask", style("brush_mask", s.mask_mode, theme::MASK)).clicked() {
                    s.mask_mode = !s.mask_mode;
                }
                if theme::button(ui, w, "Mirror", style("mirror", s.symmetry[0], ACCENT())).clicked() {
                    s.symmetry[0] = !s.symmetry[0];
                }
            });
            self.brush_strip(ui, room.width());
        });
    }

    /// Every brush, a tile each, in a row that a finger slides along.
    fn brush_strip(&mut self, ui: &mut Ui, width: f32) {
        let pictures = &self.ui.pictures;
        let (library, s) = (&mut self.sculptor.library, &mut self.sculptor.settings);
        let entries = library.entries();
        let in_hand = library.selected(s);
        let list: Vec<usize> = ui::GROUPS
            .iter()
            .flat_map(|(_, kernels)| kernels.iter())
            .flat_map(|&k| (0..entries.len()).filter(move |&i| entries[i].kernel == k))
            .filter(|&i| entries[i].origin != Origin::User)
            .collect();
        let mut pick = None;
        egui::Frame::new().fill(theme::PANEL()).corner_radius(8.0).inner_margin(4.0).show(ui, |ui| {
            ui.set_width(width - 8.0);
            egui::ScrollArea::horizontal().id_salt("phone_brushes").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
                ui,
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                        for &i in &list {
                            let e = &entries[i];
                            let lit = !s.mask_mode && in_hand == i;
                            let r = theme::tile(ui, vec2(66.0, STRIP - 8.0), pictures.icon(e.kernel.icon()), &short(&e.name), "", lit.then_some(ACCENT()));
                            if r.clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                },
            );
        });
        if let Some(i) = pick {
            library.select(i, s);
            s.mask_mode = false;
        }
    }

    /// The menu: everything the two desktop panels hold, a side at a time,
    /// over the lower part of the screen (the right half on its side), so
    /// the model stays in sight above.
    fn phone_sheet(&mut self, root: &mut Ui, screen: Rect, page: Page) {
        let ctx = root.ctx().clone();
        let wide = screen.width() > screen.height();
        let rect = if wide {
            Rect::from_min_max(pos2(screen.center().x.max(screen.right() - 420.0), screen.top() + BAR + GAP), screen.max)
        } else {
            let top = (screen.top() + screen.height() * 0.34).max(screen.top() + BAR + GAP);
            Rect::from_min_max(pos2(screen.left(), top), screen.max)
        };
        let mut page = page;
        let mut close = false;
        egui::Area::new(Id::new("phone_sheet")).order(Order::Foreground).fade_in(false).fixed_pos(rect.min).show(&ctx, |ui| {
            egui::Frame::new()
                .fill(theme::PANEL())
                .corner_radius(12.0)
                .stroke(theme::hairline())
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    let inner = rect.width() - 20.0;
                    ui.set_width(inner);
                    ui.set_max_height(rect.height() - 20.0);
                    ui.spacing_mut().item_spacing = vec2(GAP, GAP);
                    ui.horizontal(|ui| {
                        let w = (inner - 70.0 - GAP * 3.0) / 3.0;
                        let tab = |on: bool| ButtonStyle { height: 40.0, active: on.then_some(ACCENT()), ..Default::default() };
                        for (this, name) in [(Page::Basics, "Basics"), (Page::Sculpt, "Brush & Tools"), (Page::Scene, "Scene")] {
                            if theme::button(ui, w, name, tab(page == this)).clicked() {
                                page = this;
                            }
                        }
                        if theme::button(ui, 70.0, "Done", ButtonStyle { height: 40.0, ..Default::default() }).clicked() {
                            close = true;
                        }
                    });
                    let side = page == Page::Sculpt;
                    let top = ui.cursor().top();
                    let tips = self.ui.tips;
                    // The panel's own tabs, under the sheet's (Basics has none).
                    if side && self.mode == Mode::Sculpt {
                        let mut showing = self.ui.sculpt_tab;
                        ui::tabs::sculpt_strip(ui, &mut showing, tips);
                        self.ui.sculpt_tab = showing;
                    } else if page == Page::Scene {
                        let mut showing = self.ui.scene_tab;
                        ui::tabs::scene_strip(ui, &mut showing, tips);
                        self.ui.scene_tab = showing;
                    }
                    let head = ui.cursor().top() - top;
                    let tab = match page {
                        Page::Basics => "basics",
                        Page::Sculpt => self.ui.sculpt_tab.name(),
                        Page::Scene => self.ui.scene_tab.name(),
                    };
                    egui::ScrollArea::vertical()
                        .id_salt(("phone_sheet", tab))
                        .max_height((rect.height() - 20.0 - 42.0 - head).max(60.0))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(inner);
                            if page == Page::Basics {
                                close |= self.phone_basics(ui, inner);
                                return;
                            }
                            if side {
                                if self.mode == Mode::Sculpt && self.ui.sculpt_tab == ui::tabs::Sculpt::Brush {
                                    let pictures = &self.ui.pictures;
                                    let icon = |name: &str| pictures.icon(name);
                                    ui::brush_plate(ui, &mut self.sculptor.settings, &icon, tips);
                                }
                                self.sculpt_body(ui);
                            } else {
                                self.scene_body(ui);
                            }
                            ui.add_space(12.0);
                            self.phone_switches(ui);
                        });
                });
        });
        self.touch.last_page = page;
        self.touch.sheet = if close { None } else { Some(page) };
    }

    /// The display switches of the desktop's bottom bar, at the foot of the sheet.
    fn phone_switches(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            theme::checkbox(ui, &mut self.flat_shading, "Flat Shading", ACCENT());
            theme::checkbox(ui, &mut self.show_backfaces, "Show Backfaces", ACCENT());
            theme::checkbox(ui, &mut self.mask_tint, "Show Mask", ACCENT());
            let mut wire = self.look.wire;
            theme::checkbox(ui, &mut wire, "Wireframe", ACCENT());
            if wire != self.look.wire {
                self.set_wire(wire);
            }
        });
        let help = ui.add(egui::Label::new(egui::RichText::new("Help").font(theme::font(13.0)).color(ACCENT())).sense(Sense::click()));
        if help.clicked() {
            self.touch.sheet = None;
            self.touch.tour = Some(0);
        }
        theme::hint(
            ui,
            "One finger on the model sculpts; off it, turns the view. Two fingers pinch to zoom and drag to pan. \
             Tap with two fingers to undo, three to redo.",
        );
    }

    /// The Basics page: what most people want, in plain words, a big button each.
    /// True when the sheet should close (a file is being picked, the tour starts).
    fn phone_basics(&mut self, ui: &mut Ui, width: f32) -> bool {
        let mut close = false;
        let big = |on: bool| ButtonStyle { height: 44.0, active: on.then_some(ACCENT()), ..Default::default() };
        let heading = |ui: &mut Ui, text: &str| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(text).font(theme::bold(14.0)).color(theme::TEXT()));
        };
        let note = |ui: &mut Ui, text: &str| {
            ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(12.0)).color(theme::HINT())).wrap());
        };

        heading(ui, "Detail");
        ui.horizontal(|ui| {
            let w = (width - GAP) / 2.0;
            if theme::button(ui, w, "Finer Detail", big(false)).clicked() {
                self.subdivide();
            }
            if theme::button(ui, w, "Even Out", big(false)).clicked() {
                self.remesh();
            }
        });
        note(ui, "Finer Detail splits every triangle so small marks come out sharp. Even Out rebuilds the surface evenly when it gets stretched.");

        heading(ui, "Start again with a shape");
        let shapes = [Primitive::Sphere, Primitive::Cube, Primitive::Cylinder, Primitive::Capsule, Primitive::Cone];
        for row in shapes.chunks(3) {
            ui.horizontal(|ui| {
                let w = (width - GAP * 2.0) / 3.0;
                for &shape in row {
                    if theme::button(ui, w, shape.name(), big(false)).clicked() {
                        self.shape = shape;
                        self.new_shape();
                        close = true;
                    }
                }
            });
        }
        note(ui, "This replaces the model. Undo brings your work back.");

        heading(ui, "Save and open");
        if theme::button(ui, width, "Save My Work", big(true)).clicked() {
            self.save_scene();
        }
        ui.horizontal(|ui| {
            let w = (width - GAP) / 2.0;
            if theme::button(ui, w, "Open a File", big(false)).clicked() {
                self.open_picker();
                close = true;
            }
            if theme::button(ui, w, "Save for 3D Printing", big(false)).clicked() {
                self.save_file(true);
            }
        });
        note(ui, "Your work is also kept in this browser between visits. Save My Work downloads it, to carry on elsewhere. Save for 3D Printing downloads an STL file.");

        heading(ui, "Help");
        if theme::button(ui, width, "Show Me Around", big(false)).clicked() {
            self.touch.tour = Some(0);
            close = true;
        }
        ui.add_space(4.0);
        note(ui, "Brush & Tools and Scene hold every setting, as on the computer version.");
        close
    }

    /// The first visit's tour: the screen dimmed but for one thing at a
    /// time, with a word on what it is for. Nothing under it takes a touch.
    fn phone_tour(&mut self, ctx: &egui::Context, step: usize) {
        let Some(&(place, title, text)) = TOUR.get(step) else {
            self.touch.tour = None;
            return;
        };
        let screen = ctx.content_rect();
        let target = self.touch.places.get(place).copied().unwrap_or(screen).expand(4.0);
        // The dimmed page, with a window onto the target; it takes every touch.
        egui::Area::new(Id::new("phone_tour_dim")).order(Order::Foreground).fade_in(false).fixed_pos(screen.min).show(ctx, |ui| {
            ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
            let p = ui.painter();
            let dim = Color32::from_black_alpha(170);
            p.rect_filled(Rect::from_min_max(screen.min, pos2(screen.right(), target.top())), 0.0, dim);
            p.rect_filled(Rect::from_min_max(pos2(screen.left(), target.bottom()), screen.max), 0.0, dim);
            p.rect_filled(Rect::from_min_max(pos2(screen.left(), target.top()), pos2(target.left(), target.bottom())), 0.0, dim);
            p.rect_filled(Rect::from_min_max(pos2(target.right(), target.top()), pos2(screen.right(), target.bottom())), 0.0, dim);
            p.rect_stroke(target, 10.0, Stroke::new(2.5, ACCENT()), egui::StrokeKind::Outside);
        });
        // The card: beside a narrow target, else below it if there is room,
        // else above it, else inside it at the foot.
        let mut width = (screen.width() - 32.0).min(420.0);
        let left = screen.center().x - width / 2.0;
        const ROOM: f32 = 170.0;
        let beside = screen.right() - target.right() - 24.0;
        let (at, pivot) = if target.width() < screen.width() / 4.0 && beside >= 220.0 {
            width = beside.min(420.0);
            (pos2(target.right() + 12.0, target.center().y), Align2::LEFT_CENTER)
        } else if screen.bottom() - target.bottom() > ROOM {
            (pos2(left, target.bottom() + 12.0), Align2::LEFT_TOP)
        } else if target.top() - screen.top() > ROOM {
            (pos2(left, target.top() - 12.0), Align2::LEFT_BOTTOM)
        } else {
            (pos2(left, target.bottom() - 16.0), Align2::LEFT_BOTTOM)
        };
        let mut next = None;
        egui::Area::new(Id::new("phone_tour_card")).order(Order::Tooltip).fade_in(false).pivot(pivot).fixed_pos(at).show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::PANEL())
                .corner_radius(12.0)
                .stroke(Stroke::new(1.0, ACCENT()))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(width - 28.0);
                    ui.spacing_mut().item_spacing = vec2(GAP, GAP);
                    let count = format!("{} of {}", step + 1, TOUR.len());
                    ui.label(egui::RichText::new(count).font(theme::font(11.0)).color(theme::HINT()));
                    ui.label(egui::RichText::new(title).font(theme::bold(17.0)).color(theme::TEXT()));
                    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(14.0)).color(theme::TEXT())).wrap());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let w = (width - 28.0 - GAP) / 2.0;
                        if theme::button(ui, w, "Skip", ButtonStyle { height: 42.0, ..Default::default() }).clicked() {
                            next = Some(None);
                        }
                        let last = step + 1 == TOUR.len();
                        let style = ButtonStyle { height: 42.0, active: Some(ACCENT()), ..Default::default() };
                        if theme::button(ui, w, if last { "Start Sculpting" } else { "Next" }, style).clicked() {
                            next = Some((!last).then_some(step + 1));
                        }
                    });
                });
        });
        if let Some(next) = next {
            self.touch.tour = next;
            if next.is_none() {
                // Seen: the first visit's welcome does not come back.
                self.welcome_seen();
            }
            ctx.request_repaint();
        }
    }
}

/// A brush's name short enough for its tile.
fn short(name: &str) -> String {
    if name.chars().count() <= 11 { name.to_owned() } else { name.chars().take(10).collect::<String>() + "…" }
}

/// One thumb slider, `part` of the way up (0 to 1), `height` tall: its name
/// at the top, its value at the foot. The new part while it is dragged.
fn thumb(ui: &mut Ui, id: &str, height: f32, name: &str, shown: &str, part: f32) -> Option<f32> {
    let (rect, response) = ui.allocate_exact_size(vec2(THUMB, height), Sense::drag());
    let id = Id::new(id);
    // Where the drag began, so the value follows the finger from where it was.
    let mut part = part.clamp(0.0, 1.0);
    let mut changed = None;
    if response.drag_started() {
        ui.ctx().data_mut(|d| d.insert_temp(id, part));
    }
    if response.dragged() {
        let from: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(part);
        let travelled = response.total_drag_delta().map_or(0.0, |d| -d.y);
        part = (from + travelled / (height - 8.0)).clamp(0.0, 1.0);
        changed = Some(part);
    }
    let p = ui.painter();
    let busy = response.dragged();
    p.rect_filled(rect, 10.0, Color32::from_black_alpha(if busy { 150 } else { 110 }));
    let track = rect.shrink2(vec2(15.0, 24.0));
    p.rect_filled(track, 4.0, theme::CONTROL());
    let filled = Rect::from_min_max(pos2(track.left(), track.bottom() - track.height() * part), track.max);
    p.rect_filled(filled, 4.0, ACCENT());
    let knob = pos2(track.center().x, track.bottom() - track.height() * part);
    p.circle_filled(knob, if busy { 10.0 } else { 8.0 }, Color32::WHITE);
    p.circle_stroke(knob, if busy { 10.0 } else { 8.0 }, Stroke::new(1.0, Color32::from_black_alpha(80)));
    p.text(pos2(rect.center().x, rect.top() + 11.0), Align2::CENTER_CENTER, name, theme::bold(9.5), theme::TEXT());
    p.text(pos2(rect.center().x, rect.bottom() - 11.0), Align2::CENTER_CENTER, shown, theme::font(10.5), theme::TEXT());
    changed
}
