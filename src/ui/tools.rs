//! The hosted tools' side of the interface (see `app/tools.rs`): their
//! buttons, the panel of the one that is up, their cards in the Scene panel,
//! the shapes they draw over the viewport, their hold-Space wheel, and the
//! list of background jobs.

use egui::{Align2, Color32, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use sculpt_toolkit::{Event, Label, Shape};

use super::theme::{self, ACCENT, ButtonStyle};
use super::{UiState, tip};
use crate::app::tools::Tools;
use crate::doc::Doc;

/// Tool buttons to a row, as in the row of the app's own tools above them.
const PER_ROW: usize = 3;

/// The groups the hosted tools' buttons are sorted into, by the tools' ids,
/// each under a heading that folds it away. A tool in none of them (one
/// added later, say) goes under the last.
const GROUPS: [(&str, &[&str]); 3] = [
    ("Make", &["tube", "muscle", "lathe", "array", "extract"]),
    ("Mask, Hide & Cut", &["mask", "hide", "trim", "split"]),
    ("Print & Measure", &["measure", "keys"]),
];

/// Which of `GROUPS` the tool `id` is under.
fn group_of(id: &str) -> usize {
    GROUPS.iter().position(|(_, ids)| ids.contains(&id)).unwrap_or(GROUPS.len() - 1)
}

/// A button per hosted tool, under the app's own tool buttons, in groups.
/// The group of the tool that is up stays open.
pub fn buttons(ui: &mut egui::Ui, doc: &mut Doc, state: &UiState) {
    let active = doc.tools.active(doc.tool);
    for (g, (title, _)) in GROUPS.iter().enumerate() {
        let members: Vec<usize> = (0..doc.tools.len()).filter(|&i| doc.tools.info(i).is_some_and(|info| group_of(info.id) == g)).collect();
        if members.is_empty() {
            continue;
        }
        let up = active.is_some_and(|i| members.contains(&i));
        if !theme::fold(ui, title, true, up) {
            continue;
        }
        // Four short names fit four to a row.
        let per_row = if members.len() == 4 { 4 } else { PER_ROW };
        for row in members.chunks(per_row) {
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, per_row, 6.0);
                for &index in row {
                    let Some(info) = doc.tools.info(index) else { continue };
                    let style = ButtonStyle {
                        icon: state.assets.icon(info.icon),
                        active: (active == Some(index)).then_some(ACCENT()),
                        // No hotkey badge: at a third of the panel it would sit
                        // on the name. The tooltip gives the key.
                        ..Default::default()
                    };
                    if tip(theme::button(ui, w, info.name, style), doc.opts.tooltips, info.tip).clicked() {
                        doc.tools.toggle(index, doc.tool);
                    }
                }
            });
        }
    }
}

/// The panel of the hosted tool that is up, in place of the brush settings.
/// False when none is up.
pub fn panel(ui: &mut egui::Ui, doc: &mut Doc, state: &UiState) -> bool {
    let Some(index) = doc.tools.active(doc.tool) else { return false };
    Tools::call(doc, &state.assets, index, |tool, cx| tool.panel(ui, cx));
    true
}

/// The cards tools keep in the Scene panel whichever tool is up.
pub fn cards(ui: &mut egui::Ui, doc: &mut Doc, state: &UiState) {
    for index in 0..doc.tools.len() {
        Tools::call(doc, &state.assets, index, |tool, cx| tool.card(ui, cx));
    }
}

/// What the tools drew over the viewport this frame, under the panels like
/// the model it sits on. Positions arrive in pixels, sizes in points.
pub fn overlay(ctx: &egui::Context, doc: &Doc) {
    let shapes = &doc.tools.draw.overlay.shapes;
    if shapes.is_empty() {
        return;
    }
    let ppp = ctx.pixels_per_point();
    let at = |p: &glam::Vec2| pos2(p.x / ppp, p.y / ppp);
    let p = ctx.layer_painter(LayerId::background());
    for shape in shapes {
        match shape {
            Shape::Line { points, width, color, closed, dash } => {
                let mut path: Vec<Pos2> = points.iter().map(at).collect();
                let stroke = Stroke::new(*width, *color);
                match dash {
                    Some((dash, gap)) => {
                        if *closed {
                            path.extend(path.first().copied());
                        }
                        p.extend(egui::Shape::dashed_line(&path, stroke, *dash, *gap));
                    }
                    None if *closed => {
                        p.add(egui::Shape::closed_line(path, stroke));
                    }
                    None => {
                        p.line(path, stroke);
                    }
                }
            }
            Shape::Dot { center, radius, fill, outline, square } => {
                let c = at(center);
                if *square {
                    let rect = Rect::from_center_size(c, Vec2::splat(radius * 2.0));
                    p.rect_filled(rect, 1.0, *fill);
                    p.rect_stroke(rect, 1.0, Stroke::new(1.5, *outline), StrokeKind::Outside);
                } else {
                    p.circle_filled(c, radius + 1.5, *outline);
                    p.circle_filled(c, *radius, *fill);
                }
            }
            Shape::Fill { points, color } => {
                p.add(egui::Shape::convex_polygon(points.iter().map(at).collect(), *color, Stroke::NONE));
            }
            Shape::Label(label) => paint_label(&p, label, at(&label.at)),
        }
    }
}

/// Text on a rounded plate, pushed off `at` far enough to clear it.
fn paint_label(p: &egui::Painter, label: &Label, at: Pos2) {
    let mut job = egui::text::LayoutJob { halign: egui::Align::Center, ..Default::default() };
    for run in &label.runs {
        let font = if run.bold { theme::bold(run.size) } else { theme::font(run.size) };
        job.append(&run.text, 0.0, egui::TextFormat::simple(font, run.color));
    }
    let galley = p.layout_job(job);
    let size = galley.size() + vec2(12.0, 6.0);
    let push = label.push.normalize_or_zero();
    let reach = if push == glam::Vec2::ZERO { 0.0 } else { push.x.abs() * size.x * 0.5 + push.y.abs() * size.y * 0.5 + label.gap };
    let rect = Rect::from_center_size(at + vec2(push.x, push.y) * reach, size);
    p.rect_filled(rect, 4.0, label.background);
    // A centred layout is placed by the middle of its top edge.
    p.galley(pos2(rect.center().x, rect.top() + 3.0), galley, Color32::WHITE);
}

/// The hold-Space wheel of the hosted tool that is up. Tiles are clicked;
/// letting go of Space closes it.
pub fn radial(ctx: &egui::Context, doc: &mut Doc, state: &UiState) {
    let Some(anchor) = doc.tools.radial else { return };
    let (Some(index), tiles) = (doc.tools.active(doc.tool), doc.tools.radial_items(doc.tool)) else {
        doc.tools.radial = None;
        return;
    };
    if tiles.is_empty() {
        doc.tools.radial = None;
        return;
    }
    let screen = ctx.content_rect();
    let center = super::overlay::wheel_center(anchor, 140.0, screen);
    let open = ctx.animate_bool_with_time(Id::new("tool_radial_open"), true, 0.11);
    let current = tiles.iter().find(|t| t.on).map_or(doc.tools.info(index).map_or("", |info| info.name), |t| t.label);
    let mut clicked = None;
    egui::Area::new(Id::new("tool_radial")).order(Order::Foreground).fixed_pos(center - Vec2::splat(140.0)).show(ctx, |ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(280.0), Sense::click());
        let p = ui.painter();
        let scale = 0.85 + 0.15 * open;
        let fade = |c: Color32| c.gamma_multiply(open);
        let c = rect.center();
        p.circle_filled(c, 134.0 * scale, fade(theme::wheel_disc()));
        p.circle_stroke(c, 134.0 * scale, Stroke::new(1.0, fade(Color32::from_white_alpha(18))));
        let pointer = response.hover_pos();
        let mut hovered_name = None;
        for (i, tile) in tiles.iter().enumerate() {
            let a = i as f32 / tiles.len() as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
            let at = c + vec2(a.cos(), a.sin()) * 92.0 * scale;
            let hovered = pointer.is_some_and(|q| q.distance(at) < 34.0);
            let size = 58.0 * scale * if hovered { 1.1 } else { 1.0 };
            let fill = if hovered {
                theme::lit(ACCENT())
            } else if tile.on {
                ACCENT()
            } else {
                theme::HEADER()
            };
            p.rect_filled(Rect::from_center_size(at, Vec2::splat(size)), 11.0, fade(fill));
            if let Some(id) = state.assets.icon(tile.icon) {
                p.image(
                    id,
                    Rect::from_center_size(at - vec2(0.0, 7.0), Vec2::splat(28.0)),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    fade(Color32::WHITE),
                );
            }
            p.text(at + vec2(0.0, 18.0), Align2::CENTER_CENTER, tile.label, theme::bold(10.0), fade(theme::TEXT()));
            if hovered {
                hovered_name = Some(tile.label);
                if response.clicked() {
                    clicked = Some(i);
                }
            }
        }
        p.circle_filled(c, 40.0 * scale, fade(theme::CARD()));
        p.text(c, Align2::CENTER_CENTER, hovered_name.unwrap_or(current), theme::bold(14.0), fade(theme::TEXT()));
    });
    if let Some(i) = clicked {
        Tools::call(doc, &state.assets, index, |tool, cx| tool.input(cx, Event::Radial(i)));
        doc.tools.radial = None;
    }
}

/// The background jobs that are running, each with what it says it is doing
/// and a way to stop it. Shown whichever tool is up: a job outlives a tool
/// switch.
pub fn jobs(ctx: &egui::Context, doc: &Doc) {
    let running = doc.tools.jobs.running();
    if running.is_empty() {
        return;
    }
    egui::Area::new(Id::new("tool_jobs"))
        .order(Order::Middle)
        .fade_in(false)
        // Clear of the toast, which rises through the space below.
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -112.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::PANEL())
                .corner_radius(8.0)
                .stroke(theme::hairline())
                .inner_margin(egui::Margin { left: 10, right: 10, top: 8, bottom: 8 })
                .show(ui, |ui| {
                    for (i, job) in running.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(14.0).color(ACCENT()));
                            let text = if job.progress.is_empty() {
                                format!("{}...", job.label)
                            } else {
                                format!("{}: {}", job.label, job.progress)
                            };
                            ui.label(egui::RichText::new(text).font(theme::font(12.0)).color(theme::TEXT()));
                            let style = ButtonStyle { height: 22.0, ..Default::default() };
                            if theme::button(ui, 60.0, "Cancel", style).clicked() {
                                doc.tools.jobs.cancel(i);
                            }
                        });
                    }
                });
        });
}
