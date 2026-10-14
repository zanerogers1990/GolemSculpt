//! The Materials card on the Scene panel's Look tab: the material library
//! (each material a row with a little picture of it on a ball), what can be
//! done with the material picked for painting, the scene's palette, and the
//! two switches for what painted surfaces show.
//!
//! It mirrors the Brush Library card (`brushes.rs`): picking a row picks the
//! material, the user's own materials can be renamed and changed in place,
//! and everything that touches files or the scene goes to the app as an
//! [`Ask`]. The thumbnails are the app's to draw (app/materials.rs); the
//! card says which rows it showed without one, and shows a dot of the
//! material's swatch colour until the picture arrives.

use std::collections::HashMap;

use egui::{Align2, Color32, Rect, Sense, TextureHandle, Ui, pos2, vec2};

use super::look;
use super::theme::{self, ACCENT, ButtonStyle};
use crate::doc::Action;
use crate::materials::library::{Entry, Origin};
use crate::materials::palette::material_of;
use crate::materials::{Ask, Kind, Material, PaintMaterial, PaintPick, Settings, State, TintMode, Value};
use crate::mesh::paint::palette::ScenePaint;

const CARD: Color32 = Color32::from_rgb(214, 160, 230);
const COLUMNS: usize = 2;
const GAP: f32 = 4.0;
const ROW: f32 = 26.0;
const PICTURE: f32 = 22.0;

/// The thumbnails the card shows, and the ones it showed a row without.
#[derive(Default)]
pub struct Thumbs {
    /// By the material's thumbnail key (`materials::thumbs::key`).
    pub textures: HashMap<u64, TextureHandle>,
    /// Library places of rows drawn this frame with no picture yet, for the
    /// app to draw (it clears this once it has looked).
    pub wanted: Vec<usize>,
    /// Each library material's thumbnail key, worked out by the app when the
    /// library or the shaders change, and the library's generation and the
    /// shaders' it was worked out at.
    pub keys: Vec<u64>,
    pub keys_seen: (u64, u64),
    /// What the thumbnails are drawn into, made the first time one is.
    pub rig: Option<Rig>,
}

/// The renderer's working textures for the thumbnails' size, and the
/// picture they are drawn into (see app/materials.rs). Kept apart from the
/// window's, as a recording's are, so drawing one does not rebuild the other.
pub struct Rig {
    pub side: crate::render::Side,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl Rig {
    pub fn new(gpu: &crate::render::Gpu, side: u32) -> Rig {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("material thumbnail"),
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Rig { side: crate::render::Side::new(gpu), texture, view }
    }
}

fn to_color(c: [f32; 3]) -> Color32 {
    Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

/// One material of the list: its picture (or swatch dot), its name, and a
/// red corner if it cannot be drawn.
fn row(
    ui: &mut Ui,
    width: f32,
    picture: Option<&TextureHandle>,
    (m, origin): (&Material, Option<Origin>),
    lit: bool,
    broken: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    p.rect_filled(rect, 5.0, if lit { ACCENT() } else { theme::CONTROL() });
    if response.hovered() {
        p.rect_filled(rect, 5.0, Color32::from_white_alpha(18));
    }
    let at = Rect::from_center_size(pos2(rect.left() + 3.0 + PICTURE / 2.0, rect.center().y), vec2(PICTURE, PICTURE));
    match picture {
        Some(t) => {
            p.image(t.id(), at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.circle_filled(at.center(), 6.0, to_color(m.swatch));
            p.circle_stroke(at.center(), 6.0, egui::Stroke::new(1.0, Color32::from_black_alpha(90)));
        }
    }
    if broken {
        p.circle_filled(pos2(rect.right() - 5.0, rect.top() + 5.0), 3.0, theme::ERROR);
    }
    let ink = if lit { Color32::WHITE } else { theme::TEXT() };
    let left = at.right() + 5.0;
    let room = rect.right() - 6.0 - left;
    // Under its finish's heading a built-in material needs no "Metal - ".
    let name = match origin {
        Some(Origin::BuiltIn) => m.name.split_once(" - ").map_or(m.name.as_str(), |(_, preset)| preset),
        _ => m.name.as_str(),
    };
    let fits = |text: &str, size: f32| p.layout_no_wrap(text.to_owned(), theme::font(size), ink).size().x <= room;
    let size = [12.0, 11.0].into_iter().find(|&s| fits(name, s)).unwrap_or(11.0);
    let letters = name.chars().count();
    let shown = (0..=letters).rev().map(|n| super::brushes::shortened(name, n)).find(|t| fits(t, size)).unwrap_or_default();
    p.text(pos2(left, rect.center().y), Align2::LEFT_CENTER, shown, theme::font(size), ink);
    response
}

/// Whether library entry `entry` cannot be drawn (a custom material whose
/// shader is missing or does not compile).
fn broken(state: &State, entry: &Entry) -> Option<String> {
    let Settings::Custom(c) = &entry.material.settings else { return None };
    match state.shaders.get(&c.id) {
        Some(shader) => shader.failure(),
        None => Some(format!("The shader {} is not in the shaders folder: this shows as plain colour.", c.id)),
    }
}

/// The rows for `places` (in the library), in columns.
#[allow(clippy::too_many_arguments)]
fn rows(
    ui: &mut Ui,
    state: &State,
    thumbs: &mut Thumbs,
    places: &[usize],
    picked: Option<usize>,
    tips: bool,
    actions: &mut Vec<Action>,
) {
    let width = theme::row_width(ui, COLUMNS, GAP);
    let entries = state.library.entries();
    for line in places.chunks(COLUMNS) {
        ui.horizontal(|ui| {
            for &i in line {
                let entry = &entries[i];
                let key = thumbs.keys.get(i).copied();
                let picture = key.and_then(|k| thumbs.textures.get(&k));
                if picture.is_none() && !thumbs.wanted.contains(&i) {
                    thumbs.wanted.push(i);
                }
                let failure = broken(state, entry);
                let r = row(ui, width, picture, (&entry.material, Some(entry.origin)), picked == Some(i), failure.is_some());
                let tip = match &failure {
                    Some(why) => format!("{}\n{why}", entry.material.name),
                    None => format!("{}\n{}", entry.material.name, entry.material.about()),
                };
                if theme::tip(r, tips, &tip).clicked() {
                    actions.push(Action::Material(Ask::Select(i)));
                }
            }
        });
    }
}

/// The name box of one of the user's materials, as the Brush Library's.
fn rename(ui: &mut Ui, i: usize, name: &str, tips: bool, actions: &mut Vec<Action>) {
    let id = egui::Id::new("material_rename");
    let (mut of, mut text) = ui.data(|d| d.get_temp::<(String, String)>(id)).unwrap_or_default();
    if of != name {
        (of, text) = (name.to_owned(), name.to_owned());
    }
    let field = theme::tip(theme::labelled_field(ui, "Name", &mut text), tips, "Type a new name and press Enter.");
    if field.lost_focus() {
        let backed_out = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if !backed_out && text.trim() != name && !text.trim().is_empty() {
            actions.push(Action::Material(Ask::Rename(i, text.trim().to_owned())));
        }
        of.clear();
    }
    ui.data_mut(|d| d.insert_temp(id, (of, text)));
}

/// The sliders of a material's finish, on `m` itself. A built-in finish
/// gets the Look panel's own controls, laid over a Look that holds the
/// material's settings; a custom one gets one control per sidecar slider.
fn settings(ui: &mut Ui, state: &State, m: &mut Material, tips: bool) {
    let mut scratch = crate::look::Look::default();
    match &mut m.settings {
        Settings::Plain(p) => {
            look::color_row(ui, "Colour", &mut p.base_color);
            theme::slider(ui, "Metallic", &mut p.metallic, 0.0..=1.0, look::two);
            theme::slider(ui, "Smoothness", &mut p.smoothness, 0.0..=1.0, look::two);
        }
        Settings::Lure(s) => {
            scratch.lure = s.clone();
            look::lure(ui, &mut scratch, tips, 1.0);
            *s = scratch.lure;
        }
        Settings::Metal(s) => {
            scratch.metal = s.clone();
            look::metal(ui, &mut scratch, tips, 1.0);
            *s = scratch.metal;
        }
        Settings::Clay(s) => {
            scratch.clay = s.clone();
            look::clay(ui, &mut scratch, tips, 1.0);
            *s = scratch.clay;
        }
        Settings::Wood(s) => {
            scratch.wood = s.clone();
            look::wood(ui, &mut scratch, tips, 1.0);
            *s = scratch.wood;
        }
        Settings::Glow(g) => {
            let presets = crate::look::GLOWS.iter().map(|p| (p.name, p.description, p.cool));
            if let Some(i) = look::preset_grid(ui, tips, Some(g.glow.preset), presets) {
                g.glow.preset = i;
            }
            let s = &mut g.glow;
            theme::slider(ui, "Intensity", &mut s.intensity, 0.0..=3.0, look::two);
            theme::slider(ui, "Coverage", &mut s.coverage, 0.0..=2.0, look::two);
            theme::slider(ui, "Rim", &mut s.rim, 0.0..=2.0, look::two);
            theme::slider(ui, "Pattern Size", &mut s.pattern_size, 0.25..=3.0, look::two);
            theme::slider(ui, "Speed", &mut s.speed, 0.0..=3.0, look::two);
            theme::slider(ui, "Pulse", &mut s.pulse, 0.0..=2.0, look::two);
            theme::slider(ui, "Dim Surface", &mut s.dim, 0.0..=2.0, look::two);
            theme::slider(ui, "Hue Shift", &mut s.hue_shift, 0.0..=1.0, look::percent);
            look::color_row(ui, "Surface Colour", &mut g.base.base_color);
        }
        Settings::Custom(c) => {
            let Some(shader) = state.shaders.get(&c.id) else {
                theme::hint(ui, &format!("The shader {} is not in the shaders folder.", c.id));
                return;
            };
            for param in &shader.sidecar.params {
                let mut value = param.fitted(c.values.get(&param.name));
                let r = match &mut value {
                    Value::Number(x) => theme::slider(ui, &param.name, x, param.min.min(param.max)..=param.max.max(param.min), look::two),
                    Value::Toggle(b) => theme::checkbox(ui, b, &param.name, ACCENT()),
                    Value::Colour(rgb) => ui.horizontal(|ui| {
                        let r = ui.color_edit_button_rgb(rgb);
                        ui.label(egui::RichText::new(&param.name).font(theme::font(12.0)).color(theme::TEXT()));
                        r
                    }).inner,
                };
                if !param.description.is_empty() {
                    theme::tip(r, tips, &param.description);
                }
                c.values.insert(param.name.clone(), value);
            }
        }
    }
}

/// What can be done with the library material `i`, the one paint uses.
fn manage(ui: &mut Ui, state: &State, i: usize, tips: bool, actions: &mut Vec<Action>) {
    let entry = &state.library.entries()[i];
    let ask = |ask: Ask| Action::Material(ask);
    let user = entry.origin == Origin::User;
    theme::heading(ui, &entry.material.name);
    if user {
        rename(ui, i, &entry.material.name, tips, actions);
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let w = theme::row_width(ui, 3, GAP);
        let text = "Make a material of your own that starts as this one is now, to change and rename.";
        if theme::tip(theme::button(ui, w, "Duplicate", ButtonStyle::default()), tips, text).clicked() {
            actions.push(ask(Ask::Duplicate(i)));
        }
        let text = "Write this material to a file you can keep or pass on.";
        if theme::tip(theme::button(ui, w, "Export...", ButtonStyle::default()), tips, text).clicked() {
            actions.push(ask(Ask::Export(i)));
        }
        let text = if user { "Take this material out of the library." } else { "Materials that come with the app stay." };
        let delete = ui.add_enabled_ui(user, |ui| theme::button(ui, w, "Delete", ButtonStyle::default())).inner;
        if theme::tip(delete, tips, text).clicked() && user {
            actions.push(ask(Ask::Delete(i)));
        }
    });
    if let Some(why) = broken(state, entry) {
        ui.add(egui::Label::new(egui::RichText::new(why).font(theme::font(11.0)).italics().color(theme::ERROR)).wrap());
    }
    if !user {
        let why = match entry.origin {
            Origin::Shader => "A custom material, as its file has it. Duplicate it to change its sliders.",
            _ => "Comes with the app. Duplicate it to make one of your own you can change.",
        };
        theme::hint(ui, why);
        return;
    }
    let mut m = entry.material.clone();
    theme::heading(ui, "Painted Colour");
    let modes: Vec<&str> = TintMode::ALL.iter().map(|t| t.name()).collect();
    let at = TintMode::ALL.iter().position(|t| *t == m.tint.mode).unwrap_or(0);
    if let Some(k) = look::choice(ui, &modes, at) {
        m.tint.mode = TintMode::ALL[k];
    }
    if m.tint.mode != TintMode::Off {
        theme::slider(ui, "Amount", &mut m.tint.amount, 0.0..=1.0, look::percent);
    }
    theme::hint(ui, match m.tint.mode {
        TintMode::Off => "Colour painted over this material does not show on it.",
        TintMode::Multiply => "Colour painted over this material tints it, as a stain would.",
        TintMode::Replace => "Colour painted over this material takes the place of its own; its shine and bumps stay.",
    });
    look::color_row(ui, "Swatch", &mut m.swatch);
    theme::heading(ui, "Finish");
    settings(ui, state, &mut m, tips);
    if m != entry.material {
        actions.push(ask(Ask::Set(i, m)));
    }
}

/// The scene's palette: every material the scene's paint can use.
fn palette_rows(ui: &mut Ui, state: &State, paint: &ScenePaint, pick: &PaintPick, tips: bool, actions: &mut Vec<Action>) {
    theme::heading(ui, "This Scene's Palette");
    let width = theme::row_width(ui, COLUMNS, GAP);
    let picked = match pick.material {
        Some(PaintMaterial::Palette(slot)) => Some(slot),
        _ => None,
    };
    for line in paint.palette.chunks(COLUMNS) {
        ui.horizontal(|ui| {
            for entry in line {
                let m = material_of(entry);
                let failure = match &m.settings {
                    Settings::Custom(c) => state.shaders.get(&c.id).and_then(|s| s.failure()),
                    _ => None,
                };
                let r = row(ui, width, None, (&m, None), picked == Some(entry.slot), failure.is_some());
                let tip = failure.unwrap_or_else(|| format!("{}\nPaint with this material of the scene.", m.name));
                if theme::tip(r, tips, &tip).clicked() {
                    actions.push(Action::Material(Ask::SelectSlot(entry.slot)));
                }
            }
        });
    }
    if let Some(slot) = picked.filter(|s| paint.palette.iter().any(|e| e.slot == *s)) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            let w = theme::row_width(ui, 2, GAP);
            let text = "Size this material's pattern to the model as it is now.";
            if theme::tip(theme::button(ui, w, "Re-measure", ButtonStyle::default()), tips, text).clicked() {
                actions.push(Action::Material(Ask::Remeasure(slot)));
            }
            let text = "Keep this material in your library, to use in other scenes.";
            if theme::tip(theme::button(ui, w, "Save as Material", ButtonStyle::default()), tips, text).clicked() {
                actions.push(Action::Material(Ask::SaveSlot(slot)));
            }
        });
        let entries = state.library.entries();
        egui::ComboBox::from_id_salt("material_replace")
            .selected_text("Replace With...")
            .width(ui.available_width() - 8.0)
            .show_ui(ui, |ui| {
                for (i, e) in entries.iter().enumerate() {
                    if ui.selectable_label(false, &e.material.name).clicked() {
                        actions.push(Action::Material(Ask::Replace(slot, i)));
                    }
                }
            });
    }
    let w = ui.available_width();
    let text = "Take out of the palette the materials no object's paint uses.";
    if theme::tip(theme::button(ui, w, "Remove Unused", ButtonStyle::default()), tips, text).clicked() {
        actions.push(Action::Material(Ask::RemoveUnused));
    }
}

/// The Materials card.
pub fn card(
    ui: &mut Ui,
    state: &State,
    paint: &mut ScenePaint,
    pick: &PaintPick,
    thumbs: &mut Thumbs,
    tips: bool,
    actions: &mut Vec<Action>,
) {
    let entries = state.library.entries();
    let picked = match &pick.material {
        Some(PaintMaterial::Library(name)) => state.library.named(name),
        _ => None,
    };
    let summary = match &pick.material {
        Some(PaintMaterial::Library(name)) => name.clone(),
        Some(PaintMaterial::Palette(slot)) => {
            paint.palette.iter().find(|e| e.slot == *slot).map(|e| e.name.clone()).unwrap_or_default()
        }
        None => String::new(),
    };
    theme::category(ui, "Materials", CARD, &summary, false, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let text = "Show the colour painted on the model.";
            theme::tip(theme::checkbox(ui, &mut paint.show_polypaint, "Show Polypaint", ACCENT()), tips, text);
            let text = "Show the materials painted on the model. Off, it shows the Look's own finish everywhere.";
            theme::tip(theme::checkbox(ui, &mut paint.show_materials, "Show Materials", ACCENT()), tips, text);
        });
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        let of = |f: &dyn Fn(&Entry) -> bool| (0..entries.len()).filter(|&i| f(&entries[i])).collect::<Vec<usize>>();
        let built_in = |kind: Kind| move |e: &Entry| e.origin == Origin::BuiltIn && e.material.kind() == kind;
        let groups: [(&str, Vec<usize>); 8] = [
            ("", of(&built_in(Kind::Plain))),
            ("Lure Plastic", of(&built_in(Kind::Lure))),
            ("Metal", of(&built_in(Kind::Metal))),
            ("Clay", of(&built_in(Kind::Clay))),
            ("Wood", of(&built_in(Kind::Wood))),
            ("Glow", of(&built_in(Kind::Glow))),
            ("Custom Shaders", of(&|e: &Entry| e.origin == Origin::Shader)),
            ("Yours", of(&|e: &Entry| e.origin == Origin::User)),
        ];
        thumbs.wanted.clear();
        for (heading, places) in &groups {
            if places.is_empty() {
                continue;
            }
            if !heading.is_empty() {
                theme::heading(ui, heading);
            }
            rows(ui, state, thumbs, places, picked, tips, actions);
        }
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.add_space(2.0);
        if let Some(i) = picked {
            manage(ui, state, i, tips, actions);
        }
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let text = "Bring in a material from a file: one a friend sent, or one you exported or deleted.";
            if theme::tip(theme::button(ui, w, "Import Material...", ButtonStyle::default()), tips, text).clicked() {
                actions.push(Action::Material(Ask::Import));
            }
            let text = "Make a material of your own from the Look's surface finish as it is now, sliders and all.";
            if theme::tip(theme::button(ui, w, "Save Look as Material", ButtonStyle::default()), tips, text).clicked() {
                actions.push(Action::Material(Ask::SaveLook));
            }
        });
        if !paint.palette.is_empty() {
            palette_rows(ui, state, paint, pick, tips, actions);
        }
        let w = ui.available_width();
        let text = "Read the custom materials in the shaders folder again (changed files are read by themselves).";
        if theme::tip(theme::button(ui, w, "Reload Custom Materials", ButtonStyle::default()), tips, text).clicked() {
            actions.push(Action::Material(Ask::ReloadShaders));
        }
        if let Some(folder) = state.shaders.folder() {
            theme::hint(ui, &format!("Custom materials are .wgsl files with a .json beside them, in {}.", folder.display()));
        }
        theme::hint(ui, "Pick a material to paint with. The palette holds the materials this scene's paint uses.");
    });
}
