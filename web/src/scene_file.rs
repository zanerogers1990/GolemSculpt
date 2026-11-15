//! The whole scene as a file: Save Scene downloads it, and Open (or a drop
//! on the page) takes one back.
//!
//! The file is the scene the browser keeps between visits (`scene_bytes`,
//! layout W03): every object with its place, its mesh, its mask, its hidden
//! faces, its subdivision levels and its sculpt layers, then the symmetry
//! and the view. So a scene moves between browsers and computers whole,
//! which OBJ and STL cannot do. The extension is `.golem`; a file is known
//! by its first bytes too, whatever it is called.
//!
//! Opening one replaces the scene, and the undo list goes with it, so it
//! asks first (`scene_question`).

use eframe::egui::{self, Align2, Color32, Id, Key, Modifiers, Order, RichText};

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::{GolemApp, Mode};

/// What a scene file is called when it is saved.
const FILE_NAME: &str = "golem-scene.golem";

/// A scene file that was picked or dropped, waiting for the question.
pub struct Pending {
    name: String,
    bytes: Vec<u8>,
}

/// Whether `name` / `bytes` is a scene file rather than a mesh.
pub fn is_scene(name: &str, bytes: &[u8]) -> bool {
    name.to_ascii_lowercase().ends_with(".golem") || bytes.starts_with(b"GOLEMW")
}

impl GolemApp {
    /// Save Scene: the whole scene as one file.
    pub(super) fn save_scene(&mut self) {
        if self.sculptor.is_active() {
            return;
        }
        self.layer_strength_done();
        let bytes = self.scene_bytes();
        crate::browser::download(FILE_NAME, &bytes, "application/octet-stream");
        let objects = if self.objects.len() == 1 { "1 object".to_owned() } else { format!("{} objects", self.objects.len()) };
        self.status = format!(
            "Saved {FILE_NAME} ({objects}, {:.1} MB), with levels, layers and masks. Open it here, in any browser, to carry on.",
            bytes.len() as f64 / (1 << 20) as f64
        );
    }

    /// A picked or dropped scene file: held until the question is answered.
    pub(super) fn scene_file_picked(&mut self, name: &str, bytes: &[u8]) {
        self.pending_scene = Some(Pending { name: name.to_owned(), bytes: bytes.to_vec() });
    }

    fn open_pending_scene(&mut self) {
        let Some(Pending { name, bytes }) = self.pending_scene.take() else { return };
        self.layer_strength_done();
        // A tool or the gizmo works on objects that are about to go.
        self.mode = Mode::Sculpt;
        match self.restore_scene(&bytes) {
            Some(()) => {
                let objects = if self.objects.len() == 1 { "1 object".to_owned() } else { format!("{} objects", self.objects.len()) };
                let lost = if self.status.contains("could not be read back") {
                    " Some subdivision levels or layers could not be read back, and are part of the shape now."
                } else {
                    ""
                };
                self.status = format!("Opened {name}: {objects}.{lost}");
                self.mark_changed();
            }
            None => self.status = format!("{name} is not a scene this version can read; nothing was changed."),
        }
    }

    /// Once a frame: the question for a scene file waiting to be opened.
    pub(super) fn scene_question(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending_scene else { return };
        let name = pending.name.clone();
        let mb = pending.bytes.len() as f64 / (1 << 20) as f64;
        let (mut open, mut cancel) = ctx.input_mut(|i| {
            (i.consume_key(Modifiers::NONE, Key::Enter), i.consume_key(Modifiers::NONE, Key::Escape))
        });
        let screen = ctx.content_rect();
        let shade = egui::Area::new(Id::new("scene_question_shade"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(110));
                response
            });
        cancel |= shade.inner.clicked();
        let width = (screen.width() - 32.0).clamp(240.0, 400.0);
        let count = self.objects.len();
        let card = egui::Area::new(Id::new("scene_question_card"))
            .order(Order::Foreground)
            .pivot(Align2::CENTER_CENTER)
            .fixed_pos(screen.center())
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL())
                    .stroke(egui::Stroke::new(1.0, theme::CONTROL()))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.label(RichText::new("Open this scene?").font(theme::font(18.0)).color(theme::TEXT()));
                        ui.add(egui::Label::new(RichText::new(format!("{name} ({mb:.1} MB)")).font(theme::font(12.0)).color(theme::HINT())).wrap());
                        let (now, them) = match count {
                            1 => ("the object you have now".to_owned(), "it"),
                            n => (format!("the {n} objects you have now"), "them"),
                        };
                        let text = format!(
                            "It takes the place of {now}, and Ctrl+Z cannot bring {them} back. To keep {them}, cancel \
                             and press Save Scene first."
                        );
                        ui.add(egui::Label::new(RichText::new(text).font(theme::font(12.0)).color(theme::TEXT())).wrap());
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let w = theme::row_width(ui, 2, 6.0);
                            if theme::button(ui, w, "Cancel", ButtonStyle::default()).clicked() {
                                cancel = true;
                            }
                            let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
                            if theme::button(ui, w, "Open Scene", style).clicked() {
                                open = true;
                            }
                        });
                    });
            });
        ctx.move_to_top(card.response.layer_id);
        if open {
            self.open_pending_scene();
        } else if cancel {
            self.pending_scene = None;
            self.status = format!("Kept the scene as it was; {name} was not opened.");
        }
    }
}
