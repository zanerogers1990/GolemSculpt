//! Looks and self-checks of the interface's own furniture, for runs made
//! without a person at the tablet (`--demo-ui`): the strength bar under the
//! brush wheel, the tabs that fold the panels away, cards pulled out of
//! their panel to float, and the status line drawn over the view while its
//! panel is folded away.
//!
//! `--demo-ui` takes a list, `key` or `key=value` with commas between:
//! `size=1.5` (interface size), `brush=Clay`, `mask`, `strength=0.3`,
//! `wheel` (the brush wheel in the middle of the window; `wheel=low` near
//! the bottom, where its bar goes above it; `wheel=left` at the left edge),
//! `fold=left`, `fold=right` or `fold=both` (panels folded away),
//! `theme=Clay` (the interface's colours, by the theme's name), `tab=Mesh`
//! or `tab=Look` (that tab of its panel showing, by the name on it; as many
//! as wanted),
//! `float=Brushes@300:200` (that card floating with its top-left corner
//! there, in the interface's points; as many as wanted), `status=error`,
//! `status=ok` or `status=fading` (a long error, a short success, or that
//! success as it fades, on the status line), `limit` (dynamic topology
//! stopped at its triangle limit) and `play`.
//!
//! `play` works the interface the way a pen would, through the same routines
//! real input goes through, prints a line per check and closes the app. The
//! real pointer must stay off the window while it runs.

use std::time::{Duration, Instant};

use egui::{Id, LayerId, Order, Pos2, pos2, vec2};
use glam::Vec2;
use winit::event::MouseButton;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;

use super::{Drag, Running};
use crate::doc::Tool;
use crate::sculpt::Brush;
use crate::ui::{status, tabs};
use sculpt_toolkit::floating;

/// What `status=error` and `status=ok` write, and `play` with them: made up,
/// but the length and the kind of thing the app says. Three sentences is as
/// long as its messages get.
const ERROR_SAMPLE: &str = "Could not save Dragon.sclp: the disk is full. Nothing was written, and the file from \
                            before is untouched. Free some space, or use Save As to put it somewhere else.";
const OK_SAMPLE: &str = "Saved Dragon.sclp";

/// Frames between the steps of `play`: the interface answers a press with
/// the layout of the frame before, and an area takes a frame to size itself.
const SETTLE_FRAMES: u32 = 4;
/// Frames `play` lets pass before its first step.
const LEAD_IN: u32 = 12;

#[derive(Default)]
pub(super) struct State {
    play: Option<Play>,
    /// What `play` noted in one step to compare with in a later one: the
    /// brush size and the camera's distance, and where a window was.
    kept: (f32, f32),
    at: Option<Pos2>,
}

/// Where `play` has got to.
struct Play {
    step: u32,
    /// Frames left before the next step.
    wait: u32,
    failed: u32,
}

/// Whether `spec` (the `--demo-ui` list) asks for the self-check: a run that
/// closes by itself, and so must not take the keyboard or the bridge's port.
pub(super) fn plays(spec: Option<&str>) -> bool {
    spec.is_some_and(|spec| spec.split(',').any(|item| item.trim() == "play"))
}

impl Running {
    /// Sets up what `--demo-ui` asked for, once, at start-up.
    pub(super) fn ui_demo(&mut self, spec: &str) {
        let mut wheel = None;
        for item in spec.split(',') {
            let (key, value) = item.split_once('=').unwrap_or((item, ""));
            let s = &mut self.doc.sculptor.settings;
            match (key.trim(), value.trim()) {
                ("size", size) => self.doc.opts.ui_scale = size.parse().unwrap_or(1.0),
                ("brush", name) => match Brush::ALL.iter().find(|b| b.name().eq_ignore_ascii_case(name)) {
                    Some(&brush) => s.brush = brush,
                    None => eprintln!("--demo-ui: no brush is called {name}"),
                },
                ("mask", _) => s.mask_mode = true,
                ("strength", strength) => s.current_mut().strength = strength.parse().unwrap_or(0.5),
                ("wheel", place) => wheel = Some(place.to_owned()),
                ("fold", which) => self.doc.opts.folded = [which != "right", which != "left"],
                ("theme", name) => match sculpt_toolkit::palette::named(name) {
                    Some(theme) => self.doc.opts.theme = theme,
                    None => eprintln!("--demo-ui: no theme is called {name}"),
                },
                ("tab", name) => {
                    let tabs = &mut self.doc.opts.tabs;
                    if let Some(tab) = tabs::Sculpt::ALL.into_iter().find(|tab| tab.name().eq_ignore_ascii_case(name)) {
                        tabs.sculpt = tab;
                    } else if let Some(tab) = tabs::Scene::ALL.into_iter().find(|tab| tab.name().eq_ignore_ascii_case(name)) {
                        tabs.scene = tab;
                    } else {
                        eprintln!("--demo-ui: no tab is called {name}");
                    }
                }
                ("float", card) => self.ui_demo_float(card),
                ("status", "ok") => self.status(OK_SAMPLE, true),
                ("status", "fading") => {
                    // Written long enough ago to be part of the way through its fade.
                    self.status(OK_SAMPLE, true);
                    self.ui.status_seen.since -= 7.0;
                }
                ("status", _) => self.status(ERROR_SAMPLE, false),
                ("limit", _) => (self.doc.sculptor.dyntopo.enabled, self.doc.sculptor.dyntopo.at_limit) = (true, true),
                ("play", _) => self.ui_demo.play = Some(Play { step: 0, wait: LEAD_IN, failed: 0 }),
                (other, _) => eprintln!("--demo-ui: unknown item {other}"),
            }
        }
        match wheel.as_deref() {
            Some("low") => self.ui_demo_wheel(Vec2::new(0.5, 0.92)),
            Some("left") => self.ui_demo_wheel(Vec2::new(0.0, 0.5)),
            Some(_) => self.ui_demo_wheel(Vec2::splat(0.5)),
            None => {}
        }
    }

    /// Floats a card: `name@x:y`, the top-left corner of its window.
    fn ui_demo_float(&mut self, card: &str) {
        let place = card.split_once('@').and_then(|(name, at)| {
            let (x, y) = at.split_once(':')?;
            Some((name, pos2(x.parse().ok()?, y.parse().ok()?)))
        });
        match place {
            Some((name, at)) => floating::float(&self.egui_ctx, name, at),
            None => eprintln!("--demo-ui: float wants a card and a place, as in float=Brushes@300:200"),
        }
    }

    /// Opens the hold-Space wheel for a pointer `place` of the way across
    /// and down the window (0 to 1 each way).
    fn ui_demo_wheel(&mut self, place: Vec2) {
        // In the interface's units, which no frame has set up yet.
        let per_point = self.window.scale_factor() as f32 * self.doc.opts.ui_scale;
        let at = (self.view.origin + self.view.size * place) / per_point;
        self.ui.radial = Some(pos2(at.x, at.y));
    }

    /// Makes the last tap on an interface control one that ended `ago` ago:
    /// `play` cannot count on how long its frames take on a busy machine.
    fn ui_demo_tap_was(&mut self, ago: Duration) {
        if let Some(tap) = &mut self.input.ui_tap {
            tap.at = Instant::now().checked_sub(ago).unwrap_or(tap.at);
        }
    }

    /// Moves the pointer to `at` (in the interface's points), for the
    /// interface and for the app alike.
    fn ui_demo_move(&mut self, at: Pos2) {
        let ppp = self.egui_ctx.pixels_per_point();
        self.egui_state.egui_input_mut().events.push(egui::Event::PointerMoved(at));
        self.input.inside = true;
        self.pointer_moved(Vec2::new(at.x, at.y) * ppp, None);
    }

    /// Presses or lets go of the pen at `at`.
    fn ui_demo_button(&mut self, at: Pos2, pressed: bool) {
        self.ui_demo_move(at);
        let button = egui::PointerButton::Primary;
        let modifiers = egui::Modifiers::default();
        self.egui_state.egui_input_mut().events.push(egui::Event::PointerButton { pos: at, button, pressed, modifiers });
        if pressed {
            self.pointer_pressed(MouseButton::Left, None);
        } else {
            self.pointer_released(MouseButton::Left);
        }
    }

    /// Where an area of the interface was last drawn, if it is showing.
    fn ui_demo_area(&self, id: impl std::hash::Hash + std::fmt::Debug, order: Order) -> Option<egui::Rect> {
        let id = Id::new(id);
        self.egui_ctx.memory(|m| m.areas().is_visible(&LayerId::new(order, id)).then(|| m.area_rect(id)).flatten())
    }

    /// Start of a frame: the next step of `play`, when one is due.
    pub(super) fn ui_demo_frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut play) = self.ui_demo.play.take() else { return };
        self.redraw = true;
        if play.wait > 0 {
            play.wait -= 1;
            self.ui_demo.play = Some(play);
            return;
        }
        let mut failed = 0;
        let done = self.ui_demo_step(play.step, &mut |what: &str, ok: bool| {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
            failed += u32::from(!ok);
        });
        play.failed += failed;
        if done {
            println!("ui play: done, {} failed", play.failed);
            return event_loop.exit();
        }
        play.step += 1;
        play.wait = SETTLE_FRAMES;
        self.ui_demo.play = Some(play);
    }

    /// One step of `play`. Each acts, and checks what the step before it
    /// set going. True once there are no more.
    fn ui_demo_step(&mut self, step: u32, expect: &mut dyn FnMut(&str, bool)) -> bool {
        let strength = |running: &Running| running.doc.sculptor.settings.current().strength;
        let brush = |running: &Running| running.doc.sculptor.settings.brush;
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        let bar = self.ui_demo_area("radial_strength", Order::Foreground);
        let wheel = self.ui_demo_area("radial_menu", Order::Foreground);
        // A point of the bar's track: the plate is 12 points wider than the
        // slider at each end, and the track 7 inside that (ui/overlay.rs).
        let on_bar = |t: f32| bar.map(|plate| pos2(plate.left() + 19.0 + (plate.width() - 38.0) * t, plate.center().y));
        // The tile at the top of the wheel (Move): 104 points out.
        let top_tile = wheel.map(|rect| rect.center() - vec2(0.0, 104.0));
        match step {
            0 => {
                let s = &mut self.doc.sculptor.settings;
                (s.brush, s.mask_mode) = (Brush::Clay, false);
                s.current_mut().strength = 0.5;
                self.ui_demo_wheel(Vec2::splat(0.5));
            }
            1 => {
                expect("the brush wheel has a strength bar under it", bar.zip(wheel).is_some_and(|(b, w)| b.top() >= w.bottom()));
                if let Some(at) = on_bar(0.75) {
                    self.ui_demo_button(at, true);
                }
            }
            2 => {
                expect("a press three quarters along the bar sets Strength to 75%", near(strength(self), 0.7525));
                expect("and is not a stroke on the model behind", self.input.drag == Drag::Ui && !self.doc.sculptor.is_active());
                if let Some(at) = on_bar(0.25) {
                    self.ui_demo_move(at);
                }
            }
            3 => {
                expect("dragging back to a quarter sets it to 26%", near(strength(self), 0.2575));
                if let Some(at) = on_bar(-0.5) {
                    self.ui_demo_move(at);
                }
            }
            4 => {
                expect("dragging past the left end stops at 1%", near(strength(self), 0.01));
                // Still pressed, up onto a tile of the wheel, and let go there.
                if let Some(at) = top_tile {
                    self.ui_demo_button(at, false);
                }
            }
            5 => {
                expect("a drag from the bar that ends on a tile does not pick that brush", brush(self) == Brush::Clay);
                expect("and the wheel stays up", self.ui.radial.is_some());
                if let Some(at) = on_bar(0.9) {
                    self.ui_demo_button(at, true);
                }
            }
            6 => {
                if let Some(at) = on_bar(1.5) {
                    self.ui_demo_move(at);
                }
            }
            7 => {
                expect("dragging past the right end stops at 100%", near(strength(self), 1.0));
                // Space comes up with the pen still down, off the bar's end.
                self.key_released(KeyCode::Space);
            }
            8 => {
                expect("letting go of Space in the middle of that closes the wheel", self.ui.radial.is_none());
                expect("and leaves the brush and its strength alone", brush(self) == Brush::Clay && near(strength(self), 1.0));
                let ppp = self.egui_ctx.pixels_per_point();
                self.ui_demo_button(pos2(self.input.pointer.x / ppp, self.input.pointer.y / ppp), false);
                self.doc.sculptor.settings.brush = Brush::Pose;
                self.ui_demo_wheel(Vec2::splat(0.5));
            }
            9 => {
                expect("the Pose brush, which has no use for Strength, gets no bar", bar.is_none() && wheel.is_some());
                self.doc.sculptor.settings.mask_mode = true;
            }
            10 => {
                expect("mask painting does, whatever brush is under it", bar.is_some());
                self.doc.sculptor.settings.mask_mode = false;
                if let Some(at) = top_tile {
                    self.ui_demo_button(at, true);
                }
            }
            11 => {
                if let Some(at) = top_tile {
                    self.ui_demo_button(at, false);
                }
            }
            12 => {
                let picked = brush(self) == Brush::Move && self.ui.radial.is_none();
                expect("a tap on a tile still picks its brush and closes the wheel", picked);
            }
            _ => return self.ui_demo_fold_step(step - 13, expect),
        }
        false
    }

    /// The steps of `play` that fold the panels away and bring them back.
    fn ui_demo_fold_step(&mut self, step: u32, expect: &mut dyn FnMut(&str, bool)) -> bool {
        let tab = |running: &Running, side: &str| running.ui_demo_area(("panel_tab", side), Order::Middle);
        let panel = |running: &Running, id: &str| running.ui_demo_area(id, Order::Middle);
        let folded = self.doc.opts.folded;
        // A place the Sculpt panel covers while it shows, clear of its tab.
        let under_panel = pos2(120.0, 120.0);
        match step {
            0 => {
                expect("both panels show, each with a tab", panel(self, "sculpt_panel").zip(panel(self, "scene_panel")).is_some());
                self.ui_demo_move(under_panel);
            }
            1 => {
                expect("the pointer over the Sculpt panel is over the interface", self.over_ui());
                if let Some(tab) = tab(self, "left") {
                    self.ui_demo_button(tab.center(), true);
                }
            }
            2 => {
                expect("a press on the tab is not a stroke or an orbit", self.input.drag == Drag::Ui);
                if let Some(tab) = tab(self, "left") {
                    self.ui_demo_button(tab.center(), false);
                }
            }
            3 => {
                expect("a tap on its tab folds the Sculpt panel away", folded == [true, false] && panel(self, "sculpt_panel").is_none());
                expect("and leaves a tab at the left edge of the window", tab(self, "left").is_some_and(|tab| tab.left() <= 0.5));
                expect("the Scene panel stays", panel(self, "scene_panel").is_some());
                self.ui_demo_move(under_panel);
            }
            4 => {
                expect("where the panel was is viewport again", !self.over_ui());
                self.ui_demo_button(under_panel, true);
            }
            5 => {
                expect("and a drag there orbits, as on any background", self.input.drag == Drag::Orbit && !self.doc.opts.clean_view);
                self.ui_demo_button(under_panel, false);
                crate::turntable::set_clean_view(&mut self.doc, true);
            }
            6 => {
                expect("the clean view (Tab) takes the tabs away too", tab(self, "left").or(tab(self, "right")).is_none());
                crate::turntable::set_clean_view(&mut self.doc, false);
            }
            7 => {
                let back = tab(self, "left").zip(tab(self, "right")).is_some();
                expect("and brings them back with the panel still folded", back && self.doc.opts.folded == [true, false]);
                self.doc.opts.ui_scale = 2.0;
            }
            8 => {
                let screen = self.egui_ctx.content_rect();
                let on_screen = |rect: Option<egui::Rect>| rect.is_some_and(|rect| screen.contains_rect(rect));
                expect("at twice the interface size both tabs are still on screen", on_screen(tab(self, "left")) && on_screen(tab(self, "right")));
                self.doc.opts.ui_scale = 1.0;
            }
            9 => {
                if let Some(tab) = tab(self, "right") {
                    self.ui_demo_button(tab.center(), true);
                }
            }
            10 => {
                if let Some(tab) = tab(self, "right") {
                    self.ui_demo_button(tab.center(), false);
                }
            }
            11 => {
                expect("the Scene panel folds away on its own tab", folded == [true, true] && panel(self, "scene_panel").is_none());
                let gizmo = panel(self, "view_gizmo");
                let clear = gizmo.zip(tab(self, "right")).is_some_and(|(gizmo, tab)| !gizmo.intersects(tab));
                expect("the view gizmo moves out beside the tab, not under it", clear);
                let kept = self.prefs_now();
                expect("the settings remember both", kept.left_panel_folded && kept.right_panel_folded);
                if let Some(tab) = tab(self, "left") {
                    self.ui_demo_button(tab.center(), true);
                }
            }
            12 => {
                if let Some(tab) = tab(self, "left") {
                    self.ui_demo_button(tab.center(), false);
                }
            }
            13 => {
                expect("a tap on the tab at the edge brings the Sculpt panel back", folded == [false, true] && panel(self, "sculpt_panel").is_some());
                self.doc.opts.folded = [false; 2];
            }
            _ => return self.ui_demo_float_step(step - 14, expect),
        }
        false
    }

    /// The steps of `play` that pull a card out of its panel, move it, keep
    /// it through folded panels, the clean view and a larger interface, and
    /// put it back.
    fn ui_demo_float_step(&mut self, step: u32, expect: &mut dyn FnMut(&str, bool)) -> bool {
        let ctx = self.egui_ctx.clone();
        let window = floating::window(&ctx, "Brushes");
        let button = floating::button(&ctx, "Brushes").map(|rect| rect.center());
        let panel = self.ui_demo_area("sculpt_panel", Order::Middle);
        let screen = ctx.content_rect();
        // By its title, clear of the button in its corner.
        let grip = window.map(|rect| rect.min + vec2(70.0, 16.0));
        let near = |a: Pos2, b: Pos2| (a - b).length() < 8.0;
        let ppp = ctx.pixels_per_point();
        let here = pos2(self.input.pointer.x / ppp, self.input.pointer.y / ppp);
        let settings = &self.doc.sculptor.settings;
        let (size, zoom) = (settings.size(), self.doc.camera.distance);
        match step {
            0 => {
                self.doc.sculptor.settings.brush = Brush::Clay;
                expect("the Brushes card has a button in its header", button.zip(panel).is_some_and(|(at, panel)| panel.contains(at)));
                if let Some(at) = button {
                    self.ui_demo_button(at, true);
                }
            }
            1 => self.ui_demo_button(here, false),
            2 => {
                expect("a tap on it pulls the card out into a window of its own", floating::is_floating(&ctx, "Brushes") && window.is_some());
                let beside = window.zip(panel).is_some_and(|(window, panel)| !window.intersects(panel));
                expect("which appears beside the panel, not over it", beside);
                if let Some(at) = grip {
                    self.ui_demo_button(at, true);
                }
            }
            3 => {
                expect("a press on its title is not a stroke or an orbit", self.input.drag == Drag::Ui);
                // Over the middle of the model.
                let to = screen.center() - vec2(60.0, 120.0) + vec2(70.0, 16.0);
                self.ui_demo_move(here.lerp(to, 0.5));
                self.ui_demo_move(to);
            }
            4 => {
                self.ui_demo_button(here, false);
            }
            5 => {
                let over_model = window.is_some_and(|rect| near(rect.min, screen.center() - vec2(60.0, 120.0)));
                expect("dragged by its title it goes where the pen goes", over_model);
                // A tile of the card, with the model right behind it.
                if let Some(rect) = window {
                    self.ui_demo_button(rect.min + vec2(40.0, 70.0), true);
                }
            }
            6 => {
                expect("the pointer on the floating card is over the interface", self.over_ui());
                expect("a press on it does not reach the model behind", self.input.drag == Drag::Ui && !self.doc.sculptor.is_active());
                self.ui_demo_button(here, false);
            }
            7 => {
                expect("but it is the card's own: the tap picked the brush under it", self.doc.sculptor.settings.brush == Brush::Move);
                self.ui_demo.kept = (size, zoom);
                self.wheel(1.0);
                self.wheel(-3.0);
            }
            8 => {
                let (was_size, was_zoom) = self.ui_demo.kept;
                expect("the scroll wheel over it neither sizes the brush nor zooms", size == was_size && zoom == was_zoom);
                self.doc.opts.folded = [true; 2];
                self.ui_demo.at = window.map(|rect| rect.min);
            }
            9 => {
                let stayed = window.map(|rect| rect.min) == self.ui_demo.at && window.is_some();
                expect("with both panels folded away the floating card stays", stayed && panel.is_none());
                crate::turntable::set_clean_view(&mut self.doc, true);
            }
            10 => {
                expect("the clean view (Tab) takes it away with everything else", window.is_none() && floating::is_floating(&ctx, "Brushes"));
                crate::turntable::set_clean_view(&mut self.doc, false);
            }
            11 => {
                expect("and brings it back where it was", window.map(|rect| rect.min) == self.ui_demo.at && window.is_some());
                floating::float(&ctx, "Brushes", screen.max - vec2(120.0, 60.0));
            }
            12 => {
                expect("left hanging off the corner it is drawn back inside the window", window.is_some_and(|rect| screen.contains_rect(rect.shrink(1.0))));
                // A place that is on the window now, and off it at twice the size.
                let place = pos2((screen.width() * 0.65).round(), 300.0);
                floating::float(&ctx, "Brushes", place);
                self.ui_demo.at = Some(place);
                self.doc.opts.ui_scale = 2.0;
            }
            13 => {
                let inside = window.is_some_and(|rect| screen.contains_rect(rect.shrink(1.0)));
                let off = self.ui_demo.at.is_some_and(|place| place.x > screen.width() - 100.0);
                expect("at twice the interface size, where its place is off the window, it is inside too", inside && off);
                self.doc.opts.ui_scale = 1.0;
            }
            14 => {
                let place = self.ui_demo.at.unwrap_or_default();
                expect("and goes back out when the interface is small again", window.is_some_and(|rect| near(rect.min, place)));
                let kept = self.prefs_now().floating_cards;
                expect("the settings remember it and where", kept.get("Brushes") == Some(&[place.x, place.y]) && kept.len() == 1);
                self.set_tool(Tool::SSpheres);
            }
            15 => {
                expect("with another tool's panel up the Brushes card is away", window.is_none() && floating::is_floating(&ctx, "Brushes"));
                floating::float(&ctx, "Spheres", pos2(400.0, 80.0));
            }
            16 => {
                expect("and a card of that tool's floats while the tool is up", floating::window(&ctx, "Spheres").is_some());
                self.set_tool(Tool::Sculpt);
            }
            17 => {
                let swapped = window.is_some() && floating::window(&ctx, "Spheres").is_none();
                expect("back in Sculpt the tool's card has gone and Brushes is back", swapped && floating::is_floating(&ctx, "Spheres"));
                floating::dock(&ctx, "Spheres");
                if let Some(at) = button {
                    self.ui_demo_button(at, true);
                }
            }
            18 => self.ui_demo_button(here, false),
            19 => {
                expect("the button in its corner puts it back, with its panel still folded", !floating::is_floating(&ctx, "Brushes") && window.is_none());
                expect("and the settings forget it", self.prefs_now().floating_cards.is_empty());
                self.doc.opts.folded = [false; 2];
            }
            20 => {
                let back = button.zip(panel).is_some_and(|(at, panel)| panel.contains(at));
                expect("unfolded, the card is in the Sculpt panel again", back);
            }
            _ => return self.ui_demo_status_step(step - 21, expect),
        }
        false
    }

    /// The steps of `play` for what a folded panel would otherwise leave
    /// unsaid: the status line over the view while the Scene panel is away
    /// (an error that stays, a success that fades, a tap that puts it away),
    /// and the Sculpt panel's triangle-limit note.
    fn ui_demo_status_step(&mut self, step: u32, expect: &mut dyn FnMut(&str, bool)) -> bool {
        let ctx = self.egui_ctx.clone();
        let screen = ctx.content_rect();
        let plate = self.ui_demo_area(status::AREA, Order::Foreground);
        let area = |id: &str| self.ui_demo_area(id, Order::Middle);
        let tab = |side: &str| self.ui_demo_area(("panel_tab", side), Order::Middle);
        // Everything of the interface's own that the plate must keep off.
        let others = [area("view_gizmo"), area("bottom_bar"), area("sculpt_panel"), area("scene_panel"), tab("left"), tab("right")];
        let scene_panel = area("scene_panel");
        let clear = |plate: egui::Rect| screen.contains_rect(plate) && others.iter().flatten().all(|other| !other.intersects(plate));
        let ppp = ctx.pixels_per_point();
        let here = pos2(self.input.pointer.x / ppp, self.input.pointer.y / ppp);
        match step {
            0 => {
                self.doc.opts.folded = [false, true];
                self.status(ERROR_SAMPLE, false);
            }
            1 => {
                expect("with the Scene panel folded away an error is still on screen", plate.is_some());
                expect("clear of the view gizmo, the tabs, the bottom bar and the Sculpt panel", plate.is_some_and(clear));
                let wrapped = plate.is_some_and(|plate| plate.width() <= 441.0 && plate.height() > 40.0);
                expect("its three sentences wrapped, not run across the view", wrapped);
                if let Some(plate) = plate {
                    self.ui_demo_move(plate.center_bottom() + vec2(0.0, 14.0));
                }
            }
            2 => {
                expect("the view right under it is still the pen's", plate.is_some() && !self.over_ui());
                self.ui.status_seen.since -= 600.0;
            }
            3 => {
                expect("ten minutes on, the error is still there to be read", plate.is_some());
                self.ui_demo.kept.0 = plate.map_or(0.0, |plate| plate.height());
                self.status(OK_SAMPLE, true);
            }
            4 => {
                expect("a success takes its place, on one line", plate.is_some_and(|plate| plate.height() < self.ui_demo.kept.0));
                self.ui.status_seen.since -= 600.0;
            }
            5 => {
                expect("and has faded away by the time it is old news", plate.is_none());
                self.status(ERROR_SAMPLE, false);
            }
            6 => {
                if let Some(plate) = plate {
                    self.ui_demo_button(plate.center(), true);
                }
            }
            7 => {
                expect("a press on the message is the interface's, not a stroke", self.input.drag == Drag::Ui && !self.doc.sculptor.is_active());
                self.ui_demo_button(here, false);
            }
            8 => {
                expect("a tap on it puts it away", plate.is_none());
                // The second tap of a double tap, where the message was.
                self.ui_demo_tap_was(Duration::ZERO);
                self.ui_demo_button(here, true);
            }
            9 => {
                let ignored = self.input.drag == Drag::Ui && !self.over_ui() && !self.doc.sculptor.is_active();
                expect("and a second tap where it was is not a stroke or an orbit", ignored);
                self.ui_demo_button(here, false);
                self.status(ERROR_SAMPLE, false);
            }
            10 => {
                expect("the same error written again is back up", plate.is_some());
                self.doc.opts.folded = [true, true];
            }
            11 => {
                expect("with both panels folded away it is there, clear of both tabs, the gizmo and the bottom bar", plate.is_some_and(clear));
                self.doc.opts.ui_scale = 2.0;
            }
            12 => {
                expect("and at twice the interface size", plate.is_some_and(clear));
                self.doc.opts.ui_scale = 1.0;
                crate::turntable::set_clean_view(&mut self.doc, true);
            }
            13 => {
                expect("the clean view (Tab) takes it away with the rest", plate.is_none());
                crate::turntable::set_clean_view(&mut self.doc, false);
                self.doc.opts.folded = [true, false];
            }
            14 => {
                expect("with the Scene panel back the message is on its status line, not over the view", plate.is_none() && scene_panel.is_some());
                let dyntopo = &mut self.doc.sculptor.dyntopo;
                (dyntopo.enabled, dyntopo.at_limit) = (true, true);
            }
            15 => {
                let said = plate.is_some_and(clear);
                expect("with the Sculpt panel folded away, dynamic topology stopping at its limit is said over the view", said);
                if let Some(plate) = plate {
                    self.ui_demo_move(plate.center());
                }
            }
            16 => {
                expect("that note goes by itself, so the pen goes through it to the model", plate.is_some() && !self.over_ui());
                let dyntopo = &mut self.doc.sculptor.dyntopo;
                (dyntopo.enabled, dyntopo.at_limit) = (false, false);
                self.doc.opts.folded = [false; 2];
            }
            _ => return self.ui_demo_card_step(step - 17, expect),
        }
        false
    }

    /// The steps of `play` for a card asked to open while its panel is
    /// folded away: Measure's, which opens itself when the tool comes up.
    fn ui_demo_card_step(&mut self, step: u32, expect: &mut dyn FnMut(&str, bool)) -> bool {
        let ctx = self.egui_ctx.clone();
        // Whether the Measure & Size card is open in the Scene panel. It
        // starts closed, so it is among the cards that are not as they start
        // out exactly when it is open.
        let open = crate::prefs::cards::snapshot(&ctx).get("Measure & Size") == Some(&true);
        match step {
            0 => {
                let drawn = floating::button(&ctx, "Measure & Size").is_some();
                expect("the Measure & Size card starts closed", drawn && !open);
                // The Scene panel away, and run out of sight because a card floats.
                self.doc.opts.folded = [false, true];
                floating::float(&ctx, "Brushes", pos2(400.0, 120.0));
            }
            1 => {
                let measure = self.doc.tools.info_index("measure");
                expect("there is a Measure tool to bring up", measure.is_some());
                if let Some(index) = measure {
                    self.tools_activate(index);
                }
            }
            2 => self.doc.opts.folded = [false; 2],
            3 => {
                expect("brought up while the Scene panel was folded away, Measure finds its card open when the panel is back", open);
                floating::dock(&ctx, "Brushes");
                self.tools_leave();
            }
            _ => return self.ui_demo_short_step(step - 4, expect),
        }
        false
    }

    /// The steps of `play` on a window too short for the brush wheel and its
    /// bar. The window stays as it is: the interface is made larger instead,
    /// which leaves it as few points to work in.
    fn ui_demo_short_step(&mut self, step: u32, expect: &mut dyn FnMut(&str, bool)) -> bool {
        let screen = self.egui_ctx.content_rect();
        let bar = self.ui_demo_area("radial_strength", Order::Foreground);
        let wheel = self.ui_demo_area("radial_menu", Order::Foreground);
        // The interface size at which the window is `points` high.
        let size_for = |points: f32| self.doc.opts.ui_scale * screen.height() / points;
        match step {
            0 => {
                let s = &mut self.doc.sculptor.settings;
                (s.brush, s.mask_mode) = (Brush::Clay, false);
                self.ui_demo.kept.0 = self.doc.opts.ui_scale;
                // Under 362 points there is no room for the bar under the wheel or over it.
                self.doc.opts.ui_scale = size_for(330.0);
            }
            // At the left edge: the only room for the bar is beside the wheel.
            1 => self.ui.radial = Some(pos2(0.0, screen.center().y)),
            2 => {
                let whole = wheel.is_some_and(|wheel| screen.expand(0.5).contains_rect(wheel));
                expect("on a window too short for a bar under the wheel or over it, the wheel still opens whole", whole);
                let beside = bar.zip(wheel).is_some_and(|(bar, wheel)| screen.expand(0.5).contains_rect(bar) && !bar.intersects(wheel));
                expect("and its Strength bar is beside it: on the window and off the tiles", beside);
                self.ui.radial = None;
                self.doc.opts.ui_scale = size_for(200.0);
            }
            3 => self.ui.radial = Some(screen.center()),
            4 => {
                expect("on a window shorter than the wheel itself, holding Space still opens it", wheel.is_some());
                expect("with no bar, as there is no room for one", bar.is_none());
                self.ui.radial = None;
                self.doc.opts.ui_scale = self.ui_demo.kept.0;
            }
            _ => return self.ui_demo_echo_step(step - 5, expect),
        }
        false
    }

    /// The last steps of `play`: the second tap of a double tap on a fold
    /// tab, which lands on the view once the panel has gone.
    fn ui_demo_echo_step(&mut self, step: u32, expect: &mut dyn FnMut(&str, bool)) -> bool {
        let tab = self.ui_demo_area(("panel_tab", "left"), Order::Middle);
        let ppp = self.egui_ctx.pixels_per_point();
        let here = pos2(self.input.pointer.x / ppp, self.input.pointer.y / ppp);
        match step {
            0 => {
                if let Some(tab) = tab {
                    self.ui_demo_button(tab.center(), true);
                }
            }
            1 => self.ui_demo_button(here, false),
            2 => {
                expect("a tap on its tab folds the Sculpt panel away from under the pen", self.doc.opts.folded == [true, false] && !self.over_ui());
                self.ui_demo_tap_was(Duration::ZERO);
                self.ui_demo_button(here, true);
            }
            3 => {
                let ignored = self.input.drag == Drag::Ui && !self.over_ui() && !self.doc.sculptor.is_active();
                expect("a second tap where the tab was a moment ago is not a stroke or an orbit", ignored);
                self.ui_demo_button(here, false);
                self.ui_demo_tap_was(Duration::from_secs(1));
                self.ui_demo_button(here, true);
            }
            4 => {
                expect("a second later, a press in that place is the view's again", matches!(self.input.drag, Drag::Orbit | Drag::Sculpt));
                self.ui_demo_button(here, false);
                self.doc.opts.folded = [false; 2];
            }
            _ => return true,
        }
        false
    }
}
