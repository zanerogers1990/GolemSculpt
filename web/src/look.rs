//! How the model is shown: what goes to the renderer each frame (the
//! globals, the runs of a big mesh in view, a picture to save), and the
//! Look tab's cards for the backdrop, the lens, the material finishes, the
//! display switches and Save Picture.
//!
//! A child of app.rs, so it reaches the app's own fields; the rest of the
//! interface is in ui.rs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32, Ui};
use glam::Vec2;

use super::GolemApp;
use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use crate::render::finish::{self, Finish, Finishes};
use crate::render::still::{self, Request};
use crate::render::{Culling, Globals, Viewport};

/// The wireframe's opacity (the desktop's).
const WIRE_OPACITY: f32 = 0.6;

/// What the Look tab sets and the renderer is told each frame.
pub struct Look {
    /// The wireframe over the model.
    pub wire: bool,
    /// A line where each mirror plane cuts the model, while symmetry is on.
    pub mirror_guides: bool,
    /// The backdrop: the theme's colour flat, or a gradient from it up to
    /// the theme's top colour, and the gradient's bias.
    pub gradient: bool,
    pub gradient_bias: f32,
    /// The material finish and every finish's settings.
    pub finishes: Finishes,
    /// Drawing only what is in view, for a big mesh. `cull` is on unless
    /// the browser's settings say `golemsculpt.cull` = `off` (for measuring
    /// what it saves).
    culling: Culling,
    cull: bool,
    /// Set by the renderer when it needs the mesh sent whole.
    wants_whole: Arc<AtomicBool>,
    /// Save Picture: its size (times the view's), a clear background, and
    /// the picture on its way.
    pub still_scale: u32,
    pub still_transparent: bool,
    still: Option<Arc<Request>>,
    /// The lens kept from the last visit is put on the camera on the first frame.
    lens_restored: bool,
}

impl Look {
    pub fn new() -> Look {
        let pref = crate::browser::pref;
        Look {
            wire: pref("wire") == "on",
            mirror_guides: pref("guides") != "off",
            gradient: pref("backdrop") == "gradient",
            gradient_bias: pref("gradient_bias").parse().unwrap_or(1.0f32).clamp(0.2, 4.0),
            finishes: Finishes::from_pref(&pref("finish")),
            culling: Culling::default(),
            cull: pref("cull") != "off",
            wants_whole: Arc::default(),
            still_scale: pref("still_scale").parse().unwrap_or(2u32).clamp(1, 4),
            still_transparent: pref("still_clear") == "on",
            still: None,
            lens_restored: false,
        }
    }
}

impl Default for Look {
    fn default() -> Self {
        Look::new()
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

impl GolemApp {
    /// Whether the mesh's curvature is needed on the card: for cavity, or
    /// for a finish's recess wash and edge wear.
    pub(super) fn wants_curvature(&self) -> bool {
        self.cavity || self.look.finishes.finish.needs_curvature()
    }

    /// Everything the renderer needs for this frame of the viewport, `size`
    /// physical pixels big. Also looks after a picture being saved.
    pub(super) fn viewport(&mut self, ctx: &egui::Context, size: Vec2) -> Viewport {
        if !self.look.lens_restored {
            self.look.lens_restored = true;
            if let Ok(fov) = crate::browser::pref("fov").parse::<f32>() {
                self.camera.set_lens(fov);
            }
        }
        // The renderer asked for the whole mesh (the wireframe came on).
        if self.look.wants_whole.swap(false, Ordering::Relaxed) {
            self.upload_whole = true;
            ctx.request_repaint();
        }
        let palette = sculpt_toolkit::theme::palette();
        let [r, g, b] = palette.backdrop;
        let top = if self.look.gradient { palette.backdrop_top } else { palette.backdrop };
        let [eye, forward, right, up] = Globals::camera(&self.view);
        let s = self.sculptor.settings.symmetry;
        let guides = if self.look.mirror_guides { [s[0] as u8 as f32, s[1] as u8 as f32, s[2] as u8 as f32, 0.0] } else { [0.0; 4] };
        let globals = Globals {
            view_proj: self.view.view_proj.to_cols_array_2d(),
            view: self.view.view.to_cols_array_2d(),
            mask_tint: [0.25, 0.25, 0.28, if self.mask_tint { 0.85 } else { 0.0 }],
            params: [self.flat_shading as u8 as f32, 0.0, self.show_backfaces as u8 as f32, self.cavity as u8 as f32],
            // The backdrop that goes with the theme, flat or a gradient.
            // Their fourth numbers carry cavity's ridge and valley.
            bg_top: [top[0], top[1], top[2], self.cavity_ridge],
            bg_bottom: [r, g, b, self.cavity_valley],
            eye,
            forward,
            right,
            up,
            look: [self.look.gradient as u8 as f32, self.look.gradient_bias, 0.0, WIRE_OPACITY],
            guides,
        };
        let upload = self.upload();
        let culled = if self.look.cull {
            // In the active object's own space, where its mesh is.
            let local = self.local_view();
            self.look.culling.update(&self.mesh, upload.as_deref(), &local.view_proj, self.generation)
        } else {
            None
        };
        let model_size = self.mesh.bounds().size().max_element();
        let finish = self.look.finishes.uniform(self.view.right, self.view.up, self.view.forward, model_size);

        // A picture asked for: once it is back, made into a PNG and handed over.
        let still = self.look.still.clone();
        if let Some(request) = &still {
            match request.take() {
                Some(Ok(picture)) => {
                    self.look.still = None;
                    match still::png(&picture) {
                        Ok(bytes) => {
                            crate::browser::download("golem-sculpt.png", &bytes, "image/png");
                            self.status = format!("Saved golem-sculpt.png, {} x {} pixels.", picture.width(), picture.height());
                        }
                        Err(e) => self.status = e,
                    }
                }
                Some(Err(e)) => {
                    self.look.still = None;
                    self.status = e;
                }
                None => ctx.request_repaint(),
            }
        }

        Viewport {
            globals,
            upload,
            size: [size.x.round().max(1.0) as u32, size.y.round().max(1.0) as u32],
            has: self.card_has.clone(),
            upload_seq: self.upload_seq,
            took: self.card_took.clone(),
            previews: self.tubes.draw.previews().to_vec(),
            matcap: self.matcap,
            matcaps: self.matcaps.clone(),
            wire: self.look.wire,
            wants_whole: self.look.wants_whole.clone(),
            culled,
            still: self.look.still.clone(),
            finish,
            model: self.model(),
            show_model: self.active_object().visible,
            others: self
                .objects
                .iter()
                .filter(|o| o.visible && o.id != self.active)
                .filter_map(|o| o.drawn.clone().map(|drawn| crate::render::Other { drawn, model: o.matrix() }))
                .collect(),
        }
    }

    /// The share of a big model drawn this frame (1 when all of it is).
    pub(super) fn share_in_view(&self) -> f32 {
        self.look.culling.share_in_view()
    }

    /// Turns the wireframe on or off, and keeps the choice.
    pub(super) fn set_wire(&mut self, on: bool) {
        if on && !self.look.wire {
            // Its lines are made from the mesh as it is sent whole.
            self.upload_whole = true;
        }
        self.look.wire = on;
        crate::browser::set_pref("wire", on_off(on));
    }

    /// Puts the lens of `mm` on the camera, and keeps it.
    pub(super) fn set_lens_mm(&mut self, mm: f32) {
        let fov = crate::camera::fov_of_lens(mm);
        self.camera.set_lens(fov);
        crate::browser::set_pref("fov", &self.camera.fov_y.to_string());
    }

    /// Asks for a picture of the view as it is now.
    fn save_picture(&mut self) {
        let size = self.view.size * self.look.still_scale as f32;
        let size = [size.x.round().max(1.0) as u32, size.y.round().max(1.0) as u32];
        self.look.still = Some(Arc::new(Request::new(size, self.look.still_transparent)));
        self.status = "Drawing the picture...".into();
    }

    // ---- the Look tab --------------------------------------------------------------

    /// The Look tab's cards after the matcap: the finish, the backdrop, the
    /// lens, the display switches and Save Picture.
    pub(super) fn look_cards(&mut self, ui: &mut Ui) {
        self.finish_card(ui);
        self.backdrop_card(ui);
        self.lens_card(ui);
        self.display_card(ui);
        self.picture_card(ui);
    }

    fn finish_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let before = self.look.finishes.clone();
        let model_size = self.mesh.bounds().size().max_element();
        let f = &mut self.look.finishes;
        let names = finish::preset_names(f.finish);
        let summary = match f.finish {
            Finish::Matcap => "Matcap".to_owned(),
            Finish::Clay => names[f.clay.preset.min(names.len() - 1)].to_owned(),
            Finish::Metal => names[f.metal.preset.min(names.len() - 1)].to_owned(),
            Finish::Wood => names[f.wood.preset.min(names.len() - 1)].to_owned(),
        };
        let mut froze = false;
        theme::category(ui, "Finish", Color32::from_rgb(230, 170, 90), &summary, true, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let w = theme::row_width(ui, Finish::ALL.len(), 4.0);
                for kind in Finish::ALL {
                    let style = ButtonStyle { active: (f.finish == kind).then_some(ACCENT()), ..Default::default() };
                    let help = match kind {
                        Finish::Matcap => "Shade the model with the matcap above: quick and even, for judging form.",
                        Finish::Clay => "Sculptor's oil clay: a satin sheen, a wet highlight, richer colour in the recesses.",
                        Finish::Metal => "Aged metal: bare metal on the raised edges, a coat in the hollows, a dark wash in the deepest.",
                        Finish::Wood => "Carved wood: growth rings round a log, grime in the recesses, edges rubbed back, a wax shine.",
                    };
                    if tip(theme::button(ui, w, kind.name(), style), tips, help).clicked() && f.finish != kind {
                        f.finish = kind;
                        froze = f.reference() <= 0.0;
                    }
                }
            });
            if f.finish == Finish::Matcap {
                theme::hint(ui, "A finish lights the model with three lights that turn with the view.");
                return;
            }
            // The preset, a button each, two to a row.
            let preset = match f.finish {
                Finish::Clay => &mut f.clay.preset,
                Finish::Metal => &mut f.metal.preset,
                _ => &mut f.wood.preset,
            };
            for (row, pair) in names.chunks(2).enumerate() {
                ui.horizontal(|ui| {
                    let w = theme::row_width(ui, 2, 6.0);
                    for (column, name) in pair.iter().enumerate() {
                        let i = row * 2 + column;
                        let style = ButtonStyle { active: (*preset == i).then_some(ACCENT()), ..Default::default() };
                        let about = finish::preset_about(f.finish, i);
                        if tip(theme::button(ui, w, name, style), tips, about).clicked() {
                            *preset = i;
                            froze = true;
                        }
                    }
                });
            }
            let two = |v: f32| format!("{v:.2}");
            match f.finish {
                Finish::Clay => {
                    let c = &mut f.clay;
                    tip(theme::slider(ui, "Gloss", &mut c.gloss, 0.0..=2.0, two), tips, "How shiny the clay's satin is; takes the wet film with it.");
                    tip(theme::slider(ui, "Wetness", &mut c.wetness, 0.0..=2.0, two), tips, "The thin wet highlight riding on the satin.");
                    tip(theme::slider(ui, "Subsurface", &mut c.subsurface, 0.0..=2.0, two), tips, "Light bleeding a little past the shadow line.");
                    tip(theme::slider(ui, "Recess Darkening", &mut c.recess, 0.0..=2.0, two), tips, "How much darker and richer the recesses are.");
                    tip(theme::slider(ui, "Grain", &mut c.grain, 0.0..=2.0, two), tips, "Fine grit in the clay body.");
                    tip(theme::slider(ui, "Detail", &mut c.detail, 0.25..=4.0, two), tips, "How much relief counts as a recess or a ridge.");
                }
                Finish::Metal => {
                    let m = &mut f.metal;
                    tip(theme::slider(ui, "Exposed Metal", &mut m.exposure, 0.0..=2.0, two), tips, "Less coat, more bare metal showing through.");
                    tip(theme::slider(ui, "Edge Wear", &mut m.edge_wear, 0.0..=2.0, two), tips, "Raised edges worn back to bright metal.");
                    tip(theme::slider(ui, "Wash", &mut m.wash, 0.0..=2.0, two), tips, "The dark wash pooled in the deepest recesses.");
                    tip(theme::slider(ui, "Gloss", &mut m.gloss, 0.0..=2.0, two), tips, "How polished the metal and the coat are.");
                    tip(theme::slider(ui, "Pattern Size", &mut m.pattern_size, 0.25..=4.0, two), tips, "The size of the coat's patches and the grain.");
                    tip(theme::slider(ui, "Detail", &mut m.detail, 0.25..=4.0, two), tips, "How much relief counts as a recess or a ridge.");
                    tip(theme::slider(ui, "Variation", &mut m.seed, 0.0..=1.0, two), tips, "Moves every patch to somewhere else.");
                }
                _ => {
                    let w = &mut f.wood;
                    ui.horizontal(|ui| {
                        let width = theme::row_width(ui, 3, 4.0);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, name) in ["Upright", "Across", "Front-Back"].into_iter().enumerate() {
                            let style = ButtonStyle { active: (w.axis == i).then_some(ACCENT()), ..Default::default() };
                            if tip(theme::button(ui, width, name, style), tips, "The way the log the model was carved from ran.").clicked() {
                                w.axis = i;
                            }
                        }
                    });
                    tip(theme::slider(ui, "Grain", &mut w.grain, 0.0..=2.0, two), tips, "How strongly the rings and fibres show.");
                    tip(theme::slider(ui, "Grain Scale", &mut w.grain_scale, 0.25..=4.0, two), tips, "The size of the rings and the figure.");
                    tip(theme::slider(ui, "Edge Wear", &mut w.edge_wear, 0.0..=2.0, two), tips, "High points rubbed back lighter.");
                    tip(theme::slider(ui, "Patina", &mut w.patina, 0.0..=2.0, two), tips, "Grime settled dark into the recesses.");
                    tip(theme::slider(ui, "Sheen", &mut w.sheen, 0.0..=2.0, two), tips, "The dull wax shine.");
                    tip(theme::slider(ui, "Cracks", &mut w.cracks, 0.0..=2.0, two), tips, "Seasoning cracks running along the grain.");
                    tip(theme::slider(ui, "Detail", &mut w.detail, 0.25..=4.0, two), tips, "How much relief counts as a recess or a ridge.");
                    tip(theme::slider(ui, "Variation", &mut w.seed, 0.0..=1.0, two), tips, "Moves the heart of the log, and the rings with it.");
                }
            }
            tip(theme::slider(ui, "Brightness", &mut f.brightness, 0.2..=2.5, two), tips, "How bright the lights are.");
            theme::hint(ui, "Three lights turn with the view. The desktop adds shadows, an HDRI and softboxes.");
        });
        if froze {
            // Patterns are sized to the model as it is now, and stay that
            // size while it is sculpted.
            self.look.finishes.freeze(model_size);
        }
        if self.look.finishes != before {
            crate::browser::set_pref("finish", &self.look.finishes.to_pref());
        }
    }

    fn backdrop_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let summary = if self.look.gradient { "Gradient" } else { "Flat" };
        theme::category(ui, "Backdrop", Color32::from_rgb(150, 205, 240), summary, false, |ui| {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                for (gradient, name, help) in [
                    (false, "Flat", "The theme's backdrop colour, even all over."),
                    (true, "Gradient", "From the theme's backdrop colour below to its lighter colour above, staying put as the view turns."),
                ] {
                    let style = ButtonStyle { active: (self.look.gradient == gradient).then_some(ACCENT()), ..Default::default() };
                    if tip(theme::button(ui, w, name, style), tips, help).clicked() {
                        self.look.gradient = gradient;
                        crate::browser::set_pref("backdrop", if gradient { "gradient" } else { "flat" });
                    }
                }
            });
            if self.look.gradient {
                let r = theme::slider(ui, "Gradient Bias", &mut self.look.gradient_bias, 0.2..=4.0, |v| format!("{v:.2}"));
                if tip(r, tips, "Moves where the gradient turns from one colour to the other.").changed() {
                    crate::browser::set_pref("gradient_bias", &self.look.gradient_bias.to_string());
                }
            }
            theme::hint(ui, "The colours go with the theme (Settings tab).");
        });
    }

    fn lens_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let mm = crate::camera::lens_mm(self.camera.fov_y);
        let summary = if self.camera.ortho { "Ortho".to_owned() } else { format!("{mm:.0} mm") };
        let mut wanted = None;
        theme::category(ui, "Lens", Color32::from_rgb(120, 160, 240), &summary, false, |ui| {
            ui.horizontal(|ui| {
                let lenses = crate::camera::LENSES;
                let w = theme::row_width(ui, lenses.len(), 4.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for lens in lenses {
                    let style = ButtonStyle { active: ((lens - mm).abs() < 0.5).then_some(ACCENT()), ..Default::default() };
                    if theme::button(ui, w, &format!("{lens:.0}"), style).clicked() {
                        wanted = Some(lens);
                    }
                }
            });
            let range = crate::camera::LENS_RANGE;
            let mut log = mm.clamp(*range.start(), *range.end()).ln();
            let r = theme::slider(ui, "Lens (mm)", &mut log, range.start().ln()..=range.end().ln(), |v| format!("{:.0} mm", v.exp()));
            let help = "The lens the view is seen through. Wide (18-24 mm) bends the model toward you, as a camera held close \
                        does; 50 mm is about what the eye sees; 85 mm and up flatten it, as portraits are taken. The model \
                        stays the size it is on screen.";
            if tip(r, tips, help).changed() {
                wanted = Some(log.exp());
            }
            let label = if self.camera.ortho { "Orthographic" } else { "Orthographic (Off)" };
            tip(theme::checkbox(ui, &mut self.camera.ortho, label, ACCENT()), tips, "No perspective at all: parallel lines stay parallel.");
        });
        if let Some(mm) = wanted {
            self.set_lens_mm(mm);
        }
    }

    fn display_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let mut wire = self.look.wire;
        theme::category(ui, "Display", Color32::from_rgb(168, 133, 255), "", true, |ui| {
            let was = self.cavity;
            tip(
                theme::checkbox(ui, &mut self.cavity, "Cavity", ACCENT()),
                tips,
                "Darken valleys and brighten ridges, so sculpted detail reads at a glance.",
            );
            if self.cavity != was {
                crate::browser::set_pref("cavity", on_off(self.cavity));
            }
            if self.cavity {
                theme::slider(ui, "Ridge", &mut self.cavity_ridge, 0.0..=2.0, |v| format!("{v:.2}"));
                theme::slider(ui, "Valley", &mut self.cavity_valley, 0.0..=2.0, |v| format!("{v:.2}"));
            }
            tip(theme::checkbox(ui, &mut wire, "Wireframe", ACCENT()), tips, "Draw the edges of the mesh's triangles over it.");
            let was = self.look.mirror_guides;
            tip(
                theme::checkbox(ui, &mut self.look.mirror_guides, "Mirror Guides", ACCENT()),
                tips,
                "While symmetry is on, a thin coloured line where each mirror plane cuts the model.",
            );
            if self.look.mirror_guides != was {
                crate::browser::set_pref("guides", on_off(self.look.mirror_guides));
            }
            theme::checkbox(ui, &mut self.flat_shading, "Flat Shading", ACCENT());
            theme::checkbox(ui, &mut self.show_backfaces, "Show Backfaces", ACCENT());
            theme::checkbox(ui, &mut self.mask_tint, "Show Mask", ACCENT());
            let share = self.share_in_view();
            if share < 0.995 {
                // A big model is drawn only where it is in view (render.rs, Culling).
                theme::hint(ui, &format!("Drawing {:.0}% of the model: the rest is out of view.", share * 100.0));
            }
        });
        if wire != self.look.wire {
            self.set_wire(wire);
        }
    }

    fn picture_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let busy = self.look.still.is_some();
        let mut save = false;
        theme::category(ui, "Save Picture", Color32::from_rgb(110, 200, 120), "", false, |ui| {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 3, 4.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for scale in [1u32, 2, 4] {
                    let style = ButtonStyle { active: (self.look.still_scale == scale).then_some(ACCENT()), ..Default::default() };
                    let help = format!("The picture {scale} times the size of the view on screen.");
                    if tip(theme::button(ui, w, &format!("{scale}x"), style), tips, &help).clicked() {
                        self.look.still_scale = scale;
                        crate::browser::set_pref("still_scale", &scale.to_string());
                    }
                }
            });
            let size = self.view.size * self.look.still_scale as f32;
            let [w, h] = still::fit([size.x as u32, size.y as u32], still::MAX_SIDE);
            theme::hint(ui, &format!("{w} x {h} pixels (smaller where the browser's graphics cannot draw that big)."));
            let was = self.look.still_transparent;
            tip(
                theme::checkbox(ui, &mut self.look.still_transparent, "Transparent Background", ACCENT()),
                tips,
                "Leave the backdrop out: the model on a clear background, to put on another picture.",
            );
            if self.look.still_transparent != was {
                crate::browser::set_pref("still_clear", on_off(self.look.still_transparent));
            }
            let full = ui.available_width();
            ui.add_enabled_ui(!busy, |ui| {
                let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
                if tip(theme::button(ui, full, "Save Picture", style), tips, "Download the view as a PNG picture.").clicked() {
                    save = true;
                }
            });
        });
        if save {
            self.save_picture();
        }
    }
}
