//! Colours, type and the hand-painted widgets. The palette and metrics are
//! the Unity app's, so the two look like the same tool.
//!
//! They live here, not in the app, so a tool's panel is painted with exactly
//! what the app's own panels are; the app's `ui::theme` is this module under
//! another name. A panel is a stack of [`category`] cards holding [`button`]s,
//! [`tile`]s, [`slider`]s, [`checkbox`]es, [`heading`]s, [`hint`]s and
//! [`line`]s. Use these rather than egui's stock widgets, which look like
//! another program. Any card can be pulled out of its panel to float over
//! the viewport: that is `category`'s doing (see [`crate::floating`]), and a
//! panel's code is the same either way.

use std::sync::Arc;

use egui::{
    Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect,
    Response, Sense, Stroke, StrokeKind, TextureId, Ui, Vec2, pos2, vec2,
};

pub use crate::palette::{ACCENT, CARD, CONTROL, FIELD, HEADER, HINT, PANEL, Palette, TEXT, THEMES};
pub use crate::palette::{current as current_theme, palette, set as set_theme};

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

use crate::palette::mix;

/// `color` a step brighter: a tile of that colour with the pointer on it.
pub fn lit(color: Color32) -> Color32 {
    mix(color, Color32::WHITE, 0.15)
}

/// The disc the hold-Space wheels are laid out on: dark and see-through.
pub fn wheel_disc() -> Color32 {
    let field = FIELD();
    Color32::from_rgba_unmultiplied(field.r(), field.g(), field.b(), 200)
}

/// Installs Segoe UI (the Unity app's face) and the visuals of the theme
/// that is up (see [`crate::palette`]).
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
    crate::palette::visuals(ctx);
    ctx.all_styles_mut(|style| {
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
///
/// The small button beside the chevron pulls the card out of its panel to
/// float over the viewport, where the same `body` draws it in a window of
/// its own (see [`crate::floating`]).
pub fn category(
    ui: &mut Ui,
    title: &str,
    accent: Color32,
    summary: &str,
    default_open: bool,
    body: impl FnOnce(&mut Ui),
) {
    category_as(ui, title, title, accent, summary, default_open, body);
}

/// A [`category`] whose title changes with what it shows (the settings of
/// the brush in hand, say). `key` is the one name it floats under, so it
/// stays out of its panel when its title changes.
pub fn category_as(
    ui: &mut Ui,
    key: &str,
    title: &str,
    accent: Color32,
    summary: &str,
    default_open: bool,
    body: impl FnOnce(&mut Ui),
) {
    let id = ui.make_persistent_id(("category", title));
    let header = CardHeader { title, accent, summary };
    if crate::floating::is_floating(ui.ctx(), key) {
        // A floating card is always open: there is nothing left to ask for.
        asked_open(ui.ctx(), title);
        return crate::floating::show(ui, key, &header, id, body);
    }
    if crate::floating::hidden(ui) {
        return;
    }
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    if asked_open(ui.ctx(), title) && !open {
        open = true;
        ui.data_mut(|d| d.insert_persisted(id, true));
    }
    let width = ui.available_width();
    let mut pulled_out = None;
    egui::Frame::new().fill(CARD()).corner_radius(7.0).show(ui, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        let (rect, response) = ui.allocate_exact_size(vec2(width, CARD_HEADER_HEIGHT), Sense::click());
        let button = ui.interact(Corner::Chevron(open).button(rect), id.with("float"), Sense::click());
        crate::floating::note_button(ui.ctx(), key, button.rect);
        if button.clicked() {
            pulled_out = Some(rect);
        } else if response.clicked() {
            open = !open;
            ui.data_mut(|d| d.insert_persisted(id, open));
        }
        paint_card_header(ui, rect, &header, Corner::Chevron(open), &response, &button);
        button.on_hover_text("Float this card over the view. Drag it by its title; the same button puts it back.");
        note_open(ui.ctx(), title, open);
        if open {
            card_body(ui, body);
        }
    });
    if let Some(header) = pulled_out {
        crate::floating::pull_out(ui.ctx(), key, header);
    }
}

/// The height of a card's header: its title bar when it floats.
pub(crate) const CARD_HEADER_HEIGHT: f32 = 32.0;

/// What a card's header says.
pub(crate) struct CardHeader<'a> {
    pub title: &'a str,
    pub accent: Color32,
    pub summary: &'a str,
}

/// What sits at the right end of a card's header.
#[derive(Clone, Copy)]
pub(crate) enum Corner {
    /// In its panel: the chevron (down when the card is open), with the
    /// button that floats the card beside it.
    Chevron(bool),
    /// Floating: the button that puts the card back in its panel.
    Dock,
}

impl Corner {
    /// The part of the header at `rect` that is the float or dock button.
    /// As tall as the header and wider than its picture, for a pen.
    pub(crate) fn button(self, rect: Rect) -> Rect {
        let (centre, width) = match self {
            Corner::Chevron(_) => (rect.right() - 38.0, 24.0),
            Corner::Dock => (rect.right() - 17.0, 30.0),
        };
        Rect::from_center_size(pos2(centre, rect.center().y), vec2(width, rect.height()))
    }

    /// Whether the header is tinted and square-bottomed, for a body under it.
    fn open(self) -> bool {
        !matches!(self, Corner::Chevron(false))
    }
}

/// Paints a card's header in `rect`: the pill, the title, the summary and
/// what its corner holds. `response` is the header's own and `button` the
/// float or dock button's, for their hover looks.
pub(crate) fn paint_card_header(
    ui: &Ui,
    rect: Rect,
    header: &CardHeader,
    corner: Corner,
    response: &Response,
    button: &Response,
) {
    let CardHeader { title, accent, summary } = *header;
    let fill = if corner.open() { mix(HEADER(), accent, 0.10) } else { HEADER() };
    let rounding = if corner.open() { CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 } } else { CornerRadius::same(7) };
    let p = ui.painter();
    p.rect_filled(rect, rounding, fill);
    hover_overlay(ui, rect, 7.0, response);
    p.rect_filled(
        Rect::from_min_size(pos2(rect.left() + 7.0, rect.center().y - 8.0), vec2(3.0, 16.0)),
        1.5,
        accent,
    );
    let title = p.layout_no_wrap(title.to_owned(), bold(13.0), TEXT());
    let title_end = rect.left() + 17.0 + title.size().x;
    p.galley(pos2(rect.left() + 17.0, rect.center().y - title.size().y * 0.5), title, TEXT());
    let button_rect = corner.button(rect);
    if let Corner::Chevron(open) = corner {
        // Chevron: right when closed, down when open.
        let c = pos2(rect.right() - 14.0, rect.center().y);
        let chevron = Stroke::new(1.5, Color32::from_white_alpha(140));
        if open {
            p.line(vec![c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)], chevron);
        } else {
            p.line(vec![c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)], chevron);
        }
    }
    paint_window_button(ui, button_rect, matches!(corner, Corner::Dock), button);
    if !summary.is_empty() {
        // Cut short with an ellipsis rather than run over the title.
        let end = button_rect.left() - 2.0;
        let tint = accent.gamma_multiply(0.9);
        let mut job = egui::text::LayoutJob::simple_singleline(summary.to_owned(), font(11.0), tint);
        job.wrap = egui::text::TextWrapping {
            max_width: (end - title_end - 10.0).max(0.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('\u{2026}'),
        };
        let summary = p.layout_job(job);
        let at = Align2::RIGHT_CENTER.anchor_size(pos2(end, rect.center().y), summary.size());
        p.galley(at.min, summary, tint);
    }
}

/// The picture on the float button (a window with an arrow leaving it) or,
/// with `dock`, on the button that puts a floating card back (the arrow
/// going in).
fn paint_window_button(ui: &Ui, rect: Rect, dock: bool, response: &Response) {
    let hovered = response.hovered() && ui.is_enabled();
    let p = ui.painter();
    let c = rect.center();
    if hovered {
        p.rect_filled(Rect::from_center_size(c, Vec2::splat(22.0)), 5.0, Color32::from_white_alpha(28));
    }
    let ink = Stroke::new(1.5, Color32::from_white_alpha(if hovered { 235 } else { 130 }));
    // The window, open at its top-right corner where the arrow passes.
    let frame = [vec2(-0.5, -4.5), vec2(-5.0, -4.5), vec2(-5.0, 5.0), vec2(4.5, 5.0), vec2(4.5, 0.5)];
    p.line(frame.iter().map(|&offset| c + offset).collect(), ink);
    let (inside, outside) = (c, c + vec2(5.5, -5.5));
    let (tail, head, back) = if dock { (outside, inside, 1.0) } else { (inside, outside, -1.0) };
    p.line_segment([tail, head], ink);
    p.line(vec![head + vec2(4.0 * back, 0.0), head, head + vec2(0.0, -4.0 * back)], ink);
}

/// A card's contents under its header, with the card's margins.
/// A card with no header: a plate for what is always shown, so it cannot
/// be folded away or floated (the brush's size and strength, say).
pub fn plate(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    egui::Frame::new().fill(CARD()).corner_radius(7.0).show(ui, |ui| {
        ui.set_width(width);
        egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 10, top: 10, bottom: 10 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            body(ui);
        });
    });
}

pub(crate) fn card_body(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 12 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        body(ui);
    });
}

/// Opens the [`category`] titled `title`, as if the user had clicked its
/// header: for a card that should show itself when its tool comes up. Call
/// it before drawing the category.
///
/// The card opens the next time it is drawn where it can be seen. That is
/// usually this frame, but a panel that is folded away still runs its code
/// (for its floating cards, see [`crate::floating::only`]), and a request
/// made in such a run must not be used up there: it waits for the panel to
/// come back. So a caller can make the request once, wherever it is run.
pub fn open_category(ui: &mut Ui, title: &str) {
    opening(ui.ctx(), |titles| titles.insert(title.to_owned()));
}

/// The cards [`open_category`] was asked to open and that have not been
/// drawn since, by title.
#[derive(Clone, Default)]
struct Opening(std::collections::BTreeSet<String>);

fn opening<R>(ctx: &egui::Context, f: impl FnOnce(&mut std::collections::BTreeSet<String>) -> R) -> R {
    ctx.data_mut(|d| f(&mut d.get_temp_mut_or_default::<Opening>(egui::Id::new("categories_to_open")).0))
}

/// Whether the card titled `title` was asked to open. Asking is over once
/// this is called: call it only where the card is about to be drawn.
fn asked_open(ctx: &egui::Context, title: &str) -> bool {
    opening(ctx, |titles| titles.remove(title))
}

/// How each card was when it was last drawn in its panel, by title: open
/// (true) or closed.
#[derive(Clone, Default)]
struct Drawn(std::collections::BTreeMap<String, bool>);

fn note_open(ctx: &egui::Context, title: &str, open: bool) {
    ctx.data_mut(|d| {
        let drawn = &mut d.get_temp_mut_or_default::<Drawn>(egui::Id::new("categories_drawn")).0;
        if drawn.get(title) != Some(&open) {
            drawn.insert(title.to_owned(), open);
        }
    });
}

/// Whether the card titled `title` was open when it was last drawn in its
/// panel, or `None` if it has not been drawn there. The card's own state is
/// kept under an id only the panel can make, so this is how anything else
/// finds out: a check of the interface, for one.
pub fn category_is_open(ctx: &egui::Context, title: &str) -> Option<bool> {
    ctx.data(|d| d.get_temp::<Drawn>(egui::Id::new("categories_drawn"))).and_then(|drawn| drawn.0.get(title).copied())
}

/// Sub-heading inside a category.
pub fn heading(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, text, bold(12.0), TEXT());
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font(11.0)).italics().color(HINT())).wrap());
}

/// A small heading that folds what is under it away when pressed: a
/// triangle (down while open, right while folded) and the title. Whether it
/// is open is kept by title, under the id a [`category`] of that title would
/// have, so an app that remembers its cards remembers these the same way.
/// It starts open or not as `default_open` says. `open_anyway` shows it open
/// whatever was last chosen (the group of the tool that is up, say). Gives
/// back whether to draw what is under it.
pub fn fold(ui: &mut Ui, title: &str, default_open: bool, open_anyway: bool) -> bool {
    let id = ui.make_persistent_id(("category", title));
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::click());
    if response.clicked() {
        open = !open || open_anyway;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    let shown = open || open_anyway;
    let ink = if response.hovered() { TEXT() } else { HINT() };
    let c = pos2(rect.left() + 5.0, rect.center().y);
    let points = if shown {
        vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
    } else {
        vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(points, ink, Stroke::NONE));
    ui.painter().text(pos2(rect.left() + 13.0, rect.center().y), Align2::LEFT_CENTER, title, bold(11.0), ink);
    shown
}

/// What a card, or a part of one, is for and how to use it, kept out of the
/// way: one faint line, "How it works", that shows `text` when pointed at.
/// It shows whatever the Tooltips setting says, since the line is there to
/// be pointed at. For what a single button does, its own tip is the place.
pub fn about(ui: &mut Ui, text: &str) {
    const LABEL: &str = "How it works";
    let width = ui.painter().layout_no_wrap(LABEL.to_owned(), font(11.0), HINT()).size().x;
    let (rect, response) = ui.allocate_exact_size(vec2(18.0 + width, 16.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let ink = if response.hovered() { TEXT() } else { HINT() };
        let p = ui.painter();
        let mark = pos2(rect.left() + 7.0, rect.center().y);
        p.circle_stroke(mark, 6.0, Stroke::new(1.0, ink));
        p.text(mark + vec2(0.0, 0.5), Align2::CENTER_CENTER, "i", bold(9.0), ink);
        p.text(pos2(rect.left() + 18.0, rect.center().y), Align2::LEFT_CENTER, LABEL, font(11.0), ink);
    }
    response.on_hover_ui(|ui| {
        ui.set_max_width(280.0);
        ui.label(egui::RichText::new(text).font(font(12.0)));
    });
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
    p.rect_filled(rect, 5.0, style.active.unwrap_or(CONTROL()));
    hover_overlay(ui, rect, 5.0, &response);
    let ink = if !enabled {
        Color32::from_white_alpha(70)
    } else if style.active.is_some() {
        Color32::WHITE
    } else {
        TEXT()
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
    p.rect_filled(rect, 5.0, active.unwrap_or(CONTROL()));
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
    let (rect, response) = ui.allocate_exact_size(vec2(width, SLIDER_HEIGHT), Sense::click_and_drag());
    slider_in(ui, rect, response, label, value, range, readout)
}

/// The height of a [`slider`]: its caption row and its track.
pub const SLIDER_HEIGHT: f32 = 38.0;

/// How far inside its rectangle a slider's track starts and ends, so the
/// thumb never hangs over the edge.
const SLIDER_INSET: f32 = 7.0;

/// The value a slider takes for a pointer at `x`, on a track that runs from
/// `left` to `right`: `min` at the left end and anywhere before it, `max` at
/// the right end and anywhere past it. A track with no length gives `min`.
pub fn slider_value(x: f32, left: f32, right: f32, min: f32, max: f32) -> f32 {
    let span = right - left;
    let t = if span > 0.0 && x.is_finite() { ((x - left) / span).clamp(0.0, 1.0) } else { 0.0 };
    min + (max - min) * t
}

/// The [`slider`] painted in `rect`, moved by a `response` the caller
/// allocated: for a slider that should answer a press anywhere on a larger
/// plate around it, so a pen does not have to land on the thin track (the
/// bars under the hold-Space wheels).
pub fn slider_in(
    ui: &Ui,
    rect: Rect,
    mut response: Response,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    readout: impl Fn(f32) -> String,
) -> Response {
    let (min, max) = (*range.start(), *range.end());
    let track_y = rect.bottom() - 10.0;
    let left = rect.left() + SLIDER_INSET;
    let right = rect.right() - SLIDER_INSET;
    if let Some(pos) = response.interact_pointer_pos() {
        if response.is_pointer_button_down_on() {
            let new = slider_value(pos.x, left, right, min, max);
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
    p.text(rect.left_top() + vec2(0.0, 1.0), Align2::LEFT_TOP, label, font(12.0), TEXT());
    p.text(rect.right_top() + vec2(0.0, 2.0), Align2::RIGHT_TOP, readout(*value), font(11.0), HINT());
    let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
    let x = left + (right - left) * t;
    p.rect_filled(
        Rect::from_min_max(pos2(rect.left(), track_y - 2.0), pos2(rect.right(), track_y + 2.0)),
        2.0,
        FIELD(),
    );
    p.rect_filled(
        Rect::from_min_max(pos2(rect.left(), track_y - 2.0), pos2(x, track_y + 2.0)),
        2.0,
        ACCENT(),
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
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font(12.0), TEXT());
    let size = vec2((24.0 + galley.size().x).min(width), 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let p = ui.painter();
    let well = Rect::from_min_size(pos2(rect.left(), rect.center().y - 8.0), Vec2::splat(16.0));
    p.rect_filled(well, 4.0, if *value { fill } else { FIELD() });
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
    p.galley(pos2(rect.left() + 24.0, rect.center().y - galley.size().y * 0.5), galley, TEXT());
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
        ui.painter().text(rect.left_center() + vec2(118.0, 0.0), Align2::LEFT_CENTER, what, font(11.0), TEXT());
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
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, label, font(12.0), TEXT());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The track's ends give the range's ends exactly, anywhere past an end
    /// gives that end, and nothing a pointer can report gives a value that
    /// is not a number.
    #[test]
    fn a_slider_maps_the_pointer_onto_its_range_and_stops_at_the_ends() {
        let (left, right) = (100.0, 300.0);
        let at = |x: f32| slider_value(x, left, right, 0.01, 1.0);
        assert_eq!(at(left), 0.01);
        assert_eq!(at(right), 1.0);
        assert!((at(200.0) - 0.505).abs() < 1e-6);
        assert!((at(250.0) - 0.7525).abs() < 1e-6);
        assert_eq!(at(-5000.0), 0.01, "left of the track is the lowest value");
        assert_eq!(at(5000.0), 1.0, "right of it is the highest");
        assert_eq!(at(f32::NEG_INFINITY), 0.01);
        assert_eq!(at(f32::NAN), 0.01, "a position that is not a number moves nothing above the lowest value");
        // A range that runs the other way, or below zero, maps the same way.
        assert_eq!(slider_value(150.0, left, right, -1.0, 1.0), -0.5);
        assert_eq!(slider_value(150.0, left, right, 1.0, 0.0), 0.75);
        // A track with no length, or one turned inside out, gives the lowest.
        assert_eq!(slider_value(100.0, 100.0, 100.0, 0.01, 1.0), 0.01);
        assert_eq!(slider_value(100.0, 300.0, 100.0, 0.01, 1.0), 0.01);
    }

    /// One frame of a panel holding the closed card "Size" and another card:
    /// in view, or folded away and run out of sight because a card floats.
    /// `ask` asks for "Size" to open first. True when its body was drawn.
    fn size_card_frame(ctx: &egui::Context, folded: bool, ask: bool) -> bool {
        let mut drawn = false;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0));
        let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        ctx.run_ui(input, |root| {
            let mut panel = |ui: &mut Ui| {
                if ask {
                    open_category(ui, "Size");
                }
                category(ui, "Size", ACCENT(), "", false, |_| drawn = true);
                category(ui, "Other", ACCENT(), "", false, |_| {});
            };
            if folded {
                crate::floating::only(root, "right", 240.0, panel);
            } else {
                egui::Area::new(egui::Id::new("panel")).fixed_pos(pos2(10.0, 10.0)).show(root.ctx(), |ui| {
                    ui.set_width(240.0);
                    panel(ui);
                });
            }
        })
        .drop_without_applying_deltas();
        drawn
    }

    /// A card asked to open while its panel is folded away, and run out of
    /// sight for a floating card, is open when the panel comes back: the
    /// request is not used up where nobody can see it.
    #[test]
    fn a_card_asked_to_open_while_its_panel_is_folded_away_opens_when_it_is_back() {
        let ctx = egui::Context::default();
        install(&ctx);
        crate::floating::float(&ctx, "Other", pos2(600.0, 100.0));
        assert!(!size_card_frame(&ctx, false, false), "it starts closed");
        assert!(!size_card_frame(&ctx, true, true), "folded away, nothing of it is drawn");
        assert!(!size_card_frame(&ctx, true, false));
        assert!(size_card_frame(&ctx, false, false), "back in view it opens, as it was asked to");
        assert!(size_card_frame(&ctx, false, false), "and stays open without being asked again");
    }

    /// Asked for in view, a card opens in that same frame, as it always did;
    /// and a card that floats (always open) leaves no request waiting to
    /// spring it open once it is docked and closed by hand.
    #[test]
    fn a_card_asked_to_open_in_view_opens_at_once() {
        let ctx = egui::Context::default();
        install(&ctx);
        assert!(!size_card_frame(&ctx, false, false));
        assert!(size_card_frame(&ctx, false, true));
        assert!(!asked_open(&ctx, "Size"), "the request is used up by the frame that drew it");

        let floats = egui::Context::default();
        install(&floats);
        crate::floating::float(&floats, "Size", pos2(600.0, 100.0));
        assert!(size_card_frame(&floats, true, true), "a floating card is drawn open");
        assert!(!asked_open(&floats, "Size"), "and that was the opening asked for");
    }
}
