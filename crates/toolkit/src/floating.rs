//! Cards pulled out of their panel to float over the viewport.
//!
//! Every [`category`](crate::theme::category) card has a small button in its
//! header that pulls it out of its panel into a window of its own: dragged
//! about by its title, put back with the button in the same corner. The card
//! is drawn by the same code either way. The panel calls `category` as it
//! always has, and `category` puts the card in the panel or in its window,
//! so a tool's panel needs no change to have cards that float.
//!
//! That is also why a floating card shows exactly when its panel would have
//! shown it: a tool's card floats while that tool is up, goes with it, and
//! is back where it was left the next time the tool comes up. A host whose
//! panel is hidden but whose floating cards should stay runs the panel's
//! code through [`only`], which draws the floating cards and nothing else.
//!
//! Which cards float, and where, rides in egui's own data store, so no
//! global is needed. The host hands over what its settings remembered with
//! [`restore`] and reads it back with [`snapshot`]. A window is always drawn
//! inside the app's window, however it was left; a host with something at
//! the screen's edge that must stay in reach narrows that with [`set_room`].

use std::collections::BTreeMap;

use egui::{Context, Id, LayerId, Order, Pos2, Rect, Sense, Ui, UiBuilder, UiStackInfo, Vec2, pos2, vec2};

use crate::theme::{self, CardHeader, Corner};

/// The furthest from the corner of the window a remembered place can be and
/// still be believed, in points. Anything further is not a place on a screen.
pub const FURTHEST: f32 = 100_000.0;

/// How far from the card a window appears when its card is pulled out: clear
/// of the panel's edge and of the tab that hangs off it.
const PULL_OUT: f32 = 34.0;

/// The mark on the invisible `Ui` that [`only`] runs a panel in.
const ONLY: &str = "sculpt_floating_cards_only";

/// One floating card.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Card {
    /// Where its top-left corner was left, in points. The window is drawn as
    /// near to this as the screen allows ([`keep_on_screen`]), and this is
    /// not overwritten by that: a card by the right edge goes back there
    /// when a window that was made small is made large again.
    at: Pos2,
    /// Its size when it was last drawn; nothing before the first time.
    size: Vec2,
}

#[derive(Clone, Default)]
struct Cards(BTreeMap<String, Card>);

fn cards<R>(ctx: &Context, f: impl FnOnce(&mut BTreeMap<String, Card>) -> R) -> R {
    ctx.data_mut(|d| f(&mut d.get_temp_mut_or_default::<Cards>(Id::new("floating_cards")).0))
}

fn area_id(key: &str) -> Id {
    Id::new(("floating_card", key))
}

/// Whether a remembered place could be one: two numbers, on or near a screen.
pub fn believable(at: [f32; 2]) -> bool {
    at.iter().all(|v| v.is_finite() && v.abs() <= FURTHEST)
}

/// Hands over the floating cards a settings file remembered, by the card's
/// name: the left and top of each window, in points. A place that is not
/// [`believable`] is left out, so that card comes back in its panel.
pub fn restore(ctx: &Context, remembered: &BTreeMap<String, [f32; 2]>) {
    let kept = remembered.iter().filter(|(_, at)| believable(**at));
    let kept = kept.map(|(key, at)| (key.clone(), Card { at: pos2(at[0], at[1]), size: Vec2::ZERO })).collect();
    cards(ctx, |cards| *cards = kept);
}

/// The floating cards and where each was left, for a settings file. To whole
/// points: nobody places a window finer, and it keeps the file quiet.
pub fn snapshot(ctx: &Context) -> BTreeMap<String, [f32; 2]> {
    cards(ctx, |cards| cards.iter().map(|(key, card)| (key.clone(), [card.at.x.round(), card.at.y.round()])).collect())
}

/// Pulls the card called `key` out of its panel to float with its top-left
/// corner at `at`, or moves it there if it floats already.
pub fn float(ctx: &Context, key: &str, at: Pos2) {
    if believable([at.x, at.y]) {
        cards(ctx, |cards| cards.entry(key.to_owned()).or_insert(Card { at, size: Vec2::ZERO }).at = at);
    }
}

/// Puts the card called `key` back in its panel.
pub fn dock(ctx: &Context, key: &str) {
    cards(ctx, |cards| cards.remove(key));
}

pub fn is_floating(ctx: &Context, key: &str) -> bool {
    cards(ctx, |cards| cards.contains_key(key))
}

/// Whether any card floats, showing or not.
pub fn any(ctx: &Context) -> bool {
    cards(ctx, |cards| !cards.is_empty())
}

/// How many cards float, showing or not.
pub fn count(ctx: &Context) -> usize {
    cards(ctx, |cards| cards.len())
}

/// Where the window of the card called `key` is on screen, if it is showing.
pub fn window(ctx: &Context, key: &str) -> Option<Rect> {
    let id = area_id(key);
    ctx.memory(|m| m.areas().is_visible(&LayerId::new(Order::Middle, id)).then(|| m.area_rect(id)).flatten())
}

/// Sets the part of the app's window that floating cards are kept inside,
/// for this frame and until it is set again: for a host that keeps a strip
/// of the window clear for something that must stay in reach (the tab that
/// brings a folded panel back).
pub fn set_room(ctx: &Context, room: Rect) {
    ctx.data_mut(|d| d.insert_temp(Id::new("floating_cards_room"), room));
}

/// The part of the app's window floating cards are kept inside: what the
/// host set, as far as it is on the window, else the whole window.
fn room(ctx: &Context) -> Rect {
    let screen = ctx.content_rect();
    let set: Option<Rect> = ctx.data(|d| d.get_temp(Id::new("floating_cards_room")));
    set.map(|room| room.intersect(screen)).filter(|room| room.is_finite() && room.is_positive()).unwrap_or(screen)
}

/// Where the button in the header of the card called `key` was last drawn:
/// the one that floats it while it is in its panel, the one that puts it
/// back while it floats. For scripted checks, which have a pen to aim.
pub fn button(ctx: &Context, key: &str) -> Option<Rect> {
    ctx.data(|d| d.get_temp(button_id(key)))
}

fn button_id(key: &str) -> Id {
    Id::new(("card_button", key))
}

/// Notes where the float or dock button of the card called `key` is.
pub(crate) fn note_button(ctx: &Context, key: &str, rect: Rect) {
    ctx.data_mut(|d| d.insert_temp(button_id(key), rect));
}

/// Where to draw a window `size` big that was left at `at`, so that all of
/// it is on `screen`: as near to where it was left as that allows. A window
/// larger than the screen keeps its top-left corner, where its title is, on
/// screen; a place that is not a number goes to the top-left corner.
pub fn keep_on_screen(at: Pos2, size: Vec2, screen: Rect) -> Pos2 {
    // `max` last, so that when there is no room the low end wins.
    let fit = |v: f32, low: f32, high: f32| if v.is_finite() { v.min(high).max(low) } else { low };
    pos2(
        fit(at.x, screen.left(), screen.right() - size.x),
        fit(at.y, screen.top(), screen.bottom() - size.y),
    )
}

/// Runs a panel's `body` for its floating cards alone: nothing of the panel
/// is drawn or takes the pointer, but every card of it that floats is shown.
/// For a panel that is hidden while its floating cards stay. `width` is the
/// width the panel gives its cards, which a floating card keeps; `name`
/// tells one such panel from another.
pub fn only(ui: &mut Ui, name: &str, width: f32, body: impl FnOnce(&mut Ui)) {
    if !any(ui.ctx()) {
        return;
    }
    // Far off the screen as well as invisible: nothing of it can be hovered.
    let place = Rect::from_min_size(pos2(-2.0 * FURTHEST, -2.0 * FURTHEST), vec2(width, 0.0));
    let info = UiStackInfo::default().with_tag(ONLY);
    let builder = UiBuilder::new().id_salt(("floating_cards_only", name)).max_rect(place).ui_stack_info(info).invisible();
    let mut hidden = ui.new_child(builder);
    hidden.set_width(width);
    body(&mut hidden);
}

/// Whether `ui` is the invisible one [`only`] runs a panel in. A card that
/// does not float has nothing to do there.
pub fn hidden(ui: &Ui) -> bool {
    ui.stack().iter().any(|level| level.tags().contains(ONLY))
}

/// Pulls the card called `key` out, from a click on the button in its
/// header at `header`: the window appears level with the card, beside it on
/// the viewport's side, and a step down and across from any window already
/// there so that it does not hide it.
pub(crate) fn pull_out(ctx: &Context, key: &str, header: Rect) {
    let screen = room(ctx);
    let on_the_left = header.center().x < screen.center().x;
    let x = if on_the_left { header.right() + PULL_OUT } else { header.left() - header.width() - PULL_OUT };
    let mut at = pos2(x, header.top());
    cards(ctx, |cards| {
        let taken = |at: Pos2| cards.values().any(|card| (card.at - at).abs().max_elem() < 12.0);
        while taken(at) {
            at += vec2(if on_the_left { 20.0 } else { -20.0 }, 20.0);
        }
    });
    float(ctx, key, at);
    ctx.request_repaint();
}

/// Keeps the windows of floating cards over everything else of their order:
/// a tap on a panel must not bring the panel up over a card left on top of
/// it. egui brings an area to the front when it is pressed, and areas asked
/// to the front in the same frame keep their order, so every floating card
/// asks every frame, except on the frame another floating card is pressed:
/// then that one alone asks, and comes out on top of the others.
fn raise(ctx: &Context, layer: LayerId) {
    let pressed = ctx.input(|i| i.pointer.any_pressed());
    let under_pointer = ctx.pointer_interact_pos().filter(|_| pressed).and_then(|at| ctx.layer_id_at(at));
    let another_card = under_pointer.is_some_and(|top| {
        top != layer && top.order == layer.order && cards(ctx, |cards| cards.keys().any(|key| area_id(key) == top.id))
    });
    if !another_card {
        ctx.move_to_top(layer);
    }
}

/// Draws the floating card called `key` in its window. `ui` is the panel
/// the card belongs to (it gives the card its width), `docked` the id its
/// open or closed state is kept under there, and `body` its contents.
pub(crate) fn show(ui: &mut Ui, key: &str, header: &CardHeader, docked: Id, body: impl FnOnce(&mut Ui)) {
    let ctx = ui.ctx().clone();
    let Some(card) = cards(&ctx, |cards| cards.get(key).copied()) else { return };
    let width = ui.available_width();
    let screen = room(&ctx);
    let size = if card.size == Vec2::ZERO { vec2(width, theme::CARD_HEADER_HEIGHT) } else { card.size };
    let at = keep_on_screen(card.at, size, screen);
    let id = area_id(key);
    let (mut moved, mut put_back) = (Vec2::ZERO, false);
    let frame = egui::Frame::new()
        .fill(theme::CARD())
        .corner_radius(7.0)
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(46)))
        .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: egui::Color32::from_black_alpha(110) });
    let area = egui::Area::new(id).order(Order::Middle).fade_in(false).fixed_pos(at).show(&ctx, |ui| {
        // An area only offers the size it had the frame before: ask for the
        // window's whole height, so the card can be as tall as its contents.
        ui.set_max_height(screen.height());
        frame.show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            let (rect, bar) = ui.allocate_exact_size(vec2(width, theme::CARD_HEADER_HEIGHT), Sense::click_and_drag());
            let button = ui.interact(Corner::Dock.button(rect), id.with("dock"), Sense::click());
            note_button(&ctx, key, button.rect);
            if bar.dragged() {
                moved = bar.drag_delta();
            }
            put_back = button.clicked();
            theme::paint_card_header(ui, rect, header, Corner::Dock, &bar, &button);
            bar.on_hover_cursor(egui::CursorIcon::Grab);
            button.on_hover_text("Put this card back in its panel.");
            // Never taller than the window: a long card scrolls.
            let room = (screen.height() - theme::CARD_HEADER_HEIGHT - 2.0).max(40.0);
            egui::ScrollArea::vertical().max_height(room).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(width);
                theme::card_body(ui, body);
            });
        });
    });
    raise(&ctx, area.response.layer_id);
    if put_back {
        dock(&ctx, key);
        // Back in its panel it is open, as it was a moment ago here.
        if !hidden(ui) {
            ui.data_mut(|d| d.insert_persisted(docked, true));
        }
        ctx.request_repaint();
        return;
    }
    cards(&ctx, |cards| {
        if let Some(card) = cards.get_mut(key) {
            card.size = area.response.rect.size();
            if moved != Vec2::ZERO {
                // From where it is drawn, not from where it was left: a card
                // held at the screen's edge moves as soon as it is pulled back.
                card.at = at + moved;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use egui::{Event, Modifiers, PointerButton};

    use super::*;

    const SCREEN: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1600.0, 900.0));
    /// A panel's worth of cards, 250 points wide, at the left of the screen.
    const PANEL: Rect = Rect::from_min_max(pos2(10.0, 10.0), pos2(260.0, 900.0));

    fn context() -> Context {
        let ctx = Context::default();
        theme::install(&ctx);
        ctx
    }

    /// What the cards' bodies did in a frame: each card drawn, and whether
    /// its one button was clicked.
    #[derive(Default, Debug, PartialEq)]
    struct Seen {
        drawn: Vec<&'static str>,
        clicked: Vec<&'static str>,
    }

    /// The two cards of the test panel, drawn into `ui`, and under them
    /// something of the panel's own that takes a tap.
    fn two_cards(ui: &mut Ui, seen: &mut Seen) {
        for (title, open) in [("Brushes", true), ("Masking", false)] {
            theme::category(ui, title, theme::ACCENT(), "summary", open, |ui| {
                seen.drawn.push(title);
                if theme::button(ui, ui.available_width(), "Press", Default::default()).clicked() {
                    seen.clicked.push(title);
                }
            });
        }
        if ui.allocate_exact_size(vec2(ui.available_width(), 600.0), Sense::click()).1.clicked() {
            seen.clicked.push("the panel");
        }
    }

    /// One frame on a screen of `screen`: the panel with its two cards, or
    /// only its floating ones when `folded`.
    fn frame_on(ctx: &Context, screen: Rect, folded: bool, events: Vec<Event>) -> Seen {
        let mut seen = Seen::default();
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        ctx.run_ui(input, |root| {
            if folded {
                only(root, "left", PANEL.width(), |ui| two_cards(ui, &mut seen));
            } else {
                egui::Area::new(Id::new("panel")).order(Order::Middle).fixed_pos(PANEL.min).show(root.ctx(), |ui| {
                    ui.set_width(PANEL.width());
                    two_cards(ui, &mut seen);
                });
            }
        })
        .drop_without_applying_deltas();
        seen
    }

    fn frame(ctx: &Context, folded: bool, events: Vec<Event>) -> Seen {
        frame_on(ctx, SCREEN, folded, events)
    }

    fn pen(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() }
    }

    /// A tap of the pen at `at`: down in one frame, up in the next, then a
    /// frame for what it set going.
    fn tap(ctx: &Context, folded: bool, at: Pos2) -> Seen {
        frame(ctx, folded, vec![Event::PointerMoved(at), pen(at, true)]);
        let seen = frame(ctx, folded, vec![pen(at, false)]);
        frame(ctx, folded, vec![]);
        seen
    }

    /// The button at the right of the first card's header, in the panel.
    fn pull_out_button() -> Pos2 {
        let header = Rect::from_min_size(PANEL.min, vec2(PANEL.width(), theme::CARD_HEADER_HEIGHT));
        Corner::Chevron(true).button(header).center()
    }

    #[test]
    fn a_cards_button_is_where_it_was_last_drawn() {
        let ctx = context();
        assert_eq!(button(&ctx, "Brushes"), None);
        frame(&ctx, false, vec![]);
        frame(&ctx, false, vec![]);
        assert_eq!(button(&ctx, "Brushes").map(|rect| rect.center()), Some(pull_out_button()));
        float(&ctx, "Brushes", pos2(500.0, 100.0));
        frame(&ctx, false, vec![]);
        frame(&ctx, false, vec![]);
        let window = window(&ctx, "Brushes").unwrap();
        let dock = button(&ctx, "Brushes").unwrap();
        assert!(window.contains_rect(dock) && dock.right() > window.right() - 6.0 && dock.top() < window.top() + 2.0);
    }

    #[test]
    fn a_window_is_kept_on_the_screen_down_to_no_screen_at_all() {
        let size = vec2(250.0, 400.0);
        let keep = |x: f32, y: f32| keep_on_screen(pos2(x, y), size, SCREEN);
        assert_eq!(keep(300.0, 200.0), pos2(300.0, 200.0), "one that fits stays where it was left");
        assert_eq!(keep(1500.0, 800.0), pos2(1350.0, 500.0), "one off the far corner comes back in whole");
        assert_eq!(keep(-40.0, -900.0), pos2(0.0, 0.0));
        assert_eq!(keep(f32::MAX, f32::MIN), pos2(1350.0, 0.0));
        assert_eq!(keep(f32::NAN, f32::INFINITY), pos2(0.0, 0.0), "not a number: the corner");
        // A screen smaller than the window, down to none: the title stays in.
        for (width, height) in [(249.0, 399.0), (100.0, 50.0), (1.0, 1.0), (0.0, 0.0)] {
            let small = Rect::from_min_size(pos2(5.0, 7.0), vec2(width, height));
            for at in [pos2(300.0, 200.0), pos2(-300.0, -200.0), pos2(5.0, 7.0)] {
                assert_eq!(keep_on_screen(at, size, small), pos2(5.0, 7.0), "{width} x {height}, left at {at:?}");
            }
        }
        // A window with no size yet (not drawn once) is held by its corner.
        assert_eq!(keep_on_screen(pos2(1700.0, 950.0), Vec2::ZERO, SCREEN), pos2(1600.0, 900.0));
    }

    #[test]
    fn remembered_places_that_are_nonsense_come_back_docked() {
        let ctx = context();
        let remembered = BTreeMap::from([
            ("Brushes".to_owned(), [300.0, 200.0]),
            ("Masking".to_owned(), [f32::NAN, 10.0]),
            ("Remesh".to_owned(), [1.0e9, 10.0]),
            ("File".to_owned(), [10.0, f32::NEG_INFINITY]),
            ("Objects".to_owned(), [-250.0, 99_999.0]),
        ]);
        restore(&ctx, &remembered);
        assert!(is_floating(&ctx, "Brushes") && is_floating(&ctx, "Objects"));
        assert!(!is_floating(&ctx, "Masking") && !is_floating(&ctx, "Remesh") && !is_floating(&ctx, "File"));
        let kept = snapshot(&ctx);
        assert_eq!(kept, BTreeMap::from([("Brushes".to_owned(), [300.0, 200.0]), ("Objects".to_owned(), [-250.0, 99_999.0])]));
        // What is handed over replaces what was there.
        restore(&ctx, &BTreeMap::new());
        assert!(!any(&ctx));
        float(&ctx, "Brushes", pos2(f32::NAN, 0.0));
        assert!(!any(&ctx), "nor can a card be floated to nowhere");
    }

    /// The whole life of a floating card with a pen: pulled out by the button
    /// in its header, the same body drawn in the window, dragged by its
    /// title, still up with its panel folded away, and put back.
    #[test]
    fn a_card_is_pulled_out_dragged_and_put_back() {
        let ctx = context();
        // Two frames for the panel to size itself and be laid out.
        frame(&ctx, false, vec![]);
        assert_eq!(frame(&ctx, false, vec![]).drawn, ["Brushes"], "Masking starts closed");
        assert!(!any(&ctx) && window(&ctx, "Brushes").is_none());

        tap(&ctx, false, pull_out_button());
        assert!(is_floating(&ctx, "Brushes"), "the button in the header pulls the card out");
        let seen = frame(&ctx, false, vec![]);
        assert_eq!(seen.drawn, ["Brushes"], "its body is drawn once, by the same code");
        let at = window(&ctx, "Brushes").expect("it has a window");
        assert!(at.left() > PANEL.right() && SCREEN.contains_rect(at), "beside the panel, on screen: {at:?}");
        assert!((at.width() - PANEL.width()).abs() <= 2.0, "as wide as it was in the panel");

        // The button inside the window works: it is the card, not a picture.
        let inside = pos2(at.center().x, at.top() + theme::CARD_HEADER_HEIGHT + 8.0 + 14.0);
        assert_eq!(tap(&ctx, false, inside).clicked, ["Brushes"]);

        // Dragged by its title.
        let grip = pos2(at.left() + 60.0, at.top() + 16.0);
        frame(&ctx, false, vec![Event::PointerMoved(grip), pen(grip, true)]);
        let to = grip + vec2(400.0, 250.0);
        frame(&ctx, false, vec![Event::PointerMoved(grip + vec2(200.0, 100.0))]);
        frame(&ctx, false, vec![Event::PointerMoved(to)]);
        frame(&ctx, false, vec![pen(to, false)]);
        frame(&ctx, false, vec![]);
        let moved = window(&ctx, "Brushes").unwrap();
        assert!((moved.min - (at.min + vec2(400.0, 250.0))).length() < 8.0, "{at:?} to {moved:?}");
        assert!(is_floating(&ctx, "Brushes"), "a drag is not a tap on anything");
        let kept = snapshot(&ctx)["Brushes"];
        assert!((kept[0] - moved.left()).abs() <= 1.5 && (kept[1] - moved.top()).abs() <= 1.5);

        // With its panel folded away it stays, alone.
        frame(&ctx, true, vec![]);
        let seen = frame(&ctx, true, vec![]);
        assert_eq!(seen.drawn, ["Brushes"]);
        assert_eq!(window(&ctx, "Brushes").map(|rect| rect.min), Some(moved.min));
        assert!(ctx.memory(|m| !m.areas().is_visible(&LayerId::new(Order::Middle, Id::new("panel")))));

        // Put back with the button in its corner.
        let header = Rect::from_min_size(moved.min, vec2(moved.width(), theme::CARD_HEADER_HEIGHT));
        tap(&ctx, false, Corner::Dock.button(header).center());
        assert!(!is_floating(&ctx, "Brushes") && window(&ctx, "Brushes").is_none());
        assert_eq!(frame(&ctx, false, vec![]).drawn, ["Brushes"], "and it is back in its panel, open");
    }

    /// The pointer on a floating card is on the card's own layer (what a host
    /// goes by to keep a press from reaching the model), a tap on the panel
    /// under a card does not bring the panel up over it, and a tap on one
    /// card brings it over another.
    #[test]
    fn floating_cards_stay_over_the_panel_and_the_last_one_touched_over_the_rest() {
        let ctx = context();
        frame(&ctx, false, vec![]);
        frame(&ctx, false, vec![]);
        // Both cards float over the panel itself, overlapping each other.
        float(&ctx, "Brushes", pos2(40.0, 300.0));
        float(&ctx, "Masking", pos2(120.0, 310.0));
        frame(&ctx, false, vec![]);
        frame(&ctx, false, vec![]);
        let top_at = |at: Pos2| ctx.layer_id_at(at).map(|layer| layer.id);
        let (brushes, masking) = (area_id("Brushes"), area_id("Masking"));
        let on_brushes_only = pos2(60.0, 316.0);
        let on_both = pos2(200.0, 326.0);
        assert_eq!(top_at(on_brushes_only), Some(brushes));
        // What a host goes by: anything but the background layer is interface.
        let viewport = ctx.layer_id_at(pos2(1000.0, 600.0));
        assert!(viewport.is_none_or(|layer| layer.order == Order::Background), "nothing is over the open viewport");
        assert!(ctx.layer_id_at(on_both).is_some_and(|layer| layer.order == Order::Middle));

        // A tap on the panel, above the cards: the cards stay over it.
        assert_eq!(tap(&ctx, false, pos2(130.0, 200.0)).clicked, ["the panel"]);
        assert_eq!(top_at(on_brushes_only), Some(brushes), "the panel has not come up over the card");
        assert_eq!(tap(&ctx, false, on_brushes_only).clicked, [""; 0], "and a tap on the card stops at the card");

        // A tap on each card in turn brings that one over the other.
        tap(&ctx, false, on_brushes_only);
        assert_eq!(top_at(on_both), Some(brushes));
        // On the second card alone, short of the button in its corner.
        tap(&ctx, false, pos2(310.0, 326.0));
        assert_eq!(top_at(on_both), Some(masking));
        tap(&ctx, false, on_brushes_only);
        assert_eq!(top_at(on_both), Some(brushes));
    }

    /// A card left where a smaller window, or a larger interface, no longer
    /// reaches is drawn back inside, and returns when there is room again.
    #[test]
    fn a_floating_card_follows_the_window_in_and_goes_back_out() {
        let ctx = context();
        float(&ctx, "Brushes", pos2(1300.0, 700.0));
        for _ in 0..3 {
            frame(&ctx, false, vec![]);
        }
        let big = window(&ctx, "Brushes").unwrap();
        assert!(SCREEN.contains_rect(big.shrink(1.0)) && big.left() > 1200.0, "{big:?}");
        let small = Rect::from_min_size(Pos2::ZERO, vec2(700.0, 420.0));
        for _ in 0..3 {
            frame_on(&ctx, small, false, vec![]);
        }
        let pulled_in = window(&ctx, "Brushes").unwrap();
        assert!(small.contains_rect(pulled_in.shrink(1.0)), "{pulled_in:?}");
        assert_eq!(snapshot(&ctx)["Brushes"], [1300.0, 700.0], "where it was left is not forgotten");
        for _ in 0..3 {
            frame(&ctx, false, vec![]);
        }
        assert_eq!(window(&ctx, "Brushes").unwrap().min, big.min);
    }

    /// A host can keep a strip of the window clear of floating cards; what
    /// it sets is believed only as far as it is a piece of the window.
    #[test]
    fn floating_cards_keep_to_the_room_the_host_gives_them() {
        let ctx = context();
        float(&ctx, "Brushes", pos2(5000.0, 100.0));
        let room = Rect::from_min_max(pos2(22.0, 0.0), pos2(1578.0, 900.0));
        let settle = |set: Option<Rect>| {
            for _ in 0..3 {
                if let Some(room) = set {
                    set_room(&ctx, room);
                }
                frame(&ctx, false, vec![]);
            }
            window(&ctx, "Brushes").unwrap()
        };
        let whole = settle(None);
        assert!((whole.right() - 1600.0).abs() <= 1.0, "to the window's edge until a room is set: {whole:?}");
        let kept = settle(Some(room));
        assert!((kept.right() - 1578.0).abs() <= 1.0, "{kept:?}");
        // Nonsense, and a room that is off the window: the whole window again.
        for odd in [Rect::NOTHING, Rect::NAN, Rect::from_min_size(pos2(4000.0, 0.0), vec2(500.0, 500.0))] {
            let back = settle(Some(odd));
            assert!((back.right() - 1600.0).abs() <= 1.0, "{odd:?}: {back:?}");
        }
    }

    /// A card taller than the window scrolls inside it instead of hanging
    /// off the bottom.
    #[test]
    fn a_floating_card_is_never_taller_than_the_window() {
        let ctx = context();
        float(&ctx, "Tall", pos2(300.0, 100.0));
        let short = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 300.0));
        for _ in 0..3 {
            let input = egui::RawInput { screen_rect: Some(short), ..Default::default() };
            ctx.run_ui(input, |root| {
                only(root, "left", 250.0, |ui| {
                    theme::category(ui, "Tall", theme::ACCENT(), "", true, |ui| {
                        for _ in 0..40 {
                            theme::heading(ui, "A line");
                        }
                    });
                });
            })
            .drop_without_applying_deltas();
        }
        let rect = window(&ctx, "Tall").unwrap();
        assert!(short.contains_rect(rect.shrink(1.0)), "{rect:?}");
        assert!(rect.height() > 290.0, "and it uses all the height there is: {rect:?}");
    }

    /// A panel run for its floating cards alone draws nothing else, and a
    /// card whose panel is not run at all (its tool is away) is not shown
    /// but is still remembered.
    #[test]
    fn a_floating_card_shows_only_while_its_panel_would_show_it() {
        let ctx = context();
        assert!(frame(&ctx, true, vec![]).drawn.is_empty(), "nothing floats: the folded panel does no work");
        float(&ctx, "Masking", pos2(400.0, 100.0));
        frame(&ctx, true, vec![]);
        assert_eq!(frame(&ctx, true, vec![]).drawn, ["Masking"], "only the floating card; it floats open");
        assert!(window(&ctx, "Masking").is_some());
        // Its panel is not run: two frames later the window has gone.
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput { screen_rect: Some(SCREEN), ..Default::default() }, |_| {}).drop_without_applying_deltas();
        }
        assert!(window(&ctx, "Masking").is_none());
        assert_eq!(snapshot(&ctx), BTreeMap::from([("Masking".to_owned(), [400.0, 100.0])]));
        frame(&ctx, false, vec![]);
        frame(&ctx, false, vec![]);
        assert_eq!(window(&ctx, "Masking").map(|rect| rect.min), Some(pos2(400.0, 100.0)), "back where it was left");
    }
}
