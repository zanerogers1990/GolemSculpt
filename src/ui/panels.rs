//! The frames of the two panels, and the tabs that fold each of them away.
//!
//! A panel has a small arrow tab hanging off its inner edge. A click folds
//! the whole panel away, leaving only a slim tab at the edge of the screen
//! to bring it back; the viewport has the space, and sculpting carries on
//! (this is not the Tab clean view, which is for looking and switches
//! strokes off). Each side folds on its own and is remembered between runs
//! (`crate::prefs`). The view gizmo and the bottom bar sit beside the panels,
//! so they move out to the screen edge with them. Cards pulled out of a
//! panel to float (`sculpt_toolkit::floating`) stay up when it is folded.

use egui::{Color32, Id, Order, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use super::theme;

pub const LEFT_WIDTH: f32 = 270.0;
pub const RIGHT_WIDTH: f32 = 260.0;
pub const MARGIN: f32 = 8.0;

/// The view gizmo's column: its bubbles, then Persp, Stroke Pivot and Frame.
const GIZMO_SIZE: Vec2 = vec2(108.0, 108.0 + 3.0 * (4.0 + 22.0));
const GIZMO_TOP: f32 = 14.0;
/// The gap between the gizmo and the panel, or the folded tab, beside it.
const GIZMO_GAP: f32 = 14.0;

/// The tab as it is painted: how far it stands out from the panel's edge
/// (a little further from the screen's, where there is no panel to find it
/// by), and how tall it is.
const TAB_OUT: f32 = 18.0;
const TAB_OUT_FOLDED: f32 = 22.0;
const TAB_HEIGHT: f32 = 56.0;
/// How far past its picture the tab still takes a tap, for a pen.
const TAB_REACH: f32 = 6.0;
/// Where the tabs hang: just under the view gizmo's column, so the right
/// panel's tab never sits on the gizmo, and the left one at the same height.
const TAB_TOP: f32 = GIZMO_TOP + GIZMO_SIZE.y + 12.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// Its place in `Options::folded`.
    pub fn index(self) -> usize {
        self as usize
    }

    fn width(self) -> f32 {
        match self {
            Side::Left => LEFT_WIDTH,
            Side::Right => RIGHT_WIDTH,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// Where a panel's top-left corner goes when it is showing.
fn panel_origin(side: Side, screen: Rect) -> Pos2 {
    match side {
        Side::Left => pos2(screen.left() + MARGIN, screen.top() + MARGIN),
        Side::Right => pos2(screen.right() - MARGIN - RIGHT_WIDTH, screen.top() + MARGIN),
    }
}

/// How much of the screen's width a side takes up, from the screen's edge:
/// its panel and margin, or only its tab when it is folded away.
fn taken(side: Side, folded: bool) -> f32 {
    if folded { TAB_OUT_FOLDED } else { MARGIN + side.width() }
}

/// The tab of `side` as it is painted. It hangs off the panel's inner edge,
/// or off the screen's edge when the panel is folded away, at one height
/// either way so it does not jump from under the pen. On a window too short
/// for that height it moves up to stay on screen.
pub fn tab_rect(side: Side, folded: bool, screen: Rect) -> Rect {
    let out = if folded { TAB_OUT_FOLDED } else { TAB_OUT };
    let inner = if folded { 0.0 } else { taken(side, false) };
    let lowest = (screen.bottom() - MARGIN - TAB_HEIGHT).max(screen.top());
    let top = (screen.top() + TAB_TOP).min(lowest);
    let left = match side {
        Side::Left => screen.left() + inner,
        Side::Right => screen.right() - inner - out,
    };
    Rect::from_min_size(pos2(left, top), vec2(out, TAB_HEIGHT))
}

/// What of the screen takes a tap for the tab painted at `tab`: a little
/// more than its picture, on every side but the one it hangs from.
fn tab_reach(side: Side, tab: Rect) -> Rect {
    let mut reach = tab.expand2(vec2(0.0, TAB_REACH));
    match side {
        Side::Left => reach.max.x += TAB_REACH,
        Side::Right => reach.min.x -= TAB_REACH,
    }
    reach
}

/// The part of the screen floating cards are kept inside: all of it but the
/// strip a folded panel's tab hangs in, so that no card can be left (or
/// brought back from a smaller window) on top of the way to get a panel back.
pub fn room_for_cards(screen: Rect, folded: [bool; 2]) -> Rect {
    let strip = |away: bool| if away { TAB_OUT_FOLDED } else { 0.0 };
    Rect::from_min_max(screen.min + vec2(strip(folded[0]), 0.0), screen.max - vec2(strip(folded[1]), 0.0))
}

/// Where the view gizmo goes: beside the right panel, or beside that
/// panel's tab at the screen edge when it is folded away.
pub fn gizmo_origin(screen: Rect, right_folded: bool) -> Pos2 {
    pos2(screen.right() - taken(Side::Right, right_folded) - GIZMO_GAP - GIZMO_SIZE.x, screen.top() + GIZMO_TOP)
}

/// Where the bottom bar starts: beside the left panel, or beside its tab.
pub fn bottom_bar_origin(screen: Rect, left_folded: bool) -> Pos2 {
    pos2(screen.left() + taken(Side::Left, left_folded) + 12.0, screen.bottom() - 12.0 - 22.0)
}

/// One panel: `body` in a frame down its side of the window, scrolling when
/// it is taller than the window. Folded away, nothing of it shows but the
/// cards that float: `body` is then run out of sight, for those alone.
/// `root` is the interface's own `Ui`, which that needs.
pub fn panel(root: &mut egui::Ui, side: Side, folded: bool, body: &mut dyn FnMut(&mut egui::Ui)) {
    let ctx = root.ctx().clone();
    let width = side.width();
    if folded {
        return sculpt_toolkit::floating::only(root, side.name(), width - 20.0, body);
    }
    let screen = ctx.content_rect();
    let max_height = (screen.height() - 2.0 * MARGIN).max(300.0);
    let id = match side {
        Side::Left => "sculpt_panel",
        Side::Right => "scene_panel",
    };
    egui::Area::new(Id::new(id)).order(Order::Middle).fade_in(false).fixed_pos(panel_origin(side, screen)).show(&ctx, |ui| {
        // A floating area only offers its previous size; ask for the full
        // window height so the panel can grow with its content.
        ui.set_max_height(max_height);
        egui::Frame::new()
            .fill(theme::PANEL)
            .corner_radius(10.0)
            .stroke(theme::hairline())
            .inner_margin(egui::Margin { left: 10, right: 10, top: 10, bottom: 10 })
            .show(ui, |ui| {
                ui.set_width(width - 20.0);
                egui::ScrollArea::vertical().max_height(max_height - 20.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    body(ui)
                });
            });
    });
}

/// The fold tab of `side`. Returns true when it was clicked: the panel
/// should fold away, or come back.
pub fn tab(ctx: &egui::Context, side: Side, folded: bool, tips: bool) -> bool {
    let rect = tab_rect(side, folded, ctx.content_rect());
    let reach = tab_reach(side, rect);
    let mut clicked = false;
    egui::Area::new(Id::new(("panel_tab", side.name()))).order(Order::Middle).fade_in(false).fixed_pos(reach.min).show(
        ctx,
        |ui| {
            let (_, response) = ui.allocate_exact_size(reach.size(), Sense::click());
            paint_tab(ui.painter(), side, folded, rect, response.hovered());
            let help = match (folded, side) {
                (true, Side::Left) => "Bring the Sculpt panel back.",
                (true, Side::Right) => "Bring the Scene panel back.",
                (false, Side::Left) => "Fold the Sculpt panel away. A tab stays at the edge of the screen to bring it back.",
                (false, Side::Right) => "Fold the Scene panel away. A tab stays at the edge of the screen to bring it back.",
            };
            clicked = super::tip(response, tips, help).clicked();
        },
    );
    clicked
}

/// The tab's picture: a lug in the panel's colours, rounded on the side that
/// faces the viewport, with an arrow pointing the way the panel will go.
fn paint_tab(p: &egui::Painter, side: Side, folded: bool, rect: Rect, hovered: bool) {
    // The rounded corners face the viewport: away from the edge it hangs on.
    let toward_right = side == Side::Left;
    let rounding = if toward_right {
        egui::CornerRadius { nw: 0, sw: 0, ne: 8, se: 8 }
    } else {
        egui::CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }
    };
    // Solid, where the panels are a little see-through: it sits on the model.
    p.rect_filled(rect, rounding, theme::HEADER);
    p.rect_stroke(rect, rounding, theme::hairline(), egui::StrokeKind::Inside);
    if hovered {
        p.rect_filled(rect, rounding, Color32::from_white_alpha(20));
    }
    // Pointing at the screen edge to fold away, into the screen to come back.
    let points_right = toward_right == folded;
    let dx = if points_right { 3.0 } else { -3.0 };
    let c = rect.center();
    let ink = Color32::from_white_alpha(if hovered { 230 } else { 170 });
    p.line(vec![c + vec2(-dx, -6.0), c + vec2(dx, 0.0), c + vec2(-dx, 6.0)], Stroke::new(2.0, ink));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(width: f32, height: f32) -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(width, height))
    }

    fn gizmo(screen: Rect, right_folded: bool) -> Rect {
        Rect::from_min_size(gizmo_origin(screen, right_folded), GIZMO_SIZE)
    }

    /// The tab hangs off its panel's inner edge, or off the screen's edge
    /// when the panel is away, and takes a tap a little past its picture.
    #[test]
    fn a_tab_hangs_off_its_panel_or_off_the_screen_edge() {
        let s = screen(1600.0, 900.0);
        let left = tab_rect(Side::Left, false, s);
        assert_eq!(left.left(), MARGIN + LEFT_WIDTH);
        let right = tab_rect(Side::Right, false, s);
        assert_eq!(right.right(), 1600.0 - MARGIN - RIGHT_WIDTH);
        assert_eq!(left.top(), right.top(), "both at one height");
        assert_eq!(tab_rect(Side::Left, true, s).left(), 0.0);
        assert_eq!(tab_rect(Side::Right, true, s).right(), 1600.0);
        // Folding does not move the tab up or down from under the pen.
        assert_eq!(tab_rect(Side::Left, true, s).top(), left.top());
        for (side, folded) in [(Side::Left, false), (Side::Left, true), (Side::Right, false), (Side::Right, true)] {
            let tab = tab_rect(side, folded, s);
            let reach = tab_reach(side, tab);
            assert!(reach.contains_rect(tab) && reach.width() > tab.width() && reach.height() > tab.height());
            assert!(s.contains_rect(reach), "{side:?} {folded}: the whole of it is on screen");
            // The side it hangs from is flush: it never reaches over the panel.
            match side {
                Side::Left => assert_eq!(reach.left(), tab.left()),
                Side::Right => assert_eq!(reach.right(), tab.right()),
            }
        }
    }

    /// At every window size and interface size worth the name, neither tab
    /// touches the view gizmo, the bottom bar or the other tab, folded or not.
    #[test]
    fn the_tabs_keep_clear_of_the_gizmo_the_bottom_bar_and_each_other() {
        // In the interface's points: a 4K screen at 100% down to a 1280 x 720
        // window at 200%, and a full HD screen at the largest size, 300%.
        for (width, height) in [(3840.0, 2160.0), (1920.0, 1080.0), (1600.0, 960.0), (1066.0, 640.0), (640.0, 360.0)] {
            let s = screen(width, height);
            for folded in [[false, false], [true, false], [false, true], [true, true]] {
                let left = tab_reach(Side::Left, tab_rect(Side::Left, folded[0], s));
                let right = tab_reach(Side::Right, tab_rect(Side::Right, folded[1], s));
                let what = format!("{width} x {height}, folded {folded:?}");
                assert!(!right.intersects(gizmo(s, folded[1])), "{what}: the right tab is on the gizmo");
                assert!(!left.intersects(gizmo(s, folded[1])), "{what}: the left tab is on the gizmo");
                assert!(!left.intersects(right), "{what}: the tabs meet");
                // The bar's three checkboxes: about 330 points of it are used.
                let bar = Rect::from_min_size(bottom_bar_origin(s, folded[0]), vec2(330.0, 22.0));
                assert!(!left.intersects(bar) && !right.intersects(bar), "{what}: a tab is on the bottom bar");
                assert!(s.contains_rect(left) && s.contains_rect(right), "{what}: a tab is off the screen");
            }
        }
    }

    /// With its panel away the gizmo moves out to the tab, and the bottom bar
    /// to the left edge: the viewport gets the room.
    #[test]
    fn folding_a_panel_hands_its_room_to_the_viewport() {
        let s = screen(1600.0, 900.0);
        assert_eq!(gizmo_origin(s, false), pos2(1600.0 - MARGIN - RIGHT_WIDTH - 14.0 - 108.0, 14.0), "as it was");
        assert_eq!(bottom_bar_origin(s, false), pos2(MARGIN + LEFT_WIDTH + 12.0, 900.0 - 12.0 - 22.0), "as it was");
        assert!(gizmo(s, true).right() <= tab_rect(Side::Right, true, s).left());
        assert!(gizmo(s, true).left() > gizmo(s, false).left() + 200.0);
        assert!(bottom_bar_origin(s, true).x >= tab_rect(Side::Left, true, s).right());
        assert!(bottom_bar_origin(s, true).x < 50.0);
    }

    /// Floating cards are kept off a folded panel's tab, and have the whole
    /// screen when no panel is folded.
    #[test]
    fn floating_cards_are_kept_off_the_folded_tabs() {
        let s = screen(1600.0, 900.0);
        assert_eq!(room_for_cards(s, [false, false]), s);
        for folded in [[true, false], [false, true], [true, true]] {
            let room = room_for_cards(s, folded);
            for side in [Side::Left, Side::Right] {
                let tab = tab_rect(side, folded[side.index()], s);
                assert_eq!(room.intersects(tab.shrink(0.01)), !folded[side.index()], "{folded:?} {side:?}");
            }
            assert!(s.contains_rect(room) && room.width() >= 1600.0 - 2.0 * TAB_OUT_FOLDED);
        }
    }

    /// A window too short for the tab's usual height still shows all of it,
    /// down to no window at all (house rule 9: the lowest input).
    #[test]
    fn a_tab_stays_on_a_window_that_is_too_short_for_it() {
        for height in [0.0, 1.0, 40.0, TAB_HEIGHT, TAB_HEIGHT + MARGIN, 200.0, 269.0, 270.0, 281.0, 282.0, 400.0] {
            let s = screen(700.0, height);
            for side in [Side::Left, Side::Right] {
                let tab = tab_rect(side, true, s);
                assert!(tab.top() >= 0.0 && tab.top().is_finite(), "{height}: {tab:?}");
                if height >= TAB_HEIGHT + MARGIN {
                    assert!(tab.bottom() <= height - MARGIN + 0.001, "{height}: {tab:?}");
                }
                assert!(tab.top() <= TAB_TOP);
            }
        }
        // And one that is not at the top of the screen's own coordinates.
        let s = Rect::from_min_size(pos2(10.0, 20.0), vec2(1000.0, 800.0));
        assert_eq!(tab_rect(Side::Left, true, s).min, pos2(10.0, 20.0 + TAB_TOP));
    }
}
