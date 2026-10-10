//! The Measure tool: distances on the model in real-world units.
//!
//! Rulers placed in the viewport read out real lengths (mm or inches, see
//! `sculpt_geom::units`) for checking a model against the thing it is going
//! to be printed as, and the Measure & Size card gives the selected object's
//! real printed size, typed straight into Width / Height / Depth to resize
//! it.
//!
//! While the tool is up it has the pointer:
//!
//! | Input | Action |
//! | --- | --- |
//! | Drag on the model | Lay a new ruler from the press to the release |
//! | Click, then click again | The same, one end per click (either may be off the model) |
//! | Drag an end | Re-measure from that end; the other stays put |
//! | Click a ruler | Select it (its X / Y / Z breakdown shows) |
//! | Right-click a ruler | Delete it (Delete / Backspace deletes the selected one) |
//! | Shift while dragging | Lock the ruler to the nearest world axis |
//! | Ctrl while dragging | Do not snap to the surface |
//! | U | Millimetres / inches |
//! | Esc | Cancel the drag or placement in progress, else deselect, else leave |
//!
//! L brings the tool up and puts it away. A drag on empty background orbits,
//! as everywhere in the app; this is the one change from the Unity tool,
//! where it laid a ruler in mid-air (Ctrl + drag still does).

mod panel;
pub mod rulers;
pub mod size;

use sculpt_toolkit::{Button, Ctx, Draw, Event, Info, Key, Tool};

pub use rulers::{Ruler, Rulers};

#[derive(Default)]
pub struct Measure {
    rulers: Rulers,
    size: panel::SizeCard,
}

/// The tool, ready to register with the app.
pub fn tool() -> Box<dyn Tool> {
    Box::new(Measure::default())
}

impl Measure {
    pub fn rulers(&self) -> &Rulers {
        &self.rulers
    }
}

impl Tool for Measure {
    fn info(&self) -> Info {
        Info {
            id: "measure",
            name: "Measure",
            icon: "tool_measure",
            tip: "Rulers that read real lengths in mm or inches: drag across the model to measure (L).",
            hotkey: Some(Key::L),
        }
    }

    fn enter(&mut self, cx: &mut Ctx) {
        // The card with the readout and the object's size opens itself: now
        // is when it is wanted.
        self.size.reveal();
        cx.status("Drag across the model to lay a ruler. Drag either end to re-measure.", true);
    }

    fn leave(&mut self, _cx: &mut Ctx) {
        self.rulers.cancel_drag();
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::Press(Button::Left) => self.rulers.press(cx, cx.pointer, cx.mods.ctrl),
            Event::Press(Button::Right) => self.rulers.right_press(&cx.view, cx.pointer),
            Event::Drag => {
                self.rulers.drag_to(cx, cx.pointer, cx.mods.shift, cx.mods.ctrl);
                true
            }
            Event::Release => {
                self.rulers.release();
                true
            }
            // A click that was not on the model or a ruler starts one in
            // mid-air; a drag there is the camera's.
            Event::Click(Button::Left) => self.rulers.click_off_model(cx, cx.pointer),
            Event::KeyDown(Key::Escape) => self.rulers.cancel_drag() || self.rulers.deselect(),
            Event::KeyDown(Key::U) => {
                let unit = panel::other_unit(cx.unit);
                cx.set_unit(unit);
                cx.toast(panel::unit_name(unit));
                true
            }
            Event::KeyDown(Key::Delete | Key::Backspace) => !self.rulers.is_dragging() && self.rulers.delete_selected(),
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        if cx.active {
            self.rulers.frame(cx, draw);
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        panel::panel(&mut self.rulers, ui, cx);
    }

    fn card(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        self.size.show(ui, cx);
    }

    /// Two rulers across the selected object: one locked to an axis, then a
    /// slanted one, left selected so its breakdown shows. `inches` switches
    /// the unit; `placing` leaves a third ruler following the pointer.
    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        let scene = cx.scene();
        let bounds = scene.bounds(scene.active());
        if bounds.is_empty() {
            return;
        }
        let (centre, reach) = (bounds.center(), bounds.size().max_element() * 0.5);
        let view = cx.view;
        let at = |right: f32, up: f32| view.project(centre + (view.right * right + view.up * up) * reach);
        let strokes = [(at(-0.25, 0.75), at(0.25, -0.7), true), (at(-0.75, -0.15), at(0.7, 0.3), false)];
        for (from, to, lock) in strokes {
            let (Some(from), Some(to)) = (from, to) else { continue };
            if self.rulers.press(cx, from, false) {
                for step in 1..=4 {
                    self.rulers.drag_to(cx, from.lerp(to, step as f32 / 4.0), lock, false);
                }
                self.rulers.release();
            }
        }
        if what == "inches" {
            cx.set_unit(sculpt_geom::LengthUnit::Inches);
        }
        if what == "placing" {
            if let Some(px) = at(0.1, 0.5) {
                self.rulers.press(cx, px, false);
                self.rulers.release();
            }
        }
    }
}

#[cfg(test)]
mod tests;
