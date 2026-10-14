//! The brush library in the interface: the Brush Library card (every brush
//! beyond the base fourteen as a named row, and what can be done with the one
//! in hand) and which brushes the hold-Space wheel shows.
//!
//! A brush is told from the others over its kernel without a picture of its
//! own: it shows its kernel's icon, its name, and on the wheel, where there
//! is little room for a name, its initials in the corner of its tile.

use egui::{Align2, Color32, Id, Rect, Sense, TextureId, Ui, pos2, vec2};

use super::dyntopo::WARNING;
use super::theme::{self, ACCENT, ButtonStyle};
use crate::doc::Action;
use crate::mesh::Mesh;
use crate::sculpt::erase;
use crate::sculpt::library::{self, Ask, Library, Origin};
use crate::sculpt::{Brush, Settings};

/// The card's colour: between the Brushes card's and the settings card's.
const CARD: Color32 = Color32::from_rgb(120, 190, 255);
const COLUMNS: usize = 2;
const GAP: f32 = 4.0;
const ROW: f32 = 24.0;
/// The sizes a row's name is tried at, largest first (see `row`). No
/// smaller: at an interface size of 75% the second is eight pixels tall.
const NAME_SIZES: [f32; 2] = [12.0, 11.0];

/// Where the card's controls were last drawn, by name, kept only while
/// something is watching: the interface's self-check (`app/ui_demo.rs`),
/// which presses them the way a pen would and has no other way to find a
/// hand-painted control. Import Brush... and Export Brush... are not among
/// them: they open file boxes, which nothing scripted may do.
#[derive(Clone, Default)]
struct Places {
    watching: bool,
    at: std::collections::BTreeMap<String, Rect>,
}

fn places<R>(ctx: &egui::Context, f: impl FnOnce(&mut Places) -> R) -> R {
    ctx.data_mut(|d| f(d.get_temp_mut_or_default::<Places>(Id::new("brush_library_places"))))
}

/// Starts or stops noting where the card's controls are.
pub fn watch(ctx: &egui::Context, on: bool) {
    places(ctx, |p| *p = Places { watching: on, ..Places::default() });
}

/// Where the control called `name` was last drawn while watched: a row by
/// `row:` and its brush's name, the others by the word on them.
pub fn place(ctx: &egui::Context, name: &str) -> Option<Rect> {
    places(ctx, |p| p.at.get(name).copied())
}

/// Notes where a control is, if anything is watching.
fn noted(response: egui::Response, name: &str) -> egui::Response {
    places(&response.ctx, |p| {
        if p.watching {
            p.at.insert(name.to_owned(), response.rect);
        }
    });
    response
}

/// The base brushes round the wheel, clockwise from the top: kin beside kin.
/// (Erase Layer and Paint have places for when they are pinned; they start
/// off the wheel.)
const WHEEL_ORDER: [Brush; Brush::COUNT] = {
    use Brush::*;
    [Move, Clay, Standard, Layer, Smooth, Crease, Pinch, Inflate, Flatten, Pose, SnakeHook, Nudge, EraseLayer, Paint]
};

/// The tiles of the brush wheel, clockwise from the top: every pinned brush
/// by its place in the library, and the mask tile (`None`) at the bottom.
/// Each kernel's brushes are together, its base brush first, so a brush
/// pinned later sits beside its kin. With the pins as they start this is
/// the wheel as it was before there was a library.
pub fn wheel_tiles(library: &Library) -> Vec<Option<usize>> {
    let entries = library.entries();
    let of_kernel = |kernel: Brush| {
        let pinned = move |&i: &usize| entries[i].pinned && entries[i].kernel == kernel;
        (0..entries.len()).filter(pinned).map(Some)
    };
    let mut tiles: Vec<Option<usize>> = WHEEL_ORDER.into_iter().flat_map(of_kernel).collect();
    // The bottom of the wheel: half-way round, on the side Inflate was on.
    tiles.insert(((tiles.len() + 2) / 2).min(tiles.len()), None);
    tiles
}

/// A brush's name as its first two initials, or its first two letters if it
/// is one word: what goes in the corner of its wheel tile.
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().unwrap_or_default();
    let letters: String = match words.next() {
        Some(second) => first.chars().take(1).chain(second.chars().take(1)).flat_map(char::to_uppercase).collect(),
        None => {
            let mut letters = first.chars();
            letters.next().into_iter().flat_map(char::to_uppercase).chain(letters.next()).collect()
        }
    };
    letters
}

/// `name`, or as much of its start as fits in `letters` with a mark that
/// there was more.
pub fn shortened(name: &str, letters: usize) -> String {
    if name.chars().count() <= letters {
        return name.to_owned();
    }
    let kept: String = name.chars().take(letters.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// What a row of the list, or a tile of the wheel, says when pointed at.
pub fn tip_of(library: &Library, i: usize) -> String {
    let entry = &library.entries()[i];
    let kernel = entry.kernel.name();
    match entry.origin {
        Origin::Base => match entry.kernel.hotkey() {
            Some(key) => format!("{} ({key})\n{}", entry.name, entry.kernel.description()),
            None => format!("{}\n{}", entry.name, entry.kernel.description()),
        },
        Origin::Shipped => {
            format!("{}\n{}\nA {kernel} brush.", entry.name, library::about(&entry.name).unwrap_or_default())
        }
        Origin::User => format!("{}\nOne of your own. A {kernel} brush.", entry.name),
    }
}

/// Under Erase Layer in the settings card: which layer a stroke would rub
/// out or, in the warning colour, why there is none for it to and what to
/// do first. The same words the status line has when the stroke is refused.
pub fn erase_layer_note(ui: &mut Ui, mesh: &Mesh) {
    if let Some(why) = erase::refusal(mesh) {
        ui.add(egui::Label::new(egui::RichText::new(why).font(theme::font(11.0)).italics().color(WARNING)).wrap());
        return;
    }
    let name = mesh.layers.as_ref().and_then(|layers| layers.recording()).map(|row| row.name).unwrap_or_default();
    theme::hint(
        ui,
        &format!(
            "Rubs \"{name}\", the layer that is recording, out under the ring, back to the model underneath. \
             Strength is how much of it one pass takes. Undo (Z) brings it back."
        ),
    );
}

/// One brush of the list: its kernel's icon, its name, and in the top right
/// corner a dot if it is on the wheel and an S if it is the one Shift uses.
/// The dot sits above the line of the name and takes no room from it.
fn row(
    ui: &mut Ui,
    width: f32,
    icon: Option<TextureId>,
    name: &str,
    lit: bool,
    (pinned, shift): (bool, bool),
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
    let ink = if lit { Color32::WHITE } else { theme::TEXT() };
    // What is drawn beside the name is fainter than it, except on the lit row.
    let faint = |alpha: u8| if lit { Color32::WHITE } else { Color32::from_white_alpha(alpha) };
    if let Some(icon) = icon {
        let at = Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(16.0, 16.0));
        p.image(icon, at, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), faint(210));
    }
    let mut right = rect.right() - 5.0;
    if pinned {
        p.circle_filled(pos2(right - 1.0, rect.top() + 5.0), 2.0, faint(120));
    }
    if shift {
        let at = pos2(right - if pinned { 6.0 } else { 0.0 }, rect.top() + 1.0);
        p.text(at, Align2::RIGHT_TOP, "S", theme::bold(8.0), if lit { Color32::WHITE } else { theme::KEY });
        right -= 8.0;
    }
    let left = rect.left() + 26.0;
    let fits = |text: &str, size: f32| p.layout_no_wrap(text.to_owned(), theme::font(size), ink).size().x <= right - left;
    // A name a little too long for the row at the list's size is written a
    // size smaller; only one too long even for that is cut short.
    let size = NAME_SIZES.into_iter().find(|&size| fits(name, size)).unwrap_or(NAME_SIZES[NAME_SIZES.len() - 1]);
    let letters = name.chars().count();
    let shown = (0..=letters).rev().map(|n| shortened(name, n)).find(|text| fits(text, size)).unwrap_or_default();
    p.text(pos2(left, rect.center().y), Align2::LEFT_CENTER, shown, theme::font(size), ink);
    response
}

/// The rows for `brushes` (places in the library), in columns.
fn rows(
    ui: &mut Ui,
    (library, s): (&Library, &Settings),
    brushes: &[usize],
    icon: &dyn Fn(&str) -> Option<TextureId>,
    tips: bool,
    actions: &mut Vec<Action>,
) {
    let width = theme::row_width(ui, COLUMNS, GAP);
    let in_hand = (!s.mask_mode).then(|| library.selected(s));
    for line in brushes.chunks(COLUMNS) {
        ui.horizontal(|ui| {
            for &i in line {
                let entry = &library.entries()[i];
                let marks = (entry.pinned, library.shift() == i && i != Library::base(Brush::Smooth));
                let r = row(ui, width, icon(entry.kernel.icon()), &entry.name, in_hand == Some(i), marks);
                let r = noted(r, &format!("row:{}", entry.name));
                if theme::tip(r, tips, &tip_of(library, i)).clicked() {
                    actions.push(Action::Brush(Ask::Select(i)));
                }
            }
        });
    }
}

/// The name box of one of the user's brushes. The text being typed is kept
/// between frames with the brush it is for; it becomes the brush's name
/// when the box is left, unless that was with Esc.
fn rename(ui: &mut Ui, i: usize, name: &str, tips: bool, actions: &mut Vec<Action>) {
    let id = Id::new("brush_rename");
    let (mut of, mut text) = ui.data(|d| d.get_temp::<(String, String)>(id)).unwrap_or_default();
    if of != name {
        (of, text) = (name.to_owned(), name.to_owned());
    }
    let field = noted(theme::labelled_field(ui, "Name", &mut text), "Name");
    let field = theme::tip(field, tips, "Type a new name and press Enter.");
    if field.lost_focus() {
        let backed_out = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if !backed_out && text.trim() != name && !text.trim().is_empty() {
            actions.push(Action::Brush(Ask::Rename(i, text.trim().to_owned())));
        }
        // Whatever comes of it, the box shows the brush's name next.
        of.clear();
    }
    ui.data_mut(|d| d.insert_temp(id, (of, text)));
}

/// What can be done with the brush in hand.
fn manage(ui: &mut Ui, library: &Library, i: usize, tips: bool, actions: &mut Vec<Action>) {
    let entry = &library.entries()[i];
    let ask = |ask: Ask| Action::Brush(ask);
    theme::heading(ui, &entry.name);
    if entry.origin == Origin::User {
        rename(ui, i, &entry.name, tips, actions);
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let w = theme::row_width(ui, 3, GAP);
        let text = "Make a brush of your own that starts as this one is now. The copy is yours to rename and change.";
        let duplicate = noted(theme::button(ui, w, "Duplicate", ButtonStyle::default()), "Duplicate");
        if theme::tip(duplicate, tips, text).clicked() {
            actions.push(ask(Ask::Duplicate(i)));
        }
        let text = match entry.origin {
            Origin::User => "Put this brush back as it was when you made it.",
            _ => "Put this brush back as it came with the app.",
        };
        let reset = noted(theme::button(ui, w, "Reset", ButtonStyle::default()), "Reset");
        if theme::tip(reset, tips, text).clicked() {
            actions.push(ask(Ask::Reset(i)));
        }
        let base = entry.origin == Origin::Base;
        let text = if base { "The fourteen base brushes stay." } else { "Take this brush out of the library." };
        let delete = ui.add_enabled_ui(!base, |ui| theme::button(ui, w, "Delete", ButtonStyle::default())).inner;
        if theme::tip(noted(delete, "Delete"), tips, text).clicked() && !base {
            actions.push(ask(Ask::Delete(i)));
        }
    });
    let mut pinned = entry.pinned;
    let label = if pinned { "On the Space Wheel" } else { "On the Space Wheel (Off)" };
    let text = "On: this brush has a tile on the wheel that opens while Space is held.";
    let tick = noted(theme::checkbox(ui, &mut pinned, label, ACCENT()), "Pin");
    if theme::tip(tick, tips, text).changed() {
        actions.push(ask(Ask::Pin(i, pinned)));
    }
    if entry.kernel == Brush::Smooth {
        let mut shift = library.shift() == i;
        let label = if shift { "Shift Uses This Brush" } else { "Shift Uses This Brush (Off)" };
        let text = "Holding Shift with any brush smooths with the one brush that has this on.";
        let tick = noted(theme::checkbox(ui, &mut shift, label, ACCENT()), "Shift");
        if theme::tip(tick, tips, text).changed() {
            // Taking the mark off a brush hands Shift back to the base Smooth.
            actions.push(ask(Ask::ShiftUses(if shift { i } else { Library::base(Brush::Smooth) })));
        }
    }
}

/// The Brush Library card.
pub fn card(
    ui: &mut Ui,
    library: &Library,
    s: &Settings,
    icon: &dyn Fn(&str) -> Option<TextureId>,
    tips: bool,
    actions: &mut Vec<Action>,
) {
    let in_hand = library.selected(s);
    let entries = library.entries();
    let of = |origin: Origin| (0..entries.len()).filter(|&i| entries[i].origin == origin).collect::<Vec<usize>>();
    let (shipped, mut yours) = (of(Origin::Shipped), of(Origin::User));
    yours.sort_by_key(|&i| entries[i].name.to_lowercase());
    let summary = if entries[in_hand].origin == Origin::Base || s.mask_mode { "" } else { entries[in_hand].name.as_str() };
    theme::category(ui, "Brush Library", CARD, summary, true, |ui| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        rows(ui, (library, s), &shipped, icon, tips, actions);
        if !yours.is_empty() {
            theme::heading(ui, "Yours");
            rows(ui, (library, s), &yours, icon, tips, actions);
        }
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.add_space(2.0);
        if !s.mask_mode {
            manage(ui, library, in_hand, tips, actions);
        }
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let text = "Bring in a brush from a file: one a friend sent, or one you exported or deleted.";
            if theme::tip(theme::button(ui, w, "Import Brush...", ButtonStyle::default()), tips, text).clicked() {
                actions.push(Action::Brush(Ask::Import));
            }
            let text = "Write the brush in hand to a file you can keep or pass on.";
            if theme::tip(theme::button(ui, w, "Export Brush...", ButtonStyle::default()), tips, text).clicked() {
                actions.push(Action::Brush(Ask::Export(in_hand)));
            }
        });
        let gone = library.removed().len();
        if gone > 0 {
            let label = format!("Restore Shipped Brushes ({gone})");
            let text = "Bring back the brushes that came with the app and were deleted.";
            let restore = noted(theme::button(ui, ui.available_width(), &label, ButtonStyle::default()), "Restore");
            if theme::tip(restore, tips, text).clicked() {
                actions.push(Action::Brush(Ask::RestoreShipped));
            }
        }
        theme::hint(ui, "Every brush keeps its own settings; changes stick as you make them. A dot marks a brush on the Space wheel, an S the one Shift smooths with.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_starts_as_it_was_and_takes_pins_beside_their_kin() {
        let mut library = Library::default();
        let name = |library: &Library, tile: Option<usize>| tile.map_or("Mask".to_owned(), |i| library.entries()[i].name.clone());
        let names = |library: &Library| wheel_tiles(library).into_iter().map(|tile| name(library, tile)).collect::<Vec<_>>();
        // The wheel of before there was a library: the twelve of then and
        // the mask. Erase Layer and Paint, the last two, start off it.
        let was = ["Move", "Clay", "Standard", "Layer", "Smooth", "Crease", "Pinch", "Mask", "Inflate", "Flatten", "Pose", "Snake Hook", "Nudge"];
        assert_eq!(names(&library), was);

        // A pinned brush sits after its kernel's base brush; an unpinned
        // base brush is off the wheel; the mask tile stays at the bottom.
        library.set_pinned(library.named("Clay Strips").unwrap(), true);
        library.set_pinned(library.named("Trim").unwrap(), true);
        library.set_pinned(Library::base(Brush::Pose), false);
        let now = names(&library);
        let now: Vec<&str> = now.iter().map(String::as_str).collect();
        assert_eq!(
            now,
            ["Move", "Clay", "Clay Strips", "Standard", "Layer", "Smooth", "Crease", "Mask", "Pinch", "Inflate", "Flatten", "Trim", "Snake Hook", "Nudge"]
        );

        // Any number of pins, none included: every pinned brush once, and
        // always the one mask tile.
        for entry in 0..library.entries().len() {
            library.set_pinned(entry, true);
        }
        let all = wheel_tiles(&library);
        assert_eq!(all.len(), library.entries().len() + 1);
        assert_eq!(all.iter().filter(|tile| tile.is_none()).count(), 1);
        for entry in 0..library.entries().len() {
            library.set_pinned(entry, false);
        }
        assert_eq!(wheel_tiles(&library), [None]);
    }

    #[test]
    fn a_brush_is_told_apart_by_its_initials_and_a_name_that_fits() {
        assert_eq!(initials("Clay Strips"), "CS");
        assert_eq!(initials("smooth keep volume"), "SK");
        assert_eq!(initials("Trim"), "Tr");
        assert_eq!(initials("x"), "X");
        assert_eq!(initials(""), "");
        assert_eq!(shortened("Rake", 9), "Rake");
        assert_eq!(shortened("Smooth Keep Volume", 9), "Smooth K\u{2026}");
        assert_eq!(shortened("Smooth Keep Volume", 8), "Smooth\u{2026}");
        assert_eq!(shortened("Trim", 0), "\u{2026}");
        // Every name that ships fits its row whole: none is longer than the
        // longest that does at the list's own size ("Smooth Valleys").
        let library = Library::default();
        for entry in library.entries() {
            assert!(entry.name.chars().count() <= 14, "{} is too long for its row", entry.name);
        }
        // The shipped brushes over one kernel have different initials.
        for a in library.entries() {
            for b in library.entries() {
                let same = a.name != b.name && a.kernel == b.kernel && initials(&a.name) == initials(&b.name);
                assert!(!same || a.origin == Origin::Base || b.origin == Origin::Base, "{} and {} look the same on the wheel", a.name, b.name);
            }
        }
    }

    #[test]
    fn every_brush_says_what_it_is_when_pointed_at() {
        let mut library = Library::default();
        let mut s = Settings::default();
        assert!(tip_of(&library, Library::base(Brush::Clay)).starts_with("Clay (2)\n"));
        let trim = tip_of(&library, library.named("Trim").unwrap());
        assert!(trim.starts_with("Trim\n") && trim.ends_with("A Flatten brush."), "{trim}");
        let mine = library.duplicate(library.named("Trim").unwrap(), &mut s);
        assert!(tip_of(&library, mine).contains("your own"));
    }
}
