//! The panels for how the model looks: its surface finish and glow, the
//! lighting rig and HDRI, the backdrop and the post effects.

use egui::{Color32, Sense, pos2, vec2};

use super::theme::{self, ButtonStyle, ACCENT};
use super::tip;
use crate::doc::{Action, Options};
use crate::look::{self, Finish, Look, Rgb, Tonemap};

pub(super) fn two(v: f32) -> String {
    format!("{v:.2}")
}

pub(super) fn percent(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

fn to_color(c: Rgb) -> Color32 {
    Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

/// A caption with a colour well that opens a picker.
pub(super) fn color_row(ui: &mut egui::Ui, label: &str, color: &mut Rgb) {
    ui.horizontal(|ui| {
        ui.color_edit_button_rgb(color);
        ui.label(egui::RichText::new(label).font(theme::font(12.0)).color(theme::TEXT()));
    });
}

/// A grid of preset buttons, each with a dot of the preset's colour. Returns
/// the one clicked.
pub(super) fn preset_grid(
    ui: &mut egui::Ui,
    tips: bool,
    selected: Option<usize>,
    presets: impl Iterator<Item = (&'static str, &'static str, Rgb)>,
) -> Option<usize> {
    let mut picked = None;
    let presets: Vec<_> = presets.collect();
    let w = theme::row_width(ui, 2, 6.0);
    for (row, pair) in presets.chunks(2).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (col, (name, description, color)) in pair.iter().enumerate() {
                let i = row * 2 + col;
                let style = ButtonStyle { active: (selected == Some(i)).then_some(ACCENT()), ..Default::default() };
                let response = theme::button(ui, w, name, style);
                let dot = pos2(response.rect.left() + 9.0, response.rect.center().y);
                ui.painter().circle_filled(dot, 4.5, to_color(*color));
                ui.painter().circle_stroke(dot, 4.5, egui::Stroke::new(1.0, Color32::from_black_alpha(90)));
                if tip(response, tips, description).clicked() {
                    picked = Some(i);
                }
            }
        });
        ui.add_space(4.0);
    }
    picked
}

/// A row of buttons of which one is on. Returns the one clicked.
pub(super) fn choice(ui: &mut egui::Ui, labels: &[&str], selected: usize) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = theme::row_width(ui, labels.len(), 6.0);
        for (i, label) in labels.iter().enumerate() {
            let style = ButtonStyle { active: (selected == i).then_some(ACCENT()), ..Default::default() };
            if theme::button(ui, w, label, style).clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

/// The Unity app's colour for a note that something set here is not showing.
const WARN: Color32 = Color32::from_rgb(242, 166, 102);

/// A note that something set here is not showing, with the button that puts
/// it right one click away (rather than the app changing another setting
/// behind the user's back). True when the button is clicked.
fn fix(ui: &mut egui::Ui, tips: bool, note: &str, button: &str, help: &str) -> bool {
    ui.add(egui::Label::new(egui::RichText::new(note).font(theme::font(11.0)).italics().color(WARN)).wrap());
    let clicked = tip(theme::button(ui, ui.available_width(), button, ButtonStyle::default()), tips, help).clicked();
    ui.add_space(4.0);
    clicked
}

/// Everything under the matcap grid in the Material card. `size` is the
/// model's size now, which a finish's pattern is measured against when picked.
pub fn material(ui: &mut egui::Ui, o: &mut Options, size: f32, actions: &mut Vec<Action>) {
    let tips = o.tooltips;
    if o.matcap_on {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = theme::row_width(ui, 2, 6.0);
            let import = theme::button(ui, w, "Import Matcap...", ButtonStyle::default());
            if tip(import, tips, "Add a matcap picture (.png or .jpg) of your own. It is copied into the matcap folder.").clicked() {
                actions.push(Action::ImportMatcap);
            }
            let rescan = theme::button(ui, w, "Rescan Folder", ButtonStyle::default());
            if tip(rescan, tips, "Look through the matcap folder again for pictures you have put there.").clicked() {
                actions.push(Action::RescanMatcaps);
            }
        });
        theme::slider(ui, "Matcap Intensity", &mut o.look.matcap_intensity, 0.0..=3.0, two);
        theme::slider(ui, "Tint by Base Colour", &mut o.look.matcap_tint, 0.0..=1.0, percent);
        if o.look.matcap_tint > 0.0 {
            color_row(ui, "Base Colour", &mut o.base_color);
        }
    }

    theme::heading(ui, "Surface Finish");
    // A matcap replaces the lighting outright, and every finish is lighting.
    if o.matcap_on
        && fix(
            ui,
            tips,
            "Matcap is on - it hides these finishes.",
            "Turn Off Matcap",
            "Switch the matcap off so the lights, and the surface finish, show.",
        )
    {
        o.matcap_on = false;
    }
    let finishes = Finish::ALL;
    let at = finishes.iter().position(|&f| f == o.look.finish).unwrap_or(0);
    for (row, chunk) in finishes.chunks(3).enumerate() {
        let names: Vec<&str> = chunk.iter().map(|f| f.name()).collect();
        let selected = if at / 3 == row { at % 3 } else { usize::MAX };
        if let Some(i) = choice(ui, &names, selected) {
            o.look.set_finish(chunk[i], size);
        }
        ui.add_space(4.0);
    }
    match o.look.finish {
        Finish::None => {
            theme::hint(ui, "Plain colour under the lighting rig.");
            // (Already just above, beside the tint that uses it.)
            if !(o.matcap_on && o.look.matcap_tint > 0.0) {
                color_row(ui, "Base Colour", &mut o.base_color);
            }
            theme::slider(ui, "Metallic", &mut o.look.metallic, 0.0..=1.0, two);
            theme::slider(ui, "Smoothness", &mut o.smoothness, 0.0..=1.0, two);
        }
        Finish::LurePlastic => lure(ui, &mut o.look, tips, size),
        Finish::Metal => metal(ui, &mut o.look, tips, size),
        Finish::Clay => clay(ui, &mut o.look, tips, size),
        Finish::Wood => wood(ui, &mut o.look, tips, size),
    }

    theme::heading(ui, "Surface Grain");
    theme::slider(ui, "Strength", &mut o.look.normal_strength, 0.0..=2.0, two);
    if o.look.normal_strength > 0.0 {
        theme::slider(ui, "Scale", &mut o.look.normal_scale, 1.0..=300.0, |v| format!("{v:.0}"));
    }

    theme::heading(ui, "Glow");
    let mut on = o.look.glow.on;
    let toggled = tip(
        theme::checkbox(ui, &mut on, "Enabled", ACCENT()),
        tips,
        "Light the surface gives off, over whatever shades it. Bloom turns it into a halo.",
    );
    if toggled.changed() {
        o.look.set_glow(on, size);
    }
    // The halo is Bloom's doing, and Bloom lives in another card.
    if o.look.glow.on
        && !o.look.bloom
        && fix(
            ui,
            tips,
            "Bloom is off - the glow has no halo.",
            "Turn On Bloom",
            "Switch on Bloom (under Backdrop & Effects), which spreads bright glowing areas into a soft halo.",
        )
    {
        o.look.bloom = true;
    }
    if o.look.glow.on {
        let presets = look::GLOWS.iter().map(|p| (p.name, p.description, p.cool));
        if let Some(i) = preset_grid(ui, tips, Some(o.look.glow.preset), presets) {
            o.look.pick_glow(i, size);
        }
        let g = &mut o.look.glow;
        theme::slider(ui, "Intensity", &mut g.intensity, 0.0..=3.0, two);
        theme::slider(ui, "Coverage", &mut g.coverage, 0.0..=2.0, two);
        theme::slider(ui, "Rim", &mut g.rim, 0.0..=2.0, two);
        theme::slider(ui, "Pattern Size", &mut g.pattern_size, 0.25..=3.0, two);
        theme::slider(ui, "Speed", &mut g.speed, 0.0..=3.0, two);
        theme::slider(ui, "Pulse", &mut g.pulse, 0.0..=2.0, two);
        theme::slider(ui, "Dim Surface", &mut g.dim, 0.0..=2.0, two);
        theme::slider(ui, "Hue Shift", &mut g.hue_shift, 0.0..=1.0, percent);
    }
}

pub(super) fn lure(ui: &mut egui::Ui, look: &mut Look, tips: bool, size: f32) {
    theme::hint(ui, "Translucent soft plastic with glitter cast into it.");
    let presets = look::LURES.iter().map(|p| (p.name, p.description, p.thin));
    if let Some(i) = preset_grid(ui, tips, Some(look.lure.preset), presets) {
        look.pick_preset(Finish::LurePlastic, i, size);
    }
    let s = &mut look.lure;
    theme::slider(ui, "Flake Size", &mut s.flake_size, 0.25..=3.0, two);
    theme::slider(ui, "Flake Amount", &mut s.flake_amount, 0.0..=2.0, two);
    theme::slider(ui, "Sparkle", &mut s.sparkle, 0.0..=3.0, two);
    theme::slider(ui, "Translucency", &mut s.translucency, 0.25..=3.0, two);
    theme::slider(ui, "Gloss", &mut s.gloss, 0.0..=1.0, two);
}

pub(super) fn metal(ui: &mut egui::Ui, look: &mut Look, tips: bool, size: f32) {
    theme::hint(ui, "Aged metal: a coat in the recesses, worn bright on the edges.");
    let presets = look::METALS.iter().map(|p| (p.name, p.description, p.coat_a));
    if let Some(i) = preset_grid(ui, tips, Some(look.metal.preset), presets) {
        look.pick_preset(Finish::Metal, i, size);
    }
    let s = &mut look.metal;
    theme::slider(ui, "Exposed Metal", &mut s.exposure, 0.0..=2.0, two);
    theme::slider(ui, "Edge Wear", &mut s.edge_wear, 0.0..=2.0, two);
    theme::slider(ui, "Wash", &mut s.wash, 0.0..=1.5, two);
    theme::slider(ui, "Detail", &mut s.detail, 0.25..=3.0, two);
    theme::slider(ui, "Pattern Size", &mut s.pattern_size, 0.25..=3.0, two);
    theme::slider(ui, "Gloss", &mut s.gloss, 0.0..=2.0, two);
    theme::slider(ui, "Pattern", &mut s.seed, 0.0..=1.0, percent);
    let w = ui.available_width();
    if tip(theme::button(ui, w, "Shuffle Pattern", ButtonStyle::default()), tips, "Move the blotches somewhere random.")
        .clicked()
    {
        s.seed = random(ui);
    }
}

pub(super) fn clay(ui: &mut egui::Ui, look: &mut Look, tips: bool, size: f32) {
    theme::hint(ui, "Sculptor's oil clay: satin, a wet highlight, richer in the recesses.");
    let presets = look::CLAYS.iter().map(|p| (p.name, p.description, p.color));
    if let Some(i) = preset_grid(ui, tips, Some(look.clay.preset), presets) {
        look.pick_preset(Finish::Clay, i, size);
    }
    let s = &mut look.clay;
    theme::slider(ui, "Gloss", &mut s.gloss, 0.0..=2.0, two);
    theme::slider(ui, "Wetness", &mut s.wetness, 0.0..=2.0, two);
    theme::slider(ui, "Subsurface", &mut s.subsurface, 0.0..=2.0, two);
    theme::slider(ui, "Recess", &mut s.recess, 0.0..=2.0, two);
    theme::slider(ui, "Detail", &mut s.detail, 0.25..=3.0, two);
    theme::slider(ui, "Grain", &mut s.grain, 0.0..=3.0, two);
}

pub(super) fn wood(ui: &mut egui::Ui, look: &mut Look, tips: bool, size: f32) {
    theme::hint(ui, "Carved, waxed wood: rings around a log through the model.");
    let presets = look::WOODS.iter().map(|p| (p.name, p.description, p.early));
    if let Some(i) = preset_grid(ui, tips, Some(look.wood.preset), presets) {
        look.pick_preset(Finish::Wood, i, size);
    }
    let s = &mut look.wood;
    theme::heading(ui, "Grain Runs");
    if let Some(i) = choice(ui, &["Vertical", "Left-Right", "Front-Back"], s.axis) {
        s.axis = i;
    }
    theme::slider(ui, "Grain", &mut s.grain, 0.0..=2.0, two);
    theme::slider(ui, "Grain Scale", &mut s.grain_scale, 0.25..=3.0, two);
    theme::slider(ui, "Edge Wear", &mut s.edge_wear, 0.0..=2.0, two);
    theme::slider(ui, "Patina", &mut s.patina, 0.0..=2.0, two);
    theme::slider(ui, "Detail", &mut s.detail, 0.25..=3.0, two);
    theme::slider(ui, "Sheen", &mut s.sheen, 0.0..=2.0, two);
    theme::slider(ui, "Cracks", &mut s.cracks, 0.0..=2.0, two);
    theme::slider(ui, "Heart of the Log", &mut s.seed, 0.0..=1.0, percent);
    let w = ui.available_width();
    if tip(theme::button(ui, w, "Shuffle Grain", ButtonStyle::default()), tips, "Run the log's heart somewhere random.")
        .clicked()
    {
        s.seed = random(ui);
    }
}

/// A number in 0..1 that differs from click to click.
fn random(ui: &egui::Ui) -> f32 {
    let t = ui.input(|i| i.time);
    (t * 1000.0).sin().abs().fract() as f32
}

/// The Lighting card: the preset rig and the HDRI environment.
pub fn lighting(ui: &mut egui::Ui, o: &mut Options, actions: &mut Vec<Action>) {
    let tips = o.tooltips;
    let preset = o.look.preset();
    theme::category(ui, "Lighting", Color32::from_rgb(255, 196, 110), preset.name, false, |ui| {
        if o.matcap_on {
            theme::hint(ui, "A matcap carries its own lighting. Turn Matcap off to see these lights.");
        }
        let look = &mut o.look;
        let presets = look::LIGHTING.iter().map(|p| (p.name, p.description, p.lights[0].color));
        if let Some(i) = preset_grid(ui, tips, Some(look.lighting), presets) {
            look.lighting = i;
        }
        if let Some(i) = choice(ui, &["3-Point", "5-Point"], look.five_point as usize) {
            look.five_point = i == 1;
        }
        theme::slider(ui, "Brightness", &mut look.brightness, 0.0..=look::MAX_BRIGHTNESS, two);
        theme::slider(ui, "Rotation", &mut look.rotation, -180.0..=180.0, |v| format!("{v:.0}\u{b0}"));
        tip(
            theme::checkbox(ui, &mut look.follow_camera, "Follow Camera", ACCENT()),
            tips,
            "Keep the key light at the upper left of the view. Off freezes the lights where they are.",
        );
        tip(theme::checkbox(ui, &mut look.shadows, "Shadows", ACCENT()), tips, "The key light casts a soft shadow.");

        theme::heading(ui, "HDRI Environment");
        let hdri = &mut look.hdri;
        let color = if hdri.failed { theme::ERROR } else { theme::HINT() };
        ui.add(egui::Label::new(egui::RichText::new(&hdri.status).font(theme::font(11.0)).color(color)).wrap());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = theme::row_width(ui, 2, 6.0);
            let load = theme::button(ui, w, "Load...", ButtonStyle::default());
            if tip(load, tips, "Light the scene from a 360 degree picture (.hdr, .exr, .png, .jpg).").clicked() {
                actions.push(Action::LoadHdri);
            }
            ui.add_enabled_ui(hdri.loaded, |ui| {
                if theme::button(ui, w, "Clear", ButtonStyle::default()).clicked() {
                    actions.push(Action::ClearHdri);
                }
            });
        });
        if hdri.loaded {
            theme::checkbox(ui, &mut hdri.enabled, "Light with the HDRI", ACCENT());
            theme::slider(ui, "HDRI Rotation", &mut hdri.rotation, 0.0..=360.0, |v| format!("{v:.0}\u{b0}"));
            theme::slider(ui, "Exposure", &mut hdri.exposure, 0.0..=4.0, two);
            theme::slider(ui, "Ambient", &mut hdri.ambient, 0.0..=3.0, two);
            theme::slider(ui, "Reflection", &mut hdri.reflection, 0.0..=1.0, two);
        }
    });
}

/// The backdrop and the post effects.
pub fn effects(ui: &mut egui::Ui, o: &mut Options) {
    let tips = o.tooltips;
    theme::category(ui, "Backdrop & Effects", Color32::from_rgb(150, 205, 240), "", false, |ui| {
        theme::heading(ui, "Background");
        let hdri = o.look.hdri.active();
        let selected = if o.look.hdri_background && hdri {
            2
        } else if o.gradient_background {
            1
        } else {
            0
        };
        if let Some(i) = choice(ui, &["Flat", "Gradient", "HDRI"], selected) {
            o.look.hdri_background = i == 2;
            if i < 2 {
                o.gradient_background = i == 1;
            }
        }
        if o.look.hdri_background && !hdri {
            theme::hint(ui, "Load an HDRI under Lighting to show it here.");
        }
        if selected < 2 {
            color_row(ui, if selected == 1 { "Bottom" } else { "Colour" }, &mut o.background);
        }
        if selected == 1 {
            color_row(ui, "Top", &mut o.background_top);
            theme::slider(ui, "Gradient Bias", &mut o.look.gradient_bias, 0.2..=4.0, two);
        }

        let look = &mut o.look;
        theme::heading(ui, "Bloom");
        theme::checkbox(ui, &mut look.bloom, "Enabled", ACCENT());
        if look.bloom {
            theme::slider(ui, "Intensity", &mut look.bloom_intensity, 0.0..=3.0, two);
            theme::slider(ui, "Threshold", &mut look.bloom_threshold, 0.0..=2.0, two);
        }
        theme::heading(ui, "Vignette");
        theme::checkbox(ui, &mut look.vignette, "Enabled", ACCENT());
        if look.vignette {
            theme::slider(ui, "Intensity", &mut look.vignette_intensity, 0.0..=1.0, two);
            theme::slider(ui, "Smoothness", &mut look.vignette_smoothness, 0.01..=1.0, two);
        }
        theme::heading(ui, "Depth of Field");
        tip(
            theme::checkbox(ui, &mut look.depth_of_field, "Enabled", ACCENT()),
            tips,
            "Blurs what is nearer or farther than the focus distance, like a camera lens.",
        );
        if look.depth_of_field {
            // At the settings it starts with there is next to nothing to see.
            theme::hint(ui, "A close-up effect: bring Focus Distance in to the model, then lower Aperture.");
            tip(
                theme::slider(ui, "Focus Distance", &mut look.focus_distance, 0.1..=10.0, two),
                tips,
                "Distance from the camera that stays sharp.",
            );
            tip(
                theme::slider(ui, "Aperture (lower = blurrier)", &mut look.aperture, 1.0..=32.0, |v| format!("f/{v:.1}")),
                tips,
                "The lens opening: lower values blur what is out of focus more.",
            );
        }
        theme::heading(ui, "Colour");
        theme::checkbox(ui, &mut look.grading, "Adjust", ACCENT());
        if look.grading {
            theme::slider(ui, "Saturation", &mut look.saturation, -100.0..=100.0, |v| format!("{v:.0}"));
            theme::slider(ui, "Contrast", &mut look.contrast, -100.0..=100.0, |v| format!("{v:.0}"));
        }
        theme::heading(ui, "Tone Curve");
        let at = match look.tonemap {
            Tonemap::None => 0,
            Tonemap::Neutral => 1,
            Tonemap::Aces => 2,
        };
        if let Some(i) = choice(ui, &["None", "Neutral", "ACES"], at) {
            look.tonemap = [Tonemap::None, Tonemap::Neutral, Tonemap::Aces][i];
        }
        ui.add_space(4.0);
        tip(
            theme::checkbox(ui, &mut look.ambient_occlusion, "Ambient Occlusion", ACCENT()),
            tips,
            "Soft contact shadows in creases and where forms meet (lit shading only).",
        );
        // The space the last row's hover needs before the card's edge.
        let _ = ui.allocate_exact_size(vec2(1.0, 2.0), Sense::hover());
    });
}
