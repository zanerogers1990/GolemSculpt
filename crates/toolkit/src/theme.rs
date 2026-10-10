//! Colours, type and the hand-painted widgets. The palette and metrics are
//! the Unity app's, so the two look like the same tool.
//!
//! They live here, not in the app, so a tool's panel is painted with exactly
//! what the app's own panels are; the app's `ui::theme` is this module under
//! another name. A panel is a stack of [`category`] cards holding [`button`]s,
//! [`tile`]s, [`slider`]s, [`checkbox`]es, [`heading`]s, [`hint`]s and
//! [`line`]s. Use these rather than egui's stock widgets, which look like
//! another program.

use std::sync::Arc;

use egui::{
    Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect,
    Response, Sense, Stroke, StrokeKind, TextureId, Ui, Vec2, pos2, vec2,
};

pub const PANEL: Color32 = Color32::from_rgba_premultiplied(18, 18, 23, 224);
pub const CARD: Color32 = Color32::from_rgb(29, 29, 36);
pub const HEADER: Color32 = Color32::from_rgb(38, 38, 47);
pub const CONTROL: Color32 = Color32::from_rgb(51, 51, 56);
pub const FIELD: Color32 = Color32::from_rgb(15, 15, 19);
pub const ACCENT: Color32 = Color32::from_rgb(64, 140, 242);
pub const TEXT: Color32 = Color32::from_rgb(230, 230, 237);
pub const HINT: Color32 = Color32::from_rgb(166, 166, 178);
pub const SUB: Color32 = Color32::from_rgb(219, 92, 92);
pub const MASK: Color32 = Color32::from_rgb(242, 166, 38);
pub const OK: Color32 = Color32::from_rgb(140, 217, 140);
pub const ERROR: Color32 = Color32::from_rgb(242, 115, 102);
pub const KEY: Color32 = Color32::from_rgb(242, 217, 128);
pub const AXIS: [Color32; 3] = [
    Color32::from_rgb(255, 64, 64),
    Color32::from_rgb(89, 255, 89),
    Color32::from_rgb(76, 140, 255),
];

pub fn hairline() -> Stroke {
    Stroke::new(1.0, Color32::from_white_alpha(18))
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Installs Segoe UI (the Unity app's face) and the dark visuals.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str, family: FontFamily| {
        let path = format!("C:\\Windows\\Fonts\\{file}");
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.to_owned());
            true
        } else {
            false
        }
    };
    add("segoe", "segoeui.ttf", FontFamily::Proportional);
    if !add("segoe-bold", "segoeuib.ttf", FontFamily::Name("bold".into())) {
        // No bold face available: fall back to the regular one.
        let regular = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        fonts.families.insert(FontFamily::Name("bold".into()), regular);
    } else {
        let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        fonts.families.entry(FontFamily::Name("bold".into())).or_default().extend(fallback);
    }
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.override_text_color = Some(TEXT);
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = Color32::from_rgb(20, 20, 26);
        v.window_stroke = hairline();
        v.extreme_bg_color = FIELD;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.6);
        v.widgets.inactive.bg_fill = CONTROL;
        v.widgets.inactive.weak_bg_fill = CONTROL;
        v.widgets.hovered.bg_fill = mix(CONTROL, Color32::WHITE, 0.08);
        v.widgets.hovered.weak_bg_fill = mix(CONTROL, Color32::WHITE, 0.08);
        v.widgets.active.bg_fill = ACCENT;
        v.widgets.active.weak_bg_fill = ACCENT;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
            w.corner_radius = CornerRadius::same(5);
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, TEXT);
        }
        style.spacing.item_spacing = vec2(6.0, 6.0);
        style.spacing.scroll.bar_width = 5.0;
        style.spacing.scroll.floating = true;
        style.interaction.tooltip_delay = 0.6;
        style.text_styles.insert(egui::TextStyle::Body, font(12.0));
        style.text_styles.insert(egui::TextStyle::Button, font(12.0));
        style.text_styles.insert(egui::TextStyle::Small, font(10.0));
    });
}

fn hover_overlay(ui: &Ui, rect: Rect, radius: f32, response: &Response) {
    if response.hovered() && ui.is_enabled() {
        ui.painter().rect_filled(rect, radius, Color32::from_white_alpha(20));
    }
}

/// Collapsible card with a coloured pill, like the Unity panels' categories.
pub fn category(
    ui: &mut Ui,
    title: &str,
    accent: Color32,
    summary: &str,
    default_open: bool,
    body: impl FnOnce(&mut Ui),
) {
    let id = ui.make_persistent_id(("category", title));
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let width = ui.available_width();
    egui::Frame::new().fill(CARD).corner_radius(7.0).show(ui, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
        if response.clicked() {
            open = !open;
            ui.data_mut(|d| d.insert_persisted(id, open));
        }
        let fill = if open { mix(HEADER, accent, 0.10) } else { HEADER };
        let rounding = if open {
            CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }
        } else {
            CornerRadius::same(7)
        };
        let p = ui.painter();
        p.rect_filled(rect, rounding, fill);
        hover_overlay(ui, rect, 7.0, &response);
        p.rect_filled(
            Rect::from_min_size(pos2(rect.left() + 7.0, rect.center().y - 8.0), vec2(3.0, 16.0)),
            1.5,
            accent,
        );
        p.text(pos2(rect.left() + 17.0, rect.center().y), Align2::LEFT_CENTER, title, bold(13.0), TEXT);
        // Chevron: right when closed, down when open.
        let c = pos2(rect.right() - 14.0, rect.center().y);
        let chevron = Stroke::new(1.5, Color32::from_white_alpha(140));
        if open {
            p.line(vec![c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)], chevron);
        } else {
            p.line(vec![c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)], chevron);
        }
        if !summary.is_empty() {
            p.text(
                pos2(rect.right() - 28.0, rect.center().y),
                Align2::RIGHT_CENTER,
                summary,
                font(11.0),
                accent.gamma_multiply(0.9),
            );
        }
        if open {
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 12 })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    body(ui);
                });
        }
    });
}

/// Opens the [`category`] titled `title` in this `ui`, as if the user had
/// clicked its header: for a card that should show itself when its tool
/// comes up. Call it just before drawing the category.
pub fn open_category(ui: &mut Ui, title: &str) {
    let id = ui.make_persistent_id(("category", title));
    ui.data_mut(|d| d.insert_persisted(id, true));
}

/// Sub-heading inside a category.
pub fn heading(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, text, bold(12.0), TEXT);
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font(11.0)).italics().color(HINT)).wrap());
}

pub struct ButtonStyle<'a> {
    pub icon: Option<TextureId>,
    pub active: Option<Color32>,
    pub badge: &'a str,
    pub height: f32,
}

impl Default for ButtonStyle<'_> {
    fn default() -> Self {
        Self { icon: None, active: None, badge: "", height: 28.0 }
    }
}

/// Text button with an optional icon, active colour and hotkey badge.
pub fn button(ui: &mut Ui, width: f32, text: &str, style: ButtonStyle) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, style.height), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let enabled = ui.is_enabled();
    let p = ui.painter();
    p.rect_filled(rect, 5.0, style.active.unwrap_or(CONTROL));
    hover_overlay(ui, rect, 5.0, &response);
    let ink = if !enabled {
        Color32::from_white_alpha(70)
    } else if style.active.is_some() {
        Color32::WHITE
    } else {
        TEXT
    };
    let galley = p.layout_no_wrap(text.to_owned(), font(12.0), ink);
    let icon_w = if style.icon.is_some() { 16.0 + 5.0 } else { 0.0 };
    let mut x = rect.center().x - (galley.size().x + icon_w) * 0.5;
    if let Some(icon) = style.icon {
        let r = Rect::from_min_size(pos2(x, rect.center().y - 8.0), Vec2::splat(16.0));
        let tint = if style.active.is_some() { Color32::WHITE } else { Color32::from_white_alpha(210) };
        p.image(icon, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), if enabled { tint } else { ink });
        x += icon_w;
    }
    p.galley(pos2(x, rect.center().y - galley.size().y * 0.5), galley, ink);
    if !style.badge.is_empty() {
        p.text(
            rect.right_top() + vec2(-4.0, 2.0),
            Align2::RIGHT_TOP,
            style.badge,
            bold(9.0),
            Color32::from_white_alpha(102),
        );
    }
    response
}

/// Square-ish icon tile, used for the brush grid.
pub fn tile(
    ui: &mut Ui,
    size: Vec2,
    icon: Option<TextureId>,
    caption: &str,
    badge: &str,
    active: Option<Color32>,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    p.rect_filled(rect, 5.0, active.unwrap_or(CONTROL));
    hover_overlay(ui, rect, 5.0, &response);
    let tint = if active.is_some() { Color32::WHITE } else { Color32::from_white_alpha(210) };
    let icon_size = if caption.is_empty() { (size.y - 12.0).min(28.0) } else { 22.0 };
    let icon_y = if caption.is_empty() { rect.center().y } else { rect.top() + 5.0 + icon_size * 0.5 };
    if let Some(icon) = icon {
        let r = Rect::from_center_size(pos2(rect.center().x, icon_y), Vec2::splat(icon_size));
        p.image(icon, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
    }
    if !caption.is_empty() {
        p.text(
            pos2(rect.center().x, rect.bottom() - 4.0),
            Align2::CENTER_BOTTOM,
            caption,
            font(10.0),
            tint,
        );
    }
    if !badge.is_empty() {
        p.text(
            rect.right_top() + vec2(-4.0, 2.0),
            Align2::RIGHT_TOP,
            badge,
            bold(9.0),
            Color32::from_white_alpha(102),
        );
    }
    response
}

/// Caption row with a right-aligned readout, then a thin slider. Returns true
/// when the value changed.
pub fn slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    readout: impl Fn(f32) -> String,
) -> Response {
    let width = ui.available_width();
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, 38.0), Sense::click_and_drag());
    let (min, max) = (*range.start(), *range.end());
    let track_y = rect.bottom() - 10.0;
    let left = rect.left() + 7.0;
    let right = rect.right() - 7.0;
    if let Some(pos) = response.interact_pointer_pos() {
        if response.is_pointer_button_down_on() {
            let t = ((pos.x - left) / (right - left)).clamp(0.0, 1.0);
            let new = min + (max - min) * t;
            if new != *value {
                *value = new;
                response.mark_changed();
            }
        }
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    p.text(rect.left_top() + vec2(0.0, 1.0), Align2::LEFT_TOP, label, font(12.0), TEXT);
    p.text(rect.right_top() + vec2(0.0, 2.0), Align2::RIGHT_TOP, readout(*value), font(11.0), HINT);
    let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    let x = left + (right - left) * t;
    p.rect_filled(
        Rect::from_min_max(pos2(rect.left(), track_y - 2.0), pos2(rect.right(), track_y + 2.0)),
        2.0,
        FIELD,
    );
    p.rect_filled(
        Rect::from_min_max(pos2(rect.left(), track_y - 2.0), pos2(x, track_y + 2.0)),
        2.0,
        ACCENT,
    );
    let thumb = if response.is_pointer_button_down_on() {
        Color32::from_rgb(204, 224, 255)
    } else if response.hovered() {
        Color32::from_rgb(235, 242, 255)
    } else {
        Color32::WHITE
    };
    p.circle_filled(pos2(x, track_y + 1.0), 8.0, Color32::from_black_alpha(60));
    p.circle_filled(pos2(x, track_y), 7.0, thumb);
    response
}

pub fn checkbox(ui: &mut Ui, value: &mut bool, label: &str, fill: Color32) -> Response {
    let width = ui.available_width();
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font(12.0), TEXT);
    let size = vec2((24.0 + galley.size().x).min(width), 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let p = ui.painter();
    let well = Rect::from_min_size(pos2(rect.left(), rect.center().y - 8.0), Vec2::splat(16.0));
    p.rect_filled(well, 4.0, if *value { fill } else { FIELD });
    p.rect_stroke(well, 4.0, Stroke::new(1.0, Color32::from_white_alpha(36)), StrokeKind::Inside);
    if response.hovered() {
        p.rect_filled(well, 4.0, Color32::from_white_alpha(20));
    }
    if *value {
        let c = well.center();
        p.line(
            vec![c + vec2(-4.0, 0.0), c + vec2(-1.5, 3.0), c + vec2(4.5, -3.5)],
            Stroke::new(2.0, Color32::WHITE),
        );
    }
    p.galley(pos2(rect.left() + 24.0, rect.center().y - galley.size().y * 0.5), galley, TEXT);
    response
}

/// Lays out `n` equal-width cells in a row and returns each cell's width.
pub fn row_width(ui: &Ui, n: usize, spacing: f32) -> f32 {
    ((ui.available_width() - spacing * (n as f32 - 1.0)) / n as f32).floor()
}

/// Adds a tooltip when tooltips are switched on (`cx.tips`).
pub fn tip(response: Response, on: bool, text: &str) -> Response {
    if on { response.on_hover_text(text) } else { response }
}

/// A small line of text in a colour: a count, a result ([`OK`]), a problem
/// ([`ERROR`]), a remark ([`HINT`]). Wraps to the panel.
pub fn line(ui: &mut Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font(11.0)).color(color)).wrap());
}

/// The rows of a shortcuts card: the key in yellow, what it does beside it.
pub fn shortcuts(ui: &mut Ui, rows: &[(&str, &str)]) {
    for (key, what) in rows {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::hover());
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, key, font(11.0), KEY);
        ui.painter().text(rect.left_center() + vec2(118.0, 0.0), Align2::LEFT_CENTER, what, font(11.0), TEXT);
    }
}

/// A one-line text box `width` wide. The edit is finished when the returned
/// response says `lost_focus()`; Esc is the user backing out, so check
/// `ui.input(|i| i.key_pressed(egui::Key::Escape))` before taking the text.
pub fn text_field(ui: &mut Ui, text: &mut String, width: f32) -> Response {
    ui.add(egui::TextEdit::singleline(text).font(font(12.0)).desired_width(width))
}

/// A caption on the left and a text box filling the rest of the row.
pub fn labelled_field(ui: &mut Ui, label: &str, text: &mut String) -> Response {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(96.0, 22.0), Sense::hover());
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, label, font(12.0), TEXT);
        text_field(ui, text, ui.available_width())
    })
    .inner
}

/// A count for a header or a status line: 950, 12k, 1.20M.
pub fn count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.0}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}
