//! The tabs of the two panels.
//!
//! Each panel was one long column of cards, and finding one meant scrolling
//! past the rest. Now a panel shows a tab's worth at a time: the Sculpt
//! panel the brush in hand or what is done to the whole mesh, the Scene
//! panel the objects, how they look, the file, or the app's own settings.
//! Which tab each panel is on is remembered between runs (`crate::prefs`).
//!
//! A card pulled out of its panel to float stays up whichever tab is
//! showing: the tabs that are not showing are still run, out of sight, for
//! their floating cards alone (`sculpt_toolkit::floating::only`), which
//! costs nothing while no card floats.

use egui::{Align2, Color32, Rect, Sense, Ui, pos2, vec2};
use sculpt_toolkit::floating;

use super::theme;

/// The Sculpt panel's tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sculpt {
    /// The brushes, the settings of the one in hand, the stroke.
    #[default]
    Brush,
    /// What is done to the whole mesh: symmetry, remesh, masks, hidden parts.
    Mesh,
}

/// The Scene panel's tabs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scene {
    /// The objects, and the cards the tools keep beside them.
    #[default]
    Objects,
    /// Material, lighting, backdrop and effects.
    Look,
    /// Open, save, import and export, and the videos.
    File,
    /// The app's own: colours, interface size, shortcuts, performance.
    Settings,
}

impl Sculpt {
    pub const ALL: [Sculpt; 2] = [Sculpt::Brush, Sculpt::Mesh];

    /// What its tab says, and what the settings file calls it.
    pub fn name(self) -> &'static str {
        match self {
            Sculpt::Brush => "Brush",
            Sculpt::Mesh => "Mesh",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Sculpt::Brush => "The brushes, the settings of the one in hand, and the stroke.",
            Sculpt::Mesh => "What is done to the whole mesh: symmetry, remesh, masks and hidden parts.",
        }
    }
}

impl Scene {
    pub const ALL: [Scene; 4] = [Scene::Objects, Scene::Look, Scene::File, Scene::Settings];

    /// What its tab says, and what the settings file calls it.
    pub fn name(self) -> &'static str {
        match self {
            Scene::Objects => "Scene",
            Scene::Look => "Look",
            Scene::File => "File",
            Scene::Settings => "Settings",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Scene::Objects => "The objects in the scene, and adding to them.",
            Scene::Look => "How the model is shown: material, lighting, backdrop and effects.",
            Scene::File => "Open, save, import and export, and the turntable and timelapse videos.",
            Scene::Settings => "The app's own settings: colours, interface size, shortcuts.",
        }
    }
}

/// Which tab each panel is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Showing {
    pub sculpt: Sculpt,
    pub scene: Scene,
}

impl Showing {
    /// The tabs called `sculpt` and `scene`, whatever their case. A name
    /// that is no tab's gives that panel's first.
    pub fn named(sculpt: &str, scene: &str) -> Showing {
        let is = |name: &str, given: &str| name.eq_ignore_ascii_case(given.trim());
        Showing {
            sculpt: Sculpt::ALL.into_iter().find(|tab| is(tab.name(), sculpt)).unwrap_or_default(),
            scene: Scene::ALL.into_iter().find(|tab| is(tab.name(), scene)).unwrap_or_default(),
        }
    }
}

/// As tall as a card's header: a pen finds it as easily.
const HEIGHT: f32 = 32.0;

/// The Sculpt panel's tab strip.
pub fn sculpt_strip(ui: &mut Ui, showing: &mut Sculpt, tips: bool) {
    strip(ui, &Sculpt::ALL.map(|tab| (tab, tab.name(), tab.tip())), showing, tips);
}

/// The Scene panel's tab strip.
pub fn scene_strip(ui: &mut Ui, showing: &mut Scene, tips: bool) {
    strip(ui, &Scene::ALL.map(|tab| (tab, tab.name(), tab.tip())), showing, tips);
}

/// A row of tabs across the panel, each `(tab, name, tooltip)`, with the one
/// that is `showing` marked. A tap on another makes that one `showing`.
fn strip<T: Copy + PartialEq>(ui: &mut Ui, tabs: &[(T, &str, &str)], showing: &mut T, tips: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
    ui.painter().rect_filled(rect, 7.0, theme::FIELD());
    let width = rect.width() / tabs.len() as f32;
    for (i, &(tab, name, help)) in tabs.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(rect.left() + width * i as f32, rect.top()), vec2(width, HEIGHT));
        let response = ui.interact(cell, ui.id().with(("tab", name)), Sense::click());
        let on = tab == *showing;
        let p = ui.painter();
        let pill = cell.shrink(3.0);
        if on {
            p.rect_filled(pill, 5.0, theme::HEADER());
            // The same mark a card's header wears, lying down.
            let mark = Rect::from_center_size(pos2(pill.center().x, pill.bottom() - 2.5), vec2(18.0, 3.0));
            p.rect_filled(mark, 1.5, theme::ACCENT());
        } else if response.hovered() {
            p.rect_filled(pill, 5.0, Color32::from_white_alpha(14));
        }
        let (face, ink) = if on { (theme::bold(12.5), theme::TEXT()) } else { (theme::font(12.5), theme::HINT()) };
        p.text(cell.center() - vec2(0.0, 1.0), Align2::CENTER_CENTER, name, face, ink);
        if super::tip(response, tips, help).clicked() {
            *showing = tab;
        }
    }
}

/// One tab's cards: drawn in `ui` when the tab is `showing`, else run out of
/// sight for those of its cards that float. `name` tells the tabs of a
/// panel apart. True when a card of a tab that is not showing was put back
/// in the panel: that tab should come up, or the card would simply vanish.
pub fn page(ui: &mut Ui, showing: bool, name: &str, body: impl FnOnce(&mut Ui)) -> bool {
    if showing {
        body(ui);
        return false;
    }
    let ctx = ui.ctx().clone();
    let floating_before = floating::count(&ctx);
    if floating_before == 0 {
        return false;
    }
    floating::only(ui, name, ui.available_width(), body);
    floating::count(&ctx) < floating_before
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tab is remembered by the name on it, whatever its case; a name that
    /// is no tab's (a file from another version, or edited by hand) gives
    /// the panel's first tab and leaves the other panel's alone.
    #[test]
    fn a_tab_is_found_by_its_name_and_a_strange_name_gives_the_first() {
        for sculpt in Sculpt::ALL {
            for scene in Scene::ALL {
                assert_eq!(Showing::named(sculpt.name(), scene.name()), Showing { sculpt, scene });
            }
        }
        assert_eq!(Showing::named("mesh", " SETTINGS "), Showing { sculpt: Sculpt::Mesh, scene: Scene::Settings });
        assert_eq!(Showing::named("", ""), Showing::default());
        assert_eq!(Showing::named("Topology", "Look"), Showing { sculpt: Sculpt::Brush, scene: Scene::Look });
        assert_eq!(Showing::named("Mesh", "7"), Showing { sculpt: Sculpt::Mesh, scene: Scene::Objects });
        assert_eq!(Showing::default(), Showing { sculpt: Sculpt::Brush, scene: Scene::Objects }, "where a first run starts");
    }

    /// One frame of a panel with two tabs, a card on each; `first` is the
    /// tab showing. What was drawn of each card's body, and whether the
    /// frame said a card had been put back on a tab that is not showing.
    fn frame(ctx: &egui::Context, first: bool, events: Vec<egui::Event>) -> ([bool; 2], bool) {
        let mut drawn = [false; 2];
        let mut put_back = false;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0));
        let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
        ctx.run_ui(input, |root| {
            egui::Area::new(egui::Id::new("panel")).fixed_pos(pos2(10.0, 10.0)).show(root.ctx(), |ui| {
                ui.set_width(240.0);
                put_back |= page(ui, first, "one", |ui| theme::category(ui, "One", theme::ACCENT(), "", true, |_| drawn[0] = true));
                put_back |= page(ui, !first, "two", |ui| theme::category(ui, "Two", theme::ACCENT(), "", true, |_| drawn[1] = true));
            });
        })
        .drop_without_applying_deltas();
        (drawn, put_back)
    }

    /// Only the showing tab's cards are drawn, and the other tab is not run
    /// at all while nothing floats; a card that floats is drawn whichever
    /// tab is showing.
    #[test]
    fn a_tab_that_is_not_showing_draws_only_its_floating_cards() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        assert_eq!(frame(&ctx, true, Vec::new()), ([true, false], false));
        assert_eq!(frame(&ctx, false, Vec::new()), ([false, true], false));
        floating::float(&ctx, "Two", pos2(600.0, 100.0));
        assert_eq!(frame(&ctx, true, Vec::new()), ([true, true], false), "the floating card of the other tab is up");
        assert_eq!(frame(&ctx, false, Vec::new()), ([false, true], false));
        floating::float(&ctx, "One", pos2(900.0, 100.0));
        assert_eq!(frame(&ctx, false, Vec::new()), ([true, true], false));
    }

    /// Putting a floating card back while its tab is not showing says so,
    /// once, so the panel can bring that tab up.
    #[test]
    fn putting_a_card_back_on_a_tab_that_is_not_showing_is_noticed() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        floating::float(&ctx, "Two", pos2(600.0, 100.0));
        frame(&ctx, true, Vec::new());
        let at = floating::button(&ctx, "Two").expect("the floating card has a button that puts it back").center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, true, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, true, vec![press(true)]);
        let (_, put_back) = frame(&ctx, true, vec![press(false)]);
        assert!(put_back && !floating::is_floating(&ctx, "Two"));
        assert_eq!(frame(&ctx, true, Vec::new()), ([true, false], false), "said once");
    }
}
