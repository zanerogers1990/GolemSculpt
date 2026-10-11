//! The interface's colour themes.
//!
//! A theme is a [`Palette`]: the colours of the panels, the cards and the
//! controls on them, the text, the accent that marks what is switched on,
//! and the backdrop behind the model that goes with them. [`THEMES`] is the
//! list the app offers; the first is the Unity app's, and the one a run
//! starts in.
//!
//! The colours a theme changes are read through functions that keep the
//! names they had as constants ([`ACCENT`], [`TEXT`]...), so the code that
//! paints with them reads as it always did. What says something by its
//! colour (the axes, subtract, mask, a result, an error) is the same in
//! every theme, and is still a constant in [`crate::theme`].
//!
//! Every theme has light text on a dark or middling surface: the icons and
//! the hover and pressed looks are painted in white, and count on it.
//!
//! Which theme is up is one number for the whole program, not something in
//! egui's data store like the rest of the interface's state: a colour is
//! asked for in places that have no `Context` to ask (a status line's
//! colour, a tool's overlay). Reading it is one atomic load.

use std::sync::atomic::{AtomicUsize, Ordering};

use egui::{Color32, CornerRadius, Stroke};

/// One theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub name: &'static str,
    /// The two panels: a little see-through, so the model shows under them.
    pub panel: Color32,
    /// A card on a panel, and a card floating over the view.
    pub card: Color32,
    /// A card's header, and the tabs that fold the panels away.
    pub header: Color32,
    /// A button, a tile.
    pub control: Color32,
    /// What is sunk into the surface: a slider's track, a text box, a
    /// checkbox's well.
    pub field: Color32,
    /// What is switched on, and how full a slider is. White text is read on
    /// it, so it is never a pale colour.
    pub accent: Color32,
    pub text: Color32,
    /// Quieter text: a remark, a readout.
    pub hint: Color32,
    /// Tooltips and egui's own pop-ups.
    pub window: Color32,
    /// The backdrop that goes with the theme, as `Options::background` and
    /// `Options::background_top` hold it (sRGB, 0 to 1).
    pub backdrop: [f32; 3],
    pub backdrop_top: [f32; 3],
}

/// A panel's colour: `r`, `g`, `b` a little see-through.
const fn glass(r: u8, g: u8, b: u8) -> Color32 {
    const ALPHA: u32 = 224;
    const fn through(c: u8) -> u8 {
        ((c as u32 * ALPHA + 127) / 255) as u8
    }
    Color32::from_rgba_premultiplied(through(r), through(g), through(b), ALPHA as u8)
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The themes on offer, by their place in the Theme card. The first is the
/// built-in one. A theme is remembered by its name, so the order can change.
pub const THEMES: [Palette; 8] = [
    // The Unity app's: blue-black, with a blue accent.
    Palette {
        name: "Midnight",
        panel: glass(20, 20, 26),
        card: rgb(29, 29, 36),
        header: rgb(38, 38, 47),
        control: rgb(51, 51, 56),
        field: rgb(15, 15, 19),
        accent: rgb(64, 140, 242),
        text: rgb(230, 230, 237),
        hint: rgb(166, 166, 178),
        window: rgb(20, 20, 26),
        backdrop: [0.10, 0.11, 0.14],
        backdrop_top: [0.35, 0.42, 0.55],
    },
    // Plain dark grey with an orange accent.
    Palette {
        name: "Graphite",
        panel: glass(27, 27, 27),
        card: rgb(38, 38, 38),
        header: rgb(49, 49, 49),
        control: rgb(64, 64, 64),
        field: rgb(19, 19, 19),
        accent: rgb(214, 96, 36),
        text: rgb(234, 234, 234),
        hint: rgb(170, 170, 170),
        window: rgb(27, 27, 27),
        backdrop: [0.13, 0.13, 0.13],
        backdrop_top: [0.36, 0.36, 0.36],
    },
    // The lightest: middle grey all over, for a bright room.
    Palette {
        name: "Slate",
        panel: glass(54, 56, 61),
        card: rgb(66, 68, 74),
        header: rgb(78, 81, 88),
        control: rgb(97, 100, 108),
        field: rgb(39, 41, 45),
        accent: rgb(56, 134, 236),
        text: rgb(244, 244, 247),
        hint: rgb(200, 201, 208),
        window: rgb(54, 56, 61),
        backdrop: [0.27, 0.28, 0.30],
        backdrop_top: [0.52, 0.55, 0.60],
    },
    // Warm browns, the colour of the clay.
    Palette {
        name: "Clay",
        panel: glass(29, 23, 20),
        card: rgb(41, 33, 29),
        header: rgb(54, 43, 37),
        control: rgb(71, 58, 50),
        field: rgb(20, 15, 13),
        accent: rgb(196, 104, 50),
        text: rgb(240, 232, 224),
        hint: rgb(184, 170, 158),
        window: rgb(29, 23, 20),
        backdrop: [0.135, 0.105, 0.09],
        backdrop_top: [0.46, 0.38, 0.32],
    },
    Palette {
        name: "Forest",
        panel: glass(17, 23, 20),
        card: rgb(26, 35, 31),
        header: rgb(34, 46, 40),
        control: rgb(47, 61, 54),
        field: rgb(12, 17, 15),
        accent: rgb(44, 148, 96),
        text: rgb(228, 237, 231),
        hint: rgb(162, 178, 169),
        window: rgb(17, 23, 20),
        backdrop: [0.085, 0.115, 0.10],
        backdrop_top: [0.32, 0.44, 0.38],
    },
    Palette {
        name: "Lagoon",
        panel: glass(16, 22, 25),
        card: rgb(25, 33, 37),
        header: rgb(33, 44, 49),
        control: rgb(46, 59, 65),
        field: rgb(11, 16, 18),
        accent: rgb(30, 146, 158),
        text: rgb(227, 236, 238),
        hint: rgb(160, 176, 181),
        window: rgb(16, 22, 25),
        backdrop: [0.08, 0.11, 0.125],
        backdrop_top: [0.30, 0.44, 0.50],
    },
    Palette {
        name: "Plum",
        panel: glass(24, 19, 30),
        card: rgb(36, 29, 44),
        header: rgb(47, 38, 57),
        control: rgb(63, 53, 75),
        field: rgb(17, 13, 22),
        accent: rgb(148, 96, 230),
        text: rgb(235, 230, 241),
        hint: rgb(174, 165, 186),
        window: rgb(24, 19, 30),
        backdrop: [0.115, 0.09, 0.14],
        backdrop_top: [0.42, 0.35, 0.52],
    },
    // Black, for a dark room: the model is the only bright thing on screen.
    Palette {
        name: "Carbon",
        panel: glass(8, 8, 9),
        card: rgb(18, 18, 20),
        header: rgb(28, 28, 31),
        control: rgb(44, 44, 48),
        field: rgb(0, 0, 0),
        accent: rgb(64, 140, 242),
        text: rgb(240, 240, 244),
        hint: rgb(170, 170, 180),
        window: rgb(10, 10, 12),
        backdrop: [0.03, 0.03, 0.035],
        backdrop_top: [0.20, 0.22, 0.26],
    },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// The theme that is up: its place in [`THEMES`].
pub fn current() -> usize {
    CURRENT.load(Ordering::Relaxed).min(THEMES.len() - 1)
}

/// The colours of the theme that is up.
pub fn palette() -> &'static Palette {
    &THEMES[current()]
}

/// The place in [`THEMES`] of the theme called `name`, whatever its case.
pub fn named(name: &str) -> Option<usize> {
    THEMES.iter().position(|theme| theme.name.eq_ignore_ascii_case(name.trim()))
}

/// Puts the theme at `index` of [`THEMES`] up; one that is not there is the
/// built-in one. Everything painted from the next frame on is in its
/// colours. Does nothing when that theme is up already, so it can be called
/// every frame.
pub fn set(ctx: &egui::Context, index: usize) {
    let index = if index < THEMES.len() { index } else { 0 };
    if CURRENT.swap(index, Ordering::Relaxed) != index {
        visuals(ctx);
        ctx.request_repaint();
    }
}

/// Whether `backdrop` and `top` are the backdrop of one of the themes: one
/// nobody chose, which follows the theme when it changes.
pub fn stock_backdrop(backdrop: [f32; 3], top: [f32; 3]) -> bool {
    let same = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.002);
    THEMES.iter().any(|theme| same(theme.backdrop, backdrop) && same(theme.backdrop_top, top))
}

/// Dresses egui's own widgets (text boxes, tooltips, scroll bars) in the
/// theme that is up.
pub(crate) fn visuals(ctx: &egui::Context) {
    let p = palette();
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.override_text_color = Some(p.text);
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = p.window;
        v.window_stroke = crate::theme::hairline();
        v.extreme_bg_color = p.field;
        v.selection.bg_fill = p.accent.gamma_multiply(0.6);
        v.widgets.inactive.bg_fill = p.control;
        v.widgets.inactive.weak_bg_fill = p.control;
        v.widgets.hovered.bg_fill = mix(p.control, Color32::WHITE, 0.08);
        v.widgets.hovered.weak_bg_fill = mix(p.control, Color32::WHITE, 0.08);
        v.widgets.active.bg_fill = p.accent;
        v.widgets.active.weak_bg_fill = p.accent;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
            w.corner_radius = CornerRadius::same(5);
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, p.text);
        }
    });
}

/// `a` with `t` of `b` mixed in.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

// The colours of the theme that is up, under the names they had when there
// was one theme and they were constants.

#[allow(non_snake_case)]
pub fn PANEL() -> Color32 {
    palette().panel
}

#[allow(non_snake_case)]
pub fn CARD() -> Color32 {
    palette().card
}

#[allow(non_snake_case)]
pub fn HEADER() -> Color32 {
    palette().header
}

#[allow(non_snake_case)]
pub fn CONTROL() -> Color32 {
    palette().control
}

#[allow(non_snake_case)]
pub fn FIELD() -> Color32 {
    palette().field
}

#[allow(non_snake_case)]
pub fn ACCENT() -> Color32 {
    palette().accent
}

#[allow(non_snake_case)]
pub fn TEXT() -> Color32 {
    palette().text
}

#[allow(non_snake_case)]
pub fn HINT() -> Color32 {
    palette().hint
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How bright a colour is to the eye, 0 to 1 (the WCAG's relative luminance).
    fn luminance(c: Color32) -> f32 {
        let linear = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// The built-in theme is the one the app had before it had themes, to
    /// the last digit: nobody's interface changes until they pick another.
    #[test]
    fn the_first_theme_is_the_one_the_app_always_had() {
        let p = &THEMES[0];
        assert_eq!(p.panel, Color32::from_rgba_premultiplied(18, 18, 23, 224));
        assert_eq!((p.card, p.header), (rgb(29, 29, 36), rgb(38, 38, 47)));
        assert_eq!((p.control, p.field), (rgb(51, 51, 56), rgb(15, 15, 19)));
        assert_eq!((p.accent, p.text, p.hint), (rgb(64, 140, 242), rgb(230, 230, 237), rgb(166, 166, 178)));
        let options_default = ([0.10, 0.11, 0.14], [0.35, 0.42, 0.55]);
        assert_eq!((p.backdrop, p.backdrop_top), options_default);
    }

    /// Every theme can be read: its text stands out from every surface it
    /// is written on, white stands out from its accent (an active button's
    /// label), and its surfaces step up from the field to the controls.
    #[test]
    fn every_theme_can_be_read() {
        for p in &THEMES {
            let name = p.name;
            for (surface, what) in [(p.card, "card"), (p.header, "header"), (p.control, "control"), (p.field, "field")] {
                assert!(contrast(p.text, surface) >= 4.5, "{name}: text on the {what} is {:.2}", contrast(p.text, surface));
            }
            for (surface, what) in [(p.card, "card"), (p.field, "field")] {
                assert!(contrast(p.hint, surface) >= 4.5, "{name}: a hint on the {what} is {:.2}", contrast(p.hint, surface));
            }
            let on_accent = contrast(Color32::WHITE, p.accent);
            assert!(on_accent >= 3.3, "{name}: white on the accent is {on_accent:.2}");
            // An active button must not be taken for one that is not.
            assert!(contrast(p.accent, p.control) >= 1.5, "{name}: the accent on a control is {:.2}", contrast(p.accent, p.control));
            let steps = [p.field, p.card, p.header, p.control].map(luminance);
            assert!(steps.windows(2).all(|pair| pair[0] < pair[1]), "{name}: the surfaces do not step up: {steps:?}");
            assert!(luminance(p.text) > 0.7 && luminance(p.control) < 0.2, "{name}: light text on a dark surface");
            assert_eq!(p.panel.a(), 224, "{name}: the panel is a little see-through");
            assert!(p.backdrop.iter().chain(&p.backdrop_top).all(|v| (0.0..=1.0).contains(v)), "{name}: the backdrop is a colour");
        }
    }

    /// Themes are told apart by name, and a name finds its theme whatever
    /// its case; a name that is no theme's finds none.
    #[test]
    fn a_theme_is_found_by_its_name() {
        for (index, theme) in THEMES.iter().enumerate() {
            assert_eq!(named(theme.name), Some(index));
            assert_eq!(named(&theme.name.to_uppercase()), Some(index));
            assert!(!theme.name.is_empty() && theme.name.len() <= 10, "{}: it has to fit its tile", theme.name);
        }
        assert_eq!(named(" clay "), named("Clay"));
        assert_eq!(named(""), None);
        assert_eq!(named("Neon"), None);
    }

    /// Each theme's backdrop is told from a colour somebody chose; no two
    /// themes share one, so a backdrop says which theme it came with.
    #[test]
    fn a_stock_backdrop_is_one_of_the_themes() {
        for (i, theme) in THEMES.iter().enumerate() {
            assert!(stock_backdrop(theme.backdrop, theme.backdrop_top), "{}", theme.name);
            for other in &THEMES[i + 1..] {
                assert_ne!(theme.backdrop, other.backdrop, "{} and {}", theme.name, other.name);
            }
        }
        assert!(!stock_backdrop([0.5, 0.1, 0.1], THEMES[0].backdrop_top));
        assert!(!stock_backdrop(THEMES[0].backdrop, [0.9, 0.9, 0.9]), "a gradient top somebody chose");
        assert!(!stock_backdrop([f32::NAN; 3], [f32::NAN; 3]));
    }
}
