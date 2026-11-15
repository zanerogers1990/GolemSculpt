//! The brush's keys, as the desktop has them by default (`keymap.rs`):
//!
//! - **Space held**: the brush wheel, a ring of brushes round the pointer
//!   (the desktop's `ui/overlay.rs`, `radial_menu`). A tap on a tile takes
//!   that brush up; letting go of Space over a tile takes it up too, and
//!   anywhere else just closes the wheel. Under it, a bar for the brush's
//!   strength.
//! - **1 to 0**: the base brushes, each on the number its kernel ships with
//!   (`Brush::hotkey`).
//! - **S, F, D held**: size, strength and focal shift, set by moving the
//!   pointer left and right while the key is down. F tapped twice frames the
//!   model.

use eframe::egui::{self, Align2, Color32, Id, Key, Order, Pos2, Rect, Sense, Stroke, Vec2 as EVec2, pos2, vec2};
use glam::Vec2;

use super::ui::theme;
use super::{Drag, GolemApp, Mode};
use crate::sculpt::Brush;
use crate::sculpt::library::{Library, Origin};

/// Two taps of F closer than this frame the model (seconds), as on the desktop.
const DOUBLE_TAP: f64 = 0.5;
/// The lowest and highest Strength a brush can have: the ends of the panel's
/// slider, of F + drag and of the bar under the wheel.
const STRENGTH_RANGE: std::ops::RangeInclusive<f32> = 0.01..=1.0;

#[derive(Clone, Copy, PartialEq)]
enum GaugeKind {
    Size,
    Strength,
    Focal,
}

impl GaugeKind {
    fn key(self) -> Key {
        match self {
            GaugeKind::Size => Key::S,
            GaugeKind::Strength => Key::F,
            GaugeKind::Focal => Key::D,
        }
    }
}

/// A hold key being dragged: what it sets, where the pointer was when the
/// key went down (physical pixels) and the value then. The ring stays there,
/// so it grows and shrinks in place.
#[derive(Clone, Copy)]
struct Gauge {
    kind: GaugeKind,
    anchor: Vec2,
    start: f32,
}

/// What the brush keys remember between frames.
#[derive(Default)]
pub struct BrushKeys {
    /// The wheel is open, round this point (egui points).
    wheel: Option<Pos2>,
    /// Space has been let go: the wheel closes once this frame's presses
    /// have been seen, a tap on a tile among them.
    wheel_closing: bool,
    /// The wheel was up at the start of this frame: a press this frame is
    /// the wheel's, not a stroke's.
    pub wheel_up: bool,
    gauge: Option<Gauge>,
    last_f: Option<f64>,
}

impl BrushKeys {
    /// Whether a hold key is setting a value: the pointer's moves are its.
    pub fn gauging(&self) -> bool {
        self.gauge.is_some()
    }
}

/// How a wheel of `tiles` tiles is laid out, in points (the desktop's
/// `WheelLayout::brushes`).
struct WheelLayout {
    half: f32,
    disc: f32,
    ring: f32,
    tile: f32,
    pick: f32,
}

impl WheelLayout {
    fn brushes(tiles: usize) -> WheelLayout {
        if tiles <= 9 {
            return WheelLayout { half: 152.0, disc: 146.0, ring: 104.0, tile: 54.0, pick: 31.0 };
        }
        const TILE: f32 = 48.0;
        const GAP: f32 = 6.0;
        // Neighbours on a ring are a chord apart: the tile and the gap.
        let ring = ((TILE + GAP) / (2.0 * (std::f32::consts::PI / tiles as f32).sin())).max(104.0).ceil();
        let half = ring + TILE * 0.5 + 24.0;
        WheelLayout { half, disc: half - 6.0, ring, tile: TILE, pick: (TILE + GAP) * 0.5 - 1.0 }
    }
}

/// The base brushes round the wheel, clockwise from the top: kin beside kin
/// (the desktop's order, less Pose, Erase Layer and Paint, which the web
/// version has not got).
const WHEEL_ORDER: [Brush; 11] = {
    use Brush::*;
    [Move, Clay, Standard, Layer, Smooth, Crease, Pinch, Inflate, Flatten, SnakeHook, Nudge]
};

/// The tiles of the brush wheel, clockwise from the top: every pinned brush
/// by its place in the library, and the mask tile (`None`) at the bottom
/// (the desktop's `brushes::wheel_tiles`).
fn wheel_tiles(library: &Library) -> Vec<Option<usize>> {
    let entries = library.entries();
    let of_kernel = |kernel: Brush| {
        let pinned = move |&i: &usize| entries[i].pinned && entries[i].kernel == kernel && entries[i].origin != Origin::User;
        (0..entries.len()).filter(pinned).map(Some)
    };
    let mut tiles: Vec<Option<usize>> = WHEEL_ORDER.into_iter().flat_map(of_kernel).collect();
    // The bottom of the wheel: half-way round, on the side Inflate was on.
    tiles.insert(((tiles.len() + 2) / 2).min(tiles.len()), None);
    tiles
}

/// A brush's name as its first two initials, or its first two letters if it
/// is one word: what goes in the corner of its wheel tile.
fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().unwrap_or_default();
    match words.next() {
        Some(second) => first.chars().take(1).chain(second.chars().take(1)).flat_map(char::to_uppercase).collect(),
        None => {
            let mut letters = first.chars();
            letters.next().into_iter().flat_map(char::to_uppercase).chain(letters.next()).collect()
        }
    }
}

/// `name`, or as much of its start as fits in `letters` with a mark that
/// there was more.
fn shortened(name: &str, letters: usize) -> String {
    if name.chars().count() <= letters {
        return name.to_owned();
    }
    let kept: String = name.chars().take(letters.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// Where a wheel `half` points each way from its centre opens for a pointer
/// at `anchor`: round the pointer, moved in as far as it takes for all of
/// it to be on `screen`, or in the middle of a screen too small for it.
fn wheel_center(anchor: Pos2, half: f32, screen: Rect) -> Pos2 {
    let middle = |low: f32, high: f32| (low + high) * 0.5;
    let fit = |v: f32, low: f32, high: f32| if low <= high && v.is_finite() { v.max(low).min(high) } else { middle(low, high) };
    pos2(fit(anchor.x, screen.left() + half, screen.right() - half), fit(anchor.y, screen.top() + half, screen.bottom() - half))
}

/// Where the strength bar under the wheel sits: under it, or above it near
/// the bottom of the window, or beside it; `None` where none of those fits.
fn wheel_bar_rect(center: Pos2, radius: f32, screen: Rect) -> Option<Rect> {
    const BAR_SIZE: EVec2 = vec2(236.0, theme::SLIDER_HEIGHT + 14.0);
    const BAR_GAP: f32 = 6.0;
    let wheel = Rect::from_center_size(center, EVec2::splat(2.0 * radius));
    let across = center.x - BAR_SIZE.x * 0.5;
    let down = center.y - BAR_SIZE.y * 0.5;
    let places = [
        pos2(across, wheel.bottom() + BAR_GAP),
        pos2(across, wheel.top() - BAR_GAP - BAR_SIZE.y),
        pos2(wheel.right() + BAR_GAP, down),
        pos2(wheel.left() - BAR_GAP - BAR_SIZE.x, down),
    ];
    places.into_iter().map(|at| Rect::from_min_size(at, BAR_SIZE)).find(|bar| screen.contains_rect(*bar))
}

impl GolemApp {
    /// The wheel for this frame, and the end of a hold key's drag. Runs
    /// every frame, before anything else looks at the pointer.
    pub(super) fn brush_wheel(&mut self, ctx: &egui::Context) {
        let keys = &mut self.brush_keys;
        keys.wheel_up = keys.wheel.is_some();
        if let Some(g) = keys.gauge {
            if !ctx.input(|i| i.key_down(g.kind.key())) {
                keys.gauge = None;
            }
        }
        self.paint_gauge(ctx);
        let keys = &mut self.brush_keys;
        // A tool took the panel, or something else took the pointer: no wheel.
        if self.mode != Mode::Sculpt || self.drag != Drag::None {
            keys.wheel = None;
        }
        let Some(anchor) = self.brush_keys.wheel else {
            self.brush_keys.wheel_closing = false;
            return;
        };
        let released = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::Space, pressed: false, .. }))
                || !i.key_down(Key::Space)
        });
        if released {
            self.brush_keys.wheel_closing = true;
        }

        let screen = ctx.content_rect();
        let open = ctx.animate_bool_with_time(Id::new("radial_open"), true, 0.11);
        let (library, s) = (&self.sculptor.library, &self.sculptor.settings);
        let brushes = wheel_tiles(library);
        let in_hand = library.selected(s);
        let WheelLayout { half, disc, ring, tile: tile_size, pick } = WheelLayout::brushes(brushes.len());
        let center = wheel_center(anchor, half, screen);
        let lit = |tile: Option<usize>| match tile {
            Some(i) => i == in_hand && !s.mask_mode,
            None => s.mask_mode,
        };
        // Name, icon, lit, and for a brush that is not one of the base ones its initials.
        let tiles: Vec<(String, &'static str, bool, String)> = brushes
            .iter()
            .map(|&tile| match tile.and_then(|i| library.entries().get(i)) {
                Some(e) => {
                    let badge = if e.origin == Origin::Base { String::new() } else { initials(&e.name) };
                    (e.name.clone(), e.kernel.icon(), lit(tile), badge)
                }
                None => ("Mask".to_owned(), "brush_mask", lit(tile), String::new()),
            })
            .collect();
        let current = if s.mask_mode { "Mask".to_owned() } else { library.entries()[in_hand].name.clone() };
        // A wheel's tile is taken as it is pressed, not when egui would call
        // it a click: a pen's tap that slid a few points is no click.
        let pressed = ctx.input(|i| {
            i.events.iter().find_map(|e| match *e {
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } => Some(pos),
                _ => None,
            })
        });
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let icons = &self.ui.pictures;
        let mut clicked = None;
        let mut hovered_tile = None;
        egui::Area::new(Id::new("radial_menu")).order(Order::Foreground).fixed_pos(center - EVec2::splat(half)).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(EVec2::splat(2.0 * half), Sense::click());
            let p = ui.painter();
            let scale = 0.85 + 0.15 * open;
            let fade = |c: Color32| c.gamma_multiply(open);
            let c = rect.center();
            p.circle_filled(c, disc * scale, fade(theme::wheel_disc()));
            p.circle_stroke(c, disc * scale, Stroke::new(1.0, fade(Color32::from_white_alpha(18))));
            let mut hovered_name = None;
            for (i, (name, icon, active, badge)) in tiles.iter().enumerate() {
                let a = i as f32 / tiles.len() as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let at = c + vec2(a.cos(), a.sin()) * ring * scale;
                let on_tile = |q: Pos2| q.distance(at) < pick;
                let hovered = pointer.is_some_and(on_tile);
                let size = tile_size * scale * if hovered { 1.1 } else { 1.0 };
                let tile = Rect::from_center_size(at, EVec2::splat(size));
                let fill = if hovered {
                    theme::lit(theme::ACCENT())
                } else if *active {
                    theme::ACCENT()
                } else {
                    theme::HEADER()
                };
                p.rect_filled(tile, 11.0, fade(fill));
                // The picture and the name under it, in the tile's own
                // proportions (28 and 10 points on a tile of 54).
                let k = tile_size / 54.0;
                if let Some(id) = icons.icon(icon) {
                    p.image(
                        id,
                        Rect::from_center_size(at - vec2(0.0, 7.0 * k), EVec2::splat(28.0 * k)),
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        fade(Color32::WHITE),
                    );
                }
                let letters = if name.len() > 8 { 8.0 } else { 10.0 };
                let font = theme::bold(letters * k.max(0.95));
                let fits = |text: &str| p.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x <= tile_size * scale - 4.0;
                let count = name.chars().count();
                let shown = (0..=count).rev().map(|n| shortened(name, n)).find(|text| fits(text)).unwrap_or_default();
                p.text(at + vec2(0.0, 18.0 * k), Align2::CENTER_CENTER, shown, font, fade(theme::TEXT()));
                if !badge.is_empty() {
                    let corner = tile.right_top() + vec2(-4.0, 3.0);
                    p.text(corner, Align2::RIGHT_TOP, badge, theme::bold(9.0 * k.max(0.95)), fade(theme::KEY));
                }
                if hovered {
                    hovered_name = Some(name.as_str());
                    hovered_tile = Some(i);
                }
                if pressed.is_some_and(on_tile) {
                    clicked = Some(i);
                }
            }
            p.circle_filled(c, 40.0 * scale, fade(theme::CARD()));
            p.text(c, Align2::CENTER_CENTER, hovered_name.unwrap_or(&current), theme::bold(14.0), fade(theme::TEXT()));
        });

        // The Strength bar under the wheel: the brush in hand's strength, the
        // same value as the panel's slider and F + drag.
        if let Some(plate) = wheel_bar_rect(center, half, screen) {
            let name = if s.mask_mode { "Mask".to_owned() } else { library.entries()[in_hand].name.clone() };
            let label = format!("{name} Strength");
            let strength = &mut self.sculptor.settings.current_mut().strength;
            egui::Area::new(Id::new("radial_strength")).order(Order::Foreground).fixed_pos(plate.min).show(ctx, |ui| {
                ui.multiply_opacity(open);
                let (rect, response) = ui.allocate_exact_size(plate.size(), Sense::click_and_drag());
                ui.painter().rect_filled(rect, 10.0, theme::CARD());
                let track = Rect::from_min_max(rect.min + vec2(12.0, 6.0), rect.max - vec2(12.0, 8.0));
                theme::slider_in(ui, track, response, &label, strength, STRENGTH_RANGE, |v| format!("{:.0}%", v * 100.0));
            });
        }

        // Letting go of Space over a tile takes that brush, as a tap on it does.
        let closing = std::mem::take(&mut self.brush_keys.wheel_closing);
        let chosen = clicked.or(if closing { hovered_tile } else { None });
        if let Some(i) = chosen {
            let (library, s) = (&mut self.sculptor.library, &mut self.sculptor.settings);
            match brushes[i] {
                Some(brush) => {
                    library.select(brush, s);
                    s.mask_mode = false;
                }
                None => s.mask_mode = true,
            }
            self.brush_keys.wheel = None;
        }
        if closing {
            self.brush_keys.wheel = None;
        }
        ctx.request_repaint();
    }

    /// The brush's keys: Space for the wheel, the number keys, and S, F and
    /// D held. Not while a text box has the keyboard (the caller sees to it).
    pub(super) fn brush_keys(&mut self, ctx: &egui::Context) {
        if self.mode != Mode::Sculpt {
            return;
        }
        let idle = self.drag == Drag::None && !self.sculptor.is_active();
        let (now, pointer) = ctx.input(|i| (i.time, i.pointer.latest_pos()));
        let ppp = ctx.pixels_per_point();
        let pressed: Vec<Key> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match *e {
                    egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } if modifiers.is_none() => Some(key),
                    _ => None,
                })
                .collect()
        });
        for key in pressed {
            match key {
                Key::Space if idle && (self.brush_keys.wheel.is_none() || self.brush_keys.wheel_closing) => {
                    let at = pointer.unwrap_or_else(|| ctx.content_rect().center());
                    self.brush_keys.wheel = Some(at);
                    self.brush_keys.wheel_closing = false;
                    self.brush_keys.gauge = None;
                }
                Key::S | Key::F | Key::D if idle && self.brush_keys.wheel.is_none() => {
                    let kind = match key {
                        Key::S => GaugeKind::Size,
                        Key::F => GaugeKind::Strength,
                        _ => GaugeKind::Focal,
                    };
                    if kind == GaugeKind::Strength {
                        // Tapped twice it frames the model; held and dragged it is strength.
                        if self.brush_keys.last_f.is_some_and(|t| now - t < DOUBLE_TAP) {
                            self.brush_keys.last_f = None;
                            self.brush_keys.gauge = None;
                            self.frame_model(true);
                            continue;
                        }
                        self.brush_keys.last_f = Some(now);
                    }
                    let s = &self.sculptor.settings;
                    let start = match kind {
                        GaugeKind::Size => s.size(),
                        GaugeKind::Strength => s.current().strength,
                        GaugeKind::Focal => s.current().focal_shift,
                    };
                    let anchor = pointer.map_or(Vec2::ZERO, |p| Vec2::new(p.x, p.y) * ppp);
                    self.brush_keys.gauge = Some(Gauge { kind, anchor, start });
                }
                _ if idle => {
                    if let Some(digit) = digit_of(key) {
                        self.pick_brush_by_number(digit);
                    }
                }
                _ => {}
            }
        }
    }

    /// Takes up the base brush whose kernel ships on number key `digit`.
    fn pick_brush_by_number(&mut self, digit: char) {
        let Some(kernel) = WHEEL_ORDER.into_iter().find(|b| b.hotkey() == Some(digit)) else { return };
        let (library, s) = (&mut self.sculptor.library, &mut self.sculptor.settings);
        let i = Library::base(kernel);
        if i >= library.entries().len() {
            return;
        }
        library.select(i, s);
        s.mask_mode = false;
        self.status = format!("{} brush.", library.entries()[i].name);
    }

    /// The pointer moved to `px` (physical pixels) while a hold key is down:
    /// the value it sets follows how far right of where the key went down
    /// the pointer is. False when no key is held.
    pub(super) fn gauge_moved(&mut self, px: Vec2) -> bool {
        let Some(g) = self.brush_keys.gauge else { return false };
        let travel = px.x - g.anchor.x;
        let viewport_h = self.view.size.y;
        let s = &mut self.sculptor.settings;
        match g.kind {
            GaugeKind::Size => s.set_size(g.start + travel * s.size_per_drag_px(), viewport_h),
            GaugeKind::Strength => s.current_mut().strength = (g.start + travel * 0.00125).clamp(0.01, 1.0),
            GaugeKind::Focal => s.current_mut().focal_shift = (g.start - travel * 0.0025).clamp(-1.0, 1.0),
        }
        // Moved: it was not the first tap of two.
        if g.kind == GaugeKind::Strength && travel.abs() > 20.0 {
            self.brush_keys.last_f = None;
        }
        true
    }

    /// While a hold key is down: the ring where the key went down, at the
    /// brush's size, with the strength as a red disc in it, and what is
    /// being set under it ("Size 120 px"), as the desktop's cursor shows them.
    pub(super) fn paint_gauge(&self, ctx: &egui::Context) {
        let Some(g) = self.brush_keys.gauge else { return };
        let ppp = ctx.pixels_per_point();
        let s = &self.sculptor.settings;
        let centre = pos2(g.anchor.x / ppp, g.anchor.y / ppp);
        let r = (s.radius_px / ppp).max(7.0);
        let p = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("brush_gauge")));
        if g.kind == GaugeKind::Strength {
            let t = ((s.current().strength - 0.01) / 0.99).clamp(0.0, 1.0);
            p.circle_filled(centre, (r * t).max(2.0), Color32::from_rgba_unmultiplied(255, 77, 77, 115));
        }
        p.circle_stroke(centre, r, Stroke::new(4.0, Color32::from_black_alpha(110)));
        let width = if g.kind == GaugeKind::Focal { 3.0 } else { 1.75 };
        p.circle_stroke(centre, r, Stroke::new(width, Color32::from_rgb(235, 235, 235)));
        let text = match g.kind {
            GaugeKind::Size if s.screen_space_size => format!("Size {:.0} px", s.radius_px),
            GaugeKind::Size => format!("Size {:.3}", s.radius_world),
            GaugeKind::Strength => format!("Strength {:.0}%", s.current().strength * 100.0),
            GaugeKind::Focal => format!("Focal Shift {:.0}", s.current().focal_shift * 100.0),
        };
        let at = centre + vec2(0.0, r + 14.0);
        p.text(at + vec2(0.0, 1.0), Align2::CENTER_CENTER, &text, theme::bold(13.0), Color32::from_black_alpha(170));
        p.text(at, Align2::CENTER_CENTER, &text, theme::bold(13.0), Color32::WHITE);
    }
}

/// The number on a number key.
fn digit_of(key: Key) -> Option<char> {
    Some(match key {
        Key::Num0 => '0',
        Key::Num1 => '1',
        Key::Num2 => '2',
        Key::Num3 => '3',
        Key::Num4 => '4',
        Key::Num5 => '5',
        Key::Num6 => '6',
        Key::Num7 => '7',
        Key::Num8 => '8',
        Key::Num9 => '9',
        _ => return None,
    })
}
