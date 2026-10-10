//! Looks and self-checks of the interface's own furniture, for runs made
//! without a person at the tablet (`--demo-ui`): the strength bar under the
//! brush wheel, and the tabs that fold the panels away.
//!
//! `--demo-ui` takes a list, `key` or `key=value` with commas between:
//! `size=1.5` (interface size), `brush=Clay`, `mask`, `strength=0.3`,
//! `wheel` (the brush wheel in the middle of the window; `wheel=low` near
//! the bottom, where its bar goes above it), `fold=left`, `fold=right` or
//! `fold=both` (panels folded away) and `play`.
//!
//! `play` works the interface the way a pen would, through the same routines
//! real input goes through, prints a line per check and closes the app. The
//! real pointer must stay off the window while it runs.

use egui::{Id, LayerId, Order, Pos2, pos2, vec2};
use glam::Vec2;
use winit::event::MouseButton;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;

use super::{Drag, Running};
use crate::sculpt::Brush;

/// Frames between the steps of `play`: the interface answers a press with
/// the layout of the frame before, and an area takes a frame to size itself.
const SETTLE_FRAMES: u32 = 4;
/// Frames `play` lets pass before its first step.
const LEAD_IN: u32 = 12;

#[derive(Default)]
pub(super) struct State {
    play: Option<Play>,
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
                ("play", _) => self.ui_demo.play = Some(Play { step: 0, wait: LEAD_IN, failed: 0 }),
                (other, _) => eprintln!("--demo-ui: unknown item {other}"),
            }
        }
        if let Some(place) = wheel {
            self.ui_demo_wheel(if place == "low" { 0.92 } else { 0.5 });
        }
    }

    /// Opens the hold-Space wheel `down` of the way down the middle of the
    /// window.
    fn ui_demo_wheel(&mut self, down: f32) {
        // In the interface's units, which no frame has set up yet.
        let per_point = self.window.scale_factor() as f32 * self.doc.opts.ui_scale;
        let at = (self.view.origin + self.view.size * Vec2::new(0.5, down)) / per_point;
        self.ui.radial = Some(pos2(at.x, at.y));
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
                self.ui_demo_wheel(0.5);
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
                self.ui_demo_wheel(0.5);
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
            _ => return true,
        }
        false
    }
}
