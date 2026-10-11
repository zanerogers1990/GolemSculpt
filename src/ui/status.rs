//! The status line while the Scene panel, where it lives, is folded away.
//!
//! What the app last did, or could not do ("Could not save...", a Remesh
//! that was refused, what a tool's cut left behind), is written on one line
//! at the top of the Scene panel. With that panel folded away there is no
//! such line, and a failure would pass without a word. So while it is folded
//! the same words, in the same colours, are drawn on a small plate at the
//! top of the view: between the panels, short of the view gizmo, wrapped
//! when they run long.
//!
//! How long it stays depends on what it says. An error stays until something
//! else is written, the plate is tapped, or the panel comes back: someone
//! working with a pen who looks up some seconds later must still find it.
//! Anything else has done its job once it has been read, so it fades after
//! a few seconds, a longer line later than a short one. The plate is the
//! only part of the view that takes the pen, and only while the status line
//! is on it; a tap puts the line away.
//!
//! The Sculpt panel has two notes of the same kind, which join the plate
//! while that panel is folded away: dynamic topology stopping at its
//! triangle limit, and the SSphere tool's one-off results. They come and go
//! by themselves, so a plate with only those on it lets the pen through.

use std::sync::Arc;
use std::time::Duration;

use egui::{Color32, Id, Order, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use sculpt_toolkit::floating;

use super::panels::{self, Side};
use super::{UiState, dyntopo, theme};
use crate::doc::{Doc, Tool};

/// The plate's name among the interface's areas.
pub const AREA: &str = "status_line";

/// How far below the top of the window the plate sits.
const TOP: f32 = 10.0;
/// The widest the plate gets: a long message wraps inside this.
const WIDEST: f32 = 440.0;
/// The narrowest it is squeezed to before it is allowed over its neighbours.
const NARROWEST: f32 = 150.0;
/// The space kept between the plate and what is beside it.
const GAP: f32 = 8.0;
/// Between the plate's edge and its words, and between one line and the next.
const PAD: Vec2 = vec2(10.0, 7.0);
const LINE_GAP: f32 = 4.0;
/// Room at the plate's right end for the cross that says it can be tapped away.
const CROSS: f32 = 18.0;

/// Seconds a line that is not an error stays up: this long, and this much
/// longer for each letter (about the pace of reading), but never longer
/// than `READ_LONGEST`. Then it takes `FADE` seconds to go.
const READ: f64 = 6.0;
const READ_PER_LETTER: f64 = 0.05;
const READ_LONGEST: f64 = 30.0;
const FADE: f64 = 1.5;

/// When the status line was last written, and whether its copy over the
/// view has been tapped away since.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Seen {
    /// In seconds since the app started.
    pub since: f64,
    pub put_away: bool,
}

impl Seen {
    /// For a line written at `time`: it has not been put away.
    pub fn at(time: f64) -> Seen {
        Seen { since: time, put_away: false }
    }
}

/// How many seconds a line of `letters` letters that is not an error stays
/// up before it starts to fade.
fn reading_time(letters: usize) -> f64 {
    (READ + READ_PER_LETTER * letters as f64).min(READ_LONGEST)
}

/// How strongly a status line of `letters` letters shows `age` seconds after
/// it was written: 1 while it is up, falling to 0 as it fades. An error
/// never fades.
fn strength(error: bool, letters: usize, age: f64) -> f32 {
    if error {
        return 1.0;
    }
    let left = (reading_time(letters) + FADE - age) / FADE;
    if left.is_nan() { 0.0 } else { left.max(0.0).min(1.0) as f32 }
}

/// One line of the plate: its words, their colour and how strongly they show.
struct Line {
    text: String,
    color: Color32,
    strength: f32,
}

/// The stretch of the window's width the plate may use, left end and right:
/// from the Sculpt panel's tab (the panel's own, or the one at the screen's
/// edge when it is folded away) to the view gizmo, with a gap at each end.
/// The right panel and its tab are beyond the gizmo.
fn room(screen: Rect, folded: [bool; 2]) -> (f32, f32) {
    let tab = panels::tab_rect(Side::Left, folded[Side::Left.index()], screen);
    let left = panels::tab_reach(Side::Left, tab).right() + GAP;
    let right = panels::gizmo_origin(screen, folded[Side::Right.index()]).x - GAP;
    (left, right)
}

/// How wide the plate may be on this window: all of its room up to
/// `WIDEST`, and never under `NARROWEST`, even where there is no room at all.
fn widest(screen: Rect, folded: [bool; 2]) -> f32 {
    let (left, right) = room(screen, folded);
    // `max` last: when there is no room the low end wins.
    (right - left).min(WIDEST).max(NARROWEST)
}

/// Where a plate `size` big goes: at the top of the window, centred on it
/// (clear of where cards pulled out of either panel first appear), then
/// moved as far as it must be to stay inside its room. Where the room is
/// too small it keeps to the left end and runs over the gizmo, and it never
/// starts off the window.
fn plate_rect(size: Vec2, screen: Rect, folded: [bool; 2]) -> Rect {
    let (left, right) = room(screen, folded);
    // `max` last each time, so the low end wins where the two cross.
    let x = (screen.center().x - size.x * 0.5).min(right - size.x).max(left);
    let x = x.min(screen.right() - size.x).max(screen.left());
    Rect::from_min_size(pos2(x, screen.top() + TOP), size)
}

/// The notes only the Sculpt panel shows that matter while it is folded
/// away. A card that floats still shows its own, so that one is left out.
fn sculpt_panel_notes(ctx: &egui::Context, doc: &Doc) -> Vec<Line> {
    let away = |card: &str| !floating::is_floating(ctx, card);
    let note = |text: &str, color| Line { text: text.to_owned(), color, strength: 1.0 };
    if doc.tool == Tool::SSpheres {
        return doc.spheres.note().filter(|_| away("Spheres")).map(|text| note(text, theme::OK)).into_iter().collect();
    }
    // The Remesh card is in the panel unless a hosted tool's panel has its place.
    let remesh_card = doc.tools.active(doc.tool).is_none() && away("Remesh");
    let limit = dyntopo::limit_note(&doc.sculptor.dyntopo).filter(|_| remesh_card);
    limit.map(|text| note(&text, dyntopo::WARNING)).into_iter().collect()
}

/// The status line as the plate shows it `age` seconds after it was written,
/// if it still shows, and how long until it next looks different: nothing
/// for a line that stays as it is.
fn status_line(status: &(String, Color32), seen: Seen, time: f64) -> Option<(Line, Option<Duration>)> {
    let (text, color) = status;
    if seen.put_away || text.trim().is_empty() {
        return None;
    }
    let error = *color == theme::ERROR;
    let letters = text.chars().count();
    let age = time - seen.since;
    let strength = strength(error, letters, age);
    if strength <= 0.0 {
        return None;
    }
    // Until the fade starts, then every frame while it lasts.
    let wait = (reading_time(letters) - age).max(0.0);
    let next = (!error).then(|| Duration::try_from_secs_f64(wait).unwrap_or_default());
    Some((Line { text: text.clone(), color: *color, strength }, next))
}

/// Draws the plate, when there is anything for it. For a frame in which the
/// panels show (not the clean view); `folded` is which of them are away.
pub fn overlay(ctx: &egui::Context, doc: &Doc, state: &mut UiState, folded: [bool; 2], time: f64) {
    let mut lines = Vec::new();
    let mut status = false;
    if folded[Side::Right.index()] {
        if let Some((line, next)) = status_line(&state.status, state.status_seen, time) {
            lines.push(line);
            status = true;
            if let Some(wait) = next {
                ctx.request_repaint_after(wait);
            }
        }
    }
    if folded[Side::Left.index()] {
        lines.extend(sculpt_panel_notes(ctx, doc));
    }
    if !lines.is_empty() && show(ctx, &lines, status, folded, doc.opts.tooltips) {
        state.status_seen.put_away = true;
        ctx.request_repaint();
    }
}

/// The plate with `lines` on it. With `dismissable` (the status line is on
/// it) it takes the pen and wears a small cross; true when it was tapped.
fn show(ctx: &egui::Context, lines: &[Line], dismissable: bool, folded: [bool; 2], tips: bool) -> bool {
    let screen = ctx.content_rect();
    let cross = if dismissable { CROSS } else { 0.0 };
    let wrap = widest(screen, folded) - 2.0 * PAD.x - cross;
    let galleys: Vec<Arc<egui::Galley>> = lines
        .iter()
        .map(|line| {
            let ink = line.color.gamma_multiply(line.strength);
            let job = egui::text::LayoutJob::simple(line.text.clone(), theme::font(12.0), ink, wrap);
            ctx.fonts_mut(|fonts| fonts.layout_job(job))
        })
        .collect();
    let words = galleys.iter().fold(Vec2::ZERO, |all, g| vec2(all.x.max(g.size().x), all.y + g.size().y));
    let size = words + 2.0 * PAD + vec2(cross, LINE_GAP * (galleys.len() as f32 - 1.0));
    let at = plate_rect(size, screen, folded).min;
    let strongest = lines.iter().map(|line| line.strength).fold(0.0, f32::max);
    let mut tapped = false;
    let area = egui::Area::new(Id::new(AREA)).order(Order::Foreground).fade_in(false).interactable(dismissable);
    area.fixed_pos(at).show(ctx, |ui| {
        let (rect, response) = ui.allocate_exact_size(size, if dismissable { Sense::click() } else { Sense::hover() });
        let p = ui.painter();
        p.rect_filled(rect, 7.0, theme::PANEL.gamma_multiply(strongest));
        let edge = Stroke::new(1.0, Color32::from_white_alpha(18).gamma_multiply(strongest));
        p.rect_stroke(rect, 7.0, edge, StrokeKind::Inside);
        let mut y = rect.top() + PAD.y;
        for galley in &galleys {
            p.galley(pos2(rect.left() + PAD.x, y), galley.clone(), theme::TEXT);
            y += galley.size().y + LINE_GAP;
        }
        if dismissable {
            let ink = Color32::from_white_alpha(if response.hovered() { 230 } else { 120 }).gamma_multiply(strongest);
            let c = pos2(rect.right() - PAD.x - 3.0, rect.top() + PAD.y + 8.0);
            for across in [vec2(3.5, 3.5), vec2(3.5, -3.5)] {
                p.line_segment([c - across, c + across], Stroke::new(1.5, ink));
            }
            tapped = response.clicked();
            super::tip(response, tips, "Tap to put this message away. It stays on the status line of the Scene panel.");
        }
    });
    tapped
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton, Pos2};

    use super::*;

    fn screen(width: f32, height: f32) -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(width, height))
    }

    const ERROR: &str = "Could not save Dragon.sclp: the disk is full. Nothing was written, and the file from before is \
                         untouched. Free some space, or use Save As to put it somewhere else.";

    fn line(text: &str, color: Color32) -> Line {
        Line { text: text.to_owned(), color, strength: 1.0 }
    }

    #[test]
    fn an_error_stays_up_and_anything_else_fades_once_it_has_been_read() {
        for age in [0.0, 5.0, 60.0, 86_400.0, f64::INFINITY, f64::NAN] {
            assert_eq!(strength(true, 40, age), 1.0, "an error, {age} s on");
        }
        // "Saved Dragon.sclp" is 17 letters: up for 6.85 s, gone 1.5 s later.
        assert_eq!(strength(false, 17, 0.0), 1.0);
        assert_eq!(strength(false, 17, 6.8), 1.0);
        assert!((strength(false, 17, 6.85 + 0.75) - 0.5).abs() < 1e-3, "half way through the fade");
        assert_eq!(strength(false, 17, 6.85 + 1.5), 0.0);
        assert_eq!(strength(false, 17, 3600.0), 0.0);
        // A long one is given longer, to a limit.
        assert_eq!(strength(false, 200, 15.9), 1.0);
        assert_eq!(strength(false, 200, 18.0), 0.0);
        assert_eq!(strength(false, 100_000, 29.9), 1.0);
        assert_eq!(strength(false, 100_000, 32.0), 0.0);
        // A clock that makes no sense shows nothing rather than something for ever.
        assert_eq!(strength(false, 17, f64::NAN), 0.0);
        assert_eq!(strength(false, 17, -5.0), 1.0);
    }

    /// What the plate does with the status line as the app keeps it: errors
    /// by their colour, the same error written twice comes back after it was
    /// put away, and only a line that will change asks for another frame.
    #[test]
    fn the_status_line_shows_until_it_is_put_away_or_has_faded() {
        let error = (ERROR.to_owned(), theme::ERROR);
        let saved = ("Saved Dragon.sclp".to_owned(), theme::OK);
        let shown = |status: &(String, Color32), seen: Seen, time: f64| {
            status_line(status, seen, time).map(|(line, next)| (line.strength, next))
        };
        assert_eq!(shown(&error, Seen::at(2.0), 3.0), Some((1.0, None)));
        assert_eq!(shown(&error, Seen::at(2.0), 9_000.0), Some((1.0, None)), "an error waits to be read");
        assert_eq!(shown(&error, Seen { since: 2.0, put_away: true }, 3.0), None, "until it is tapped away");
        assert_eq!(shown(&error, Seen::at(40.0), 41.0), Some((1.0, None)), "written again, it is back");

        let (strength, next) = shown(&saved, Seen::at(10.0), 12.0).unwrap();
        assert_eq!(strength, 1.0);
        assert!((next.unwrap().as_secs_f64() - 4.85).abs() < 1e-6, "a frame is asked for when the fade starts");
        let (strength, next) = shown(&saved, Seen::at(10.0), 17.6).unwrap();
        assert!(strength > 0.0 && strength < 1.0 && next == Some(Duration::ZERO), "and every frame while it lasts");
        assert_eq!(shown(&saved, Seen::at(10.0), 30.0), None);
        // The hint the app starts with is not an error either; nothing to say is nothing to show.
        assert_eq!(shown(&("Import adds a model.".to_owned(), theme::HINT), Seen::default(), 600.0), None);
        assert_eq!(shown(&("  ".to_owned(), theme::ERROR), Seen::at(0.0), 1.0), None);
    }

    /// The window sizes the fold tabs are checked at, in the interface's
    /// points: a 4K screen at 100% down to a 1280 x 720 window at 200%.
    const WINDOWS: [(f32, f32); 5] = [(3840.0, 2160.0), (1920.0, 1080.0), (1600.0, 960.0), (1066.0, 640.0), (640.0, 360.0)];

    /// With either panel folded away, a plate with a three-sentence message
    /// on it is inside the window, off the view gizmo, both fold tabs, the
    /// bottom bar and any panel that is showing.
    #[test]
    fn the_plate_keeps_clear_of_the_gizmo_the_tabs_the_bottom_bar_and_the_panels() {
        for (width, height) in WINDOWS {
            let s = screen(width, height);
            for folded in [[true, false], [false, true], [true, true]] {
                let what = format!("{width} x {height}, folded {folded:?}");
                let wide = widest(s, folded);
                assert!((NARROWEST..=WIDEST).contains(&wide), "{what}: {wide}");
                // As tall as six lines of it.
                let plate = plate_rect(vec2(wide, 110.0), s, folded);
                assert!(s.contains_rect(plate), "{what}: off the window, {plate:?}");
                let gizmo = Rect::from_min_size(panels::gizmo_origin(s, folded[1]), panels::GIZMO_SIZE);
                assert!(!plate.intersects(gizmo), "{what}: on the gizmo");
                for side in [Side::Left, Side::Right] {
                    let tab = panels::tab_reach(side, panels::tab_rect(side, folded[side.index()], s));
                    assert!(!plate.intersects(tab), "{what}: on the {side:?} tab");
                }
                let bar = Rect::from_min_size(panels::bottom_bar_origin(s, folded[0]), vec2(330.0, 22.0));
                assert!(!plate.intersects(bar), "{what}: on the bottom bar");
                let left_panel = Rect::from_min_max(s.min, pos2(panels::MARGIN + panels::LEFT_WIDTH, s.bottom()));
                let right_panel = Rect::from_min_max(pos2(s.right() - panels::MARGIN - panels::RIGHT_WIDTH, s.top()), s.max);
                assert!(folded[0] || !plate.intersects(left_panel), "{what}: on the Sculpt panel");
                assert!(folded[1] || !plate.intersects(right_panel), "{what}: on the Scene panel");
            }
        }
        // On a roomy window it is in the middle, where no pulled-out card first appears.
        let plate = plate_rect(vec2(WIDEST, 60.0), screen(1600.0, 960.0), [false, true]);
        assert_eq!(plate.center().x, 800.0);
        assert_eq!(plate.top(), TOP);
    }

    /// No window is too small to place it on, down to none at all (house
    /// rule 9: the lowest input), and none makes it narrower than can be read.
    #[test]
    fn the_plate_has_a_place_on_a_window_of_any_size() {
        for (width, height) in [(0.0, 0.0), (1.0, 1.0), (40.0, 900.0), (150.0, 20.0), (303.0, 303.0), (500.0, 300.0)] {
            let s = Rect::from_min_size(pos2(5.0, 7.0), vec2(width, height));
            for folded in [[false, false], [true, false], [false, true], [true, true]] {
                let what = format!("{width} x {height}, folded {folded:?}");
                let wide = widest(s, folded);
                assert!((NARROWEST..=WIDEST).contains(&wide), "{what}: {wide}");
                let plate = plate_rect(vec2(wide, 40.0), s, folded);
                assert!(plate.is_finite() && plate.size() == vec2(wide, 40.0), "{what}: {plate:?}");
                assert!(plate.left() >= s.left() && plate.top() == s.top() + TOP, "{what}: {plate:?}");
            }
        }
    }

    /// One frame of an interface that is nothing but the plate.
    fn frame(ctx: &egui::Context, events: Vec<Event>, lines: &[Line], dismissable: bool) -> bool {
        let mut tapped = false;
        let input = egui::RawInput { screen_rect: Some(screen(1600.0, 960.0)), events, ..Default::default() };
        ctx.run_ui(input, |root| tapped = show(root.ctx(), lines, dismissable, [false, true], false))
            .drop_without_applying_deltas();
        tapped
    }

    fn pen(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() }
    }

    fn plate(ctx: &egui::Context) -> Option<Rect> {
        let id = Id::new(AREA);
        ctx.memory(|m| m.areas().is_visible(&egui::LayerId::new(Order::Foreground, id)).then(|| m.area_rect(id)).flatten())
    }

    /// A long message wraps inside the plate instead of running across the
    /// view, the pen is the plate's only on the plate, and a tap on it is
    /// reported (it puts the line away).
    #[test]
    fn a_long_message_wraps_and_a_tap_on_the_plate_is_the_only_pen_it_takes() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let short = [line("Saved Dragon.sclp", theme::OK)];
        frame(&ctx, vec![], &short, true);
        frame(&ctx, vec![], &short, true);
        let one_line = plate(&ctx).expect("the plate is up");
        assert!(one_line.width() < 200.0 && one_line.height() < 36.0, "it is no bigger than its words: {one_line:?}");

        let long = [line(ERROR, theme::ERROR)];
        frame(&ctx, vec![], &long, true);
        frame(&ctx, vec![], &long, true);
        let wrapped = plate(&ctx).expect("the plate is up");
        assert!(wrapped.width() <= WIDEST + 0.5, "{wrapped:?}");
        assert!(wrapped.height() > one_line.height() + 12.0, "three sentences take more than one line: {wrapped:?}");
        assert_eq!(wrapped.center().x.round(), 800.0);

        // What the app goes by to keep a press off the model: the layer under the pointer.
        let on_plate = |at: Pos2| ctx.layer_id_at(at).is_some_and(|layer| layer.id == Id::new(AREA));
        assert!(on_plate(wrapped.center()));
        let the_views = |at: Pos2| ctx.layer_id_at(at).is_none_or(|layer| layer.order == Order::Background);
        let (under, left_of) = (wrapped.center_bottom() + vec2(0.0, 12.0), wrapped.left_center() - vec2(12.0, 0.0));
        for beside in [under, left_of, pos2(800.0, 500.0)] {
            assert!(the_views(beside), "{beside:?} is the view's");
        }
        let at = wrapped.center();
        assert!(!frame(&ctx, vec![Event::PointerMoved(at), pen(at, true)], &long, true));
        assert!(frame(&ctx, vec![pen(at, false)], &long, true), "a tap on it is told to the caller");
        let beside = wrapped.center_bottom() + vec2(0.0, 40.0);
        frame(&ctx, vec![Event::PointerMoved(beside), pen(beside, true)], &long, true);
        assert!(!frame(&ctx, vec![pen(beside, false)], &long, true), "a tap beside it is not");
    }

    /// With only notes that come and go by themselves on it, the plate lets
    /// the pen through to the model.
    #[test]
    fn a_plate_with_nothing_to_put_away_does_not_take_the_pen() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let notes = [line("Triangle limit reached (4M). Remesh at a lower resolution to keep refining.", dyntopo::WARNING)];
        frame(&ctx, vec![], &notes, false);
        frame(&ctx, vec![], &notes, false);
        let rect = plate(&ctx).expect("the plate is up");
        let under = ctx.layer_id_at(rect.center());
        assert!(under.is_none_or(|layer| layer.order == Order::Background), "the pen goes through it: {under:?}");
        let at = rect.center();
        frame(&ctx, vec![Event::PointerMoved(at), pen(at, true)], &notes, false);
        assert!(!frame(&ctx, vec![pen(at, false)], &notes, false));
    }
}
