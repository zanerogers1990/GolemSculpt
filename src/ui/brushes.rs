//! The brush library in the interface: the Brushes card (every brush as a
//! named row, sorted by kind, with a right-click menu for what can be done
//! with it) and which brushes the hold-Space wheel shows.
//!
//! A brush is told from the others over its kernel without a picture of its
//! own: it shows its kernel's icon, its name, and on the wheel, where there
//! is little room for a name, its initials in the corner of its tile.

use egui::{Align2, Color32, Id, Rect, Sense, TextureId, Ui, pos2, vec2};

use super::dyntopo::WARNING;
use super::theme::{self, ACCENT, ButtonStyle};
use crate::doc::Action;
use crate::keymap::{self, Keymap};
use crate::mesh::Mesh;
use crate::sculpt::erase;
use crate::sculpt::library::{self, Ask, Library, Origin};
use crate::sculpt::{Brush, Settings};

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

/// What a right-click on a brush in the Brushes card offers: everything that
/// can be done with it besides picking it. A tile on the wheel itself comes
/// off it with a right-click (`overlay.rs`).
fn brush_menu(response: &egui::Response, library: &Library, i: usize, actions: &mut Vec<Action>) {
    response.context_menu(|ui| menu_items(ui, library, i, actions));
}

/// The menu's lines, for the right-click on a row and for the card's
/// visible \u{2026} button, which opens the same menu for the brush in hand:
/// a pen's right-click is its barrel button, which is easy to miss.
fn menu_items(ui: &mut Ui, library: &Library, i: usize, actions: &mut Vec<Action>) {
    let Some(entry) = library.entries().get(i) else { return };
    let ask = |actions: &mut Vec<Action>, ask: Ask| actions.push(Action::Brush(ask));
    {
        ui.label(egui::RichText::new(&entry.name).font(theme::bold(12.0)));
        let label = if entry.pinned { "Take Off the Space Wheel" } else { "Put On the Space Wheel" };
        if noted(ui.button(label), "menu:Wheel").clicked() {
            ask(actions, Ask::Pin(i, !entry.pinned));
            ui.close();
        }
        if entry.kernel == Brush::Smooth {
            let shift = library.shift() == i;
            let label = if shift { "Shift Smooths With This (On)" } else { "Shift Smooths With This" };
            let item = noted(ui.button(label), "menu:Shift");
            let item = sculpt_toolkit::translate::hover(item, "Holding Shift with any brush smooths with the one brush that has this on.");
            if item.clicked() {
                // Taking the mark off a brush hands Shift back to the base Smooth.
                ask(actions, Ask::ShiftUses(if shift { Library::base(Brush::Smooth) } else { i }));
                ui.close();
            }
        }
        ui.separator();
        let item = noted(ui.button("Duplicate"), "menu:Duplicate");
        if sculpt_toolkit::translate::hover(item, "Make a brush of your own that starts as this one is now.").clicked() {
            ask(actions, Ask::Duplicate(i));
            ui.close();
        }
        let back = if entry.origin == Origin::User { "Put it back as it was when you made it." } else { "Put it back as it came with the app." };
        if sculpt_toolkit::translate::hover(noted(ui.button("Reset"), "menu:Reset"), back).clicked() {
            ask(actions, Ask::Reset(i));
            ui.close();
        }
        if entry.origin != Origin::Base && noted(ui.button("Delete"), "menu:Delete").clicked() {
            ask(actions, Ask::Delete(i));
            ui.close();
        }
        // Opens a file box: never pressed by the self-check, so not noted.
        if sculpt_toolkit::translate::hover(ui.button("Export Brush..."), "Write this brush to a file you can keep or pass on.").clicked() {
            ask(actions, Ask::Export(i));
            ui.close();
        }
        ui.separator();
        if sculpt_toolkit::translate::hover(ui.button("Reset the Space Wheel"), "Put the wheel back to the brushes it started with.").clicked() {
            ask(actions, Ask::ResetWheel);
            ui.close();
        }
    }
}

/// How many brushes the Recent row keeps: the one in hand, and one line of
/// the list's columns besides.
const RECENT: usize = 1 + COLUMNS;

/// Once a frame: the brush in hand goes to the front of the brushes taken
/// up lately (by name, as the settings file keeps them).
pub fn note_in_hand(recent: &mut Vec<String>, library: &Library, s: &Settings) {
    if s.mask_mode {
        return;
    }
    let name = &library.entries()[library.selected(s)].name;
    if recent.first() == Some(name) {
        return;
    }
    recent.retain(|kept| kept != name);
    recent.insert(0, name.clone());
    recent.truncate(RECENT);
}

/// The brushes of the Recent row, by their places in the library: the ones
/// taken up lately, newest first, but not the one in hand, and not any that
/// has since been renamed or deleted.
fn recent_rows(library: &Library, s: &Settings, recent: &[String]) -> Vec<usize> {
    let in_hand = (!s.mask_mode).then(|| library.selected(s));
    recent.iter().filter_map(|name| library.named(name)).filter(|&i| Some(i) != in_hand).collect()
}

/// The brushes whose names hold `find`, whatever the case, in the library's
/// order.
fn found(library: &Library, find: &str) -> Vec<usize> {
    let find = find.trim().to_lowercase();
    let entries = library.entries();
    (0..entries.len()).filter(|&i| entries[i].name.to_lowercase().contains(&find)).collect()
}

/// The top of the card: a box to find a brush by name, and the \u{2026} button
/// with the brush in hand's menu. Gives back what is typed in the box.
fn find_row(ui: &mut Ui, library: &Library, in_hand: usize, tips: bool, actions: &mut Vec<Action>) -> String {
    let id = Id::new("brush_find");
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
    ui.horizontal(|ui| {
        let menu = 30.0;
        let width = (ui.available_width() - menu - GAP).max(40.0);
        let field = egui::TextEdit::singleline(&mut text).hint_text("Find a brush").font(theme::font(12.0)).desired_width(width);
        let field = ui.add(field);
        // Esc in the box empties it.
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            text.clear();
        }
        theme::tip(field, tips, "Type part of a brush's name to list only the brushes that have it.");
        let style = ButtonStyle { height: 24.0, ..Default::default() };
        let button = theme::tip(theme::button(ui, menu, "\u{2026}", style), tips, "The brush in hand: put it on the Space wheel, duplicate it, reset it, export it and more. The same as a right-click on its row.");
        egui::Popup::menu(&button).show(|ui| menu_items(ui, library, in_hand, actions));
    });
    ui.data_mut(|d| d.insert_temp(id, text.clone()));
    text
}

/// What a row of the list, or a tile of the wheel, says when pointed at.
/// `key` is the brush's shortcut, empty for none (keymap.rs).
pub fn tip_of(library: &Library, i: usize, key: &str) -> String {
    let entry = &library.entries()[i];
    let kernel = entry.kernel.name();
    let name = if key.is_empty() { entry.name.clone() } else { format!("{} ({key})", entry.name) };
    match entry.origin {
        Origin::Base => format!("{name}\n{}", entry.kernel.description()),
        Origin::Shipped => format!("{name}\n{}\nA {kernel} brush.", library::about(&entry.name).unwrap_or_default()),
        Origin::User => format!("{name}\nOne of your own. A {kernel} brush."),
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
             Strength is how much of it one pass takes. Undo (Ctrl+Z) brings it back."
        ),
    );
}

/// The kinds the Brushes card sorts the brushes into, by kernel, in the
/// order the card shows them. Every kernel is in one (a test says so).
const GROUPS: [(&str, &[Brush]); 5] = [
    ("Clay & Build", &[Brush::Clay, Brush::Standard, Brush::Layer, Brush::Inflate]),
    ("Smooth & Flatten", &[Brush::Smooth, Brush::Flatten]),
    ("Cut & Pinch", &[Brush::Crease, Brush::Pinch]),
    ("Move & Pose", &[Brush::Move, Brush::SnakeHook, Brush::Nudge, Brush::Pose]),
    ("Paint & Layers", &[Brush::Paint, Brush::EraseLayer]),
];

/// The brushes of one of `GROUPS`, by their places in the library: kernel by
/// kernel, each base brush first and then the ones that ship over it. The
/// user's own are under Yours instead.
fn of_group(library: &Library, kernels: &[Brush]) -> Vec<usize> {
    let entries = library.entries();
    kernels
        .iter()
        .flat_map(|&kernel| (0..entries.len()).filter(move |&i| entries[i].kernel == kernel && entries[i].origin != Origin::User))
        .collect()
}

/// One brush of the list: its kernel's icon, its name, its number key at
/// the right, and in the top right corner a dot if it is on the wheel and an
/// S if it is the one Shift uses. The marks take no room from the name.
fn row(
    ui: &mut Ui,
    width: f32,
    icon: Option<TextureId>,
    name: &str,
    lit: bool,
    (pinned, shift, key): (bool, bool, Option<String>),
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
    let gold = if lit { Color32::WHITE } else { theme::KEY };
    if shift {
        let at = pos2(right - if pinned { 6.0 } else { 0.0 }, rect.top() + 1.0);
        p.text(at, Align2::RIGHT_TOP, "S", theme::bold(8.0), gold);
    }
    let key_width = key.as_ref().map_or(0.0, |key| {
        p.text(pos2(right, rect.bottom() - 1.0), Align2::RIGHT_BOTTOM, key, theme::bold(9.0), gold).width()
    });
    if shift || key.is_some() {
        // A key longer than one figure ("Shift+T") takes the room it needs.
        right -= key_width.max(6.0) + 2.0;
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
    (library, s, keys): (&Library, &Settings, &Keymap),
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
                let key = keys.badge(&keymap::brush_id(&entry.name));
                let shift = library.shift() == i && i != Library::base(Brush::Smooth);
                let marks = (entry.pinned, shift, (!key.is_empty()).then(|| key.clone()));
                let r = row(ui, width, icon(entry.kernel.icon()), &entry.name, in_hand == Some(i), marks);
                let r = noted(r, &format!("row:{}", entry.name));
                let r = theme::tip(r, tips, &format!("{}\n{}", tip_of(library, i, &key), menu_tip(library, i)));
                if r.clicked() {
                    actions.push(Action::Brush(Ask::Select(i)));
                }
                brush_menu(&r, library, i, actions);
            }
        });
    }
}

/// The words a row's tip ends with: what its right-click menu is for.
fn menu_tip(library: &Library, i: usize) -> &'static str {
    match library.entries().get(i).is_some_and(|e| e.pinned) {
        true => "On the Space wheel. Right-click to take it off, duplicate it, reset it and more.",
        false => "Right-click to put it on the Space wheel, duplicate it, reset it and more.",
    }
}

/// A kind's heading, which folds its rows away when pressed.
fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    if theme::fold(ui, title, true, false) {
        body(ui);
    }
}

/// The Brushes card: every brush by name, sorted by kind. A tap takes one
/// in hand; its right-click menu does the rest.
pub fn card(
    ui: &mut Ui,
    (library, s, keys): (&Library, &Settings, &Keymap),
    recent: &[String],
    icon: &dyn Fn(&str) -> Option<TextureId>,
    tips: bool,
    actions: &mut Vec<Action>,
) {
    let in_hand = library.selected(s);
    let entries = library.entries();
    let mut yours: Vec<usize> = (0..entries.len()).filter(|&i| entries[i].origin == Origin::User).collect();
    yours.sort_by_key(|&i| entries[i].name.to_lowercase());
    let summary = if s.mask_mode { "Mask" } else { entries[in_hand].name.as_str() };
    theme::category(ui, "Brushes", theme::lit(ACCENT()), summary, true, |ui| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        let find = find_row(ui, library, in_hand, tips, actions);
        if !find.trim().is_empty() {
            // Only the brushes with that in their names, in one list.
            let matches = found(library, &find);
            if matches.is_empty() {
                theme::hint(ui, "No brush has that in its name.");
            }
            rows(ui, (library, s, keys), &matches, icon, tips, actions);
        } else {
            // One line, always: brushes appearing in it must never push the
            // list down under a pen that is about to tap it.
            let lately = recent_rows(library, s, recent);
            group(ui, "Recent", |ui| {
                if lately.is_empty() {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
                    let words = "The brushes you take up show here.";
                    ui.painter().text(rect.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, words, theme::font(11.0), theme::HINT());
                } else {
                    rows(ui, (library, s, keys), &lately[..lately.len().min(COLUMNS)], icon, tips, actions);
                }
            });
            for (title, kernels) in GROUPS {
                group(ui, title, |ui| rows(ui, (library, s, keys), &of_group(library, kernels), icon, tips, actions));
            }
            if !yours.is_empty() {
                group(ui, "Yours", |ui| rows(ui, (library, s, keys), &yours, icon, tips, actions));
            }
        }
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.add_space(2.0);
        // One of the user's own in hand: its name, to change.
        if !s.mask_mode && entries[in_hand].origin == Origin::User {
            rename(ui, in_hand, &entries[in_hand].name, tips, actions);
        }
        let gone = library.removed().len();
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, if gone > 0 { 2 } else { 1 }, 6.0);
            let text = "Bring in a brush from a file: one a friend sent, or one you exported or deleted.";
            if theme::tip(theme::button(ui, w, "Import Brush...", ButtonStyle::default()), tips, text).clicked() {
                actions.push(Action::Brush(Ask::Import));
            }
            if gone > 0 {
                let label = format!("Restore Shipped ({gone})");
                let text = "Bring back the brushes that came with the app and were deleted.";
                let restore = noted(theme::button(ui, w, &label, ButtonStyle::default()), "Restore");
                if theme::tip(restore, tips, text).clicked() {
                    actions.push(Action::Brush(Ask::RestoreShipped));
                }
            }
        });
        theme::about(
            ui,
            "Tap a brush to take it up; the number keys pick the ones with a number. Every brush keeps its own \
             settings, and changes stick as you make them.\n\n\
             Right-click a brush (or press \u{2026} at the top for the brush in hand) to put it on the Space wheel or \
             take it off, duplicate it as a brush of your own, reset it, delete it or export it. A dot marks a brush \
             on the wheel, an S the one Shift smooths with. Recent holds the brushes you took up last; the box at \
             the top finds a brush by name.",
        );
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recent_row_holds_the_brushes_taken_up_lately_but_not_the_one_in_hand() {
        let (mut library, mut s) = (Library::default(), Settings::default());
        let mut recent = Vec::new();
        for name in ["Clay", "Move", "Standard", "Clay", "Inflate", "Smooth", "Layer"] {
            assert!(library.select_named(name, &mut s), "{name} ships");
            note_in_hand(&mut recent, &library, &s);
        }
        assert_eq!(recent, ["Layer", "Smooth", "Inflate"], "newest first, each once, one line's worth and the one in hand");
        let names: Vec<&str> = recent_rows(&library, &s, &recent).into_iter().map(|i| library.entries()[i].name.as_str()).collect();
        assert_eq!(names, ["Smooth", "Inflate"], "the one in hand is not offered again");
        // A name no brush has any more (renamed, deleted) is left out.
        let gone = vec!["No Such Brush".to_owned(), "Move".to_owned()];
        assert_eq!(recent_rows(&library, &s, &gone).len(), 1);
    }

    #[test]
    fn the_find_box_matches_any_part_of_a_name_whatever_the_case() {
        let library = Library::default();
        let names = |find: &str| found(&library, find).into_iter().map(|i| library.entries()[i].name.clone()).collect::<Vec<_>>();
        let clay = names("  cLaY ");
        assert!(clay.contains(&"Clay".to_owned()) && clay.contains(&"Clay Strips".to_owned()), "{clay:?}");
        assert!(clay.iter().all(|name| name.to_lowercase().contains("clay")), "{clay:?}");
        assert!(names("no such brush").is_empty());
    }

    #[test]
    fn the_wheel_starts_as_it_was_and_takes_pins_beside_their_kin() {
        let mut library = Library::default();
        let name = |library: &Library, tile: Option<usize>| tile.map_or("Mask".to_owned(), |i| library.entries()[i].name.clone());
        let names = |library: &Library| wheel_tiles(library).into_iter().map(|tile| name(library, tile)).collect::<Vec<_>>();
        // The wheel of before there was a library, with Clay Strips in the
        // plain Clay's place: twelve brushes and the mask. Erase Layer and
        // Paint start off it.
        let was = ["Move", "Clay Strips", "Standard", "Layer", "Smooth", "Crease", "Pinch", "Mask", "Inflate", "Flatten", "Pose", "Snake Hook", "Nudge"];
        assert_eq!(names(&library), was);

        // A pinned brush sits after its kernel's base brush; an unpinned
        // base brush is off the wheel; the mask tile stays at the bottom.
        library.set_pinned(Library::base(Brush::Clay), true);
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

        // Reset the Space Wheel puts it back as it started.
        library.reset_pins();
        assert_eq!(names(&library), was);
    }

    #[test]
    fn every_brush_is_in_one_kind_of_the_card() {
        let kernels: Vec<Brush> = GROUPS.iter().flat_map(|(_, kernels)| kernels.iter().copied()).collect();
        for kernel in Brush::ALL {
            assert_eq!(kernels.iter().filter(|&&k| k == kernel).count(), 1, "{} is in one kind", kernel.name());
        }
        // Every brush that is not one of the user's own shows once, under its kind.
        let library = Library::default();
        let shown: Vec<usize> = GROUPS.iter().flat_map(|(_, kernels)| of_group(&library, kernels)).collect();
        assert_eq!(shown.len(), library.entries().len());
        assert_eq!(of_group(&library, &[Brush::Clay])[..2], [Library::base(Brush::Clay), library.named("Clay Strips").unwrap()]);
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
        assert!(tip_of(&library, Library::base(Brush::Clay), "2").starts_with("Clay (2)\n"));
        let trim = tip_of(&library, library.named("Trim").unwrap(), "");
        assert!(trim.starts_with("Trim\n") && trim.ends_with("A Flatten brush."), "{trim}");
        let mine = library.duplicate(library.named("Trim").unwrap(), &mut s);
        assert!(tip_of(&library, mine, "").contains("your own"));
        // A brush of one's own can have a key too, and says so.
        assert!(tip_of(&library, mine, "Shift+T").contains("(Shift+T)\n"));
    }
}
