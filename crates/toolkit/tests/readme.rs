//! The "complete minimal tool" of README.md, compiled and driven, so the
//! guide cannot drift from the interface. Keep the two the same.

use sculpt_toolkit::testing::{Bench, cube};
use sculpt_toolkit::{Button, Color32, Command, Ctx, Draw, Event, Info, Key, NewObject, Preview, Scene, Style, Tool, theme};

#[derive(Default)]
pub struct Dropper {
    ghost: Preview,
    at: Option<glam::Mat4>,
}

pub fn tool() -> Box<dyn Tool> {
    Box::new(Dropper::default())
}

impl Tool for Dropper {
    fn info(&self) -> Info {
        Info { id: "dropper", name: "Dropper", icon: "prim_cube", tip: "Click the model to drop a cube on it.", hotkey: None }
    }

    fn enter(&mut self, _cx: &mut Ctx) {
        self.ghost.set(cube(0.05));
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::Press(Button::Left) => {
                let Some(hit) = cx.pick(cx.pointer) else { return false }; // not on the model: orbit
                let mut mesh = cube(0.05);
                mesh.transform(&glam::Mat4::from_translation(hit.position));
                cx.commit("Drop Cube", vec![Command::Add(NewObject::new("Cube", mesh).unselected())]);
                true
            }
            Event::KeyDown(Key::Enter) => {
                cx.finish();
                true
            }
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        if !cx.active {
            return;
        }
        self.at = cx.pick(cx.pointer).filter(|_| cx.hovering).map(|hit| glam::Mat4::from_translation(hit.position));
        if let Some(at) = self.at {
            draw.instances(&self.ghost, Style::shell(Color32::from_rgba_unmultiplied(120, 170, 255, 110)), &[at]);
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        theme::category(ui, "Dropper", theme::ACCENT, "", true, |ui| {
            theme::hint(ui, "Click the model to drop a cube. Each is one undo step.");
            if theme::button(ui, ui.available_width(), "Done", Default::default()).clicked() {
                cx.finish();
            }
        });
    }
}

#[test]
fn the_readme_tool_drops_a_cube_per_click_and_each_is_one_undo_step() {
    let mut bench = Bench::new();
    let base = bench.add("Base", cube(0.5));
    let mut tool = tool();
    bench.enter(tool.as_mut());

    let on_model = bench.px(glam::Vec3::new(0.2, 0.1, 0.5));
    bench.hover(on_model);
    assert_eq!(bench.frame(tool.as_mut()).previews().len(), 1, "the ghost follows the pointer");

    bench.click(tool.as_mut(), on_model, Button::Left);
    assert_eq!(bench.doc.objects().len(), 2);
    assert_eq!(bench.doc.active(), base, "the dropped cube was added unselected");
    assert!(!bench.press(tool.as_mut(), glam::Vec2::new(4.0, 4.0), Button::Left), "off the model the press is passed on");
    bench.release(tool.as_mut());

    assert!(bench.undo(tool.as_mut()));
    assert_eq!(bench.doc.objects().len(), 1);
    assert!(bench.key(tool.as_mut(), Key::Enter) && bench.finished);
}

/// The panel is real egui: build it once, headless, to prove it lays out.
#[test]
fn the_readme_panel_builds() {
    let mut bench = Bench::new();
    bench.add("Base", cube(0.5));
    let mut tool = tool();
    let ctx = egui::Context::default();
    // The theme's faces (the bold one in particular) have to be installed
    // before any of its widgets is drawn, as the app does at start-up.
    theme::install(&ctx);
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        bench.call(|cx| tool.panel(ui, cx));
    });
    assert!(!output.shapes.is_empty(), "the panel drew something");
    // No renderer here to take the font atlas; say so, or egui objects.
    output.textures_delta.clear();
}
