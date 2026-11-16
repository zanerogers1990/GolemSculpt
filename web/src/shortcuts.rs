//! The keyboard shortcuts a person can change, as on the desktop: the
//! desktop's own key map (`src/keymap.rs`, compiled here as it is, with its
//! commands, the keys they ship with and the ZBrush set), and the Settings
//! tab's Keyboard Shortcuts card, ported from the desktop's
//! (`src/ui/shortcuts.rs`). The changes are kept in the browser
//! (`golem.pref`, key "shortcuts", as the desktop's settings file keeps them).
//!
//! The card lists the commands the web version has. A key pressed is turned
//! into what it is on before the rest of the app sees it
//! (`eframe::App::raw_input_hook`): a command the app already had a key for
//! has the key pressed changed into that key, so the code behind it (the
//! hold keys, the number keys, the tools' keys) is untouched; the rest are
//! carried out here; and a key whose command moved away does nothing.
//!
//! A browser cannot tell the keypad's number keys from the others, so the
//! commands the desktop ships on the keypad (the axis views) need another
//! key to work here.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use eframe::egui::{self, Color32, Id, Key, Modifiers};
use winit::keyboard::KeyCode;

use super::masking::MaskOp;
use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::{GolemApp, Mode};
use crate::keymap::{self, Chord, Keymap, Listed};
use crate::sculpt::library::Library;

/// The key the browser keeps the changes under.
const PREF: &str = "shortcuts";

/// egui's keys and the key map's, one for one where both have the key.
const KEYS: &[(Key, KeyCode)] = &[
    (Key::A, KeyCode::KeyA),
    (Key::B, KeyCode::KeyB),
    (Key::C, KeyCode::KeyC),
    (Key::D, KeyCode::KeyD),
    (Key::E, KeyCode::KeyE),
    (Key::F, KeyCode::KeyF),
    (Key::G, KeyCode::KeyG),
    (Key::H, KeyCode::KeyH),
    (Key::I, KeyCode::KeyI),
    (Key::J, KeyCode::KeyJ),
    (Key::K, KeyCode::KeyK),
    (Key::L, KeyCode::KeyL),
    (Key::M, KeyCode::KeyM),
    (Key::N, KeyCode::KeyN),
    (Key::O, KeyCode::KeyO),
    (Key::P, KeyCode::KeyP),
    (Key::Q, KeyCode::KeyQ),
    (Key::R, KeyCode::KeyR),
    (Key::S, KeyCode::KeyS),
    (Key::T, KeyCode::KeyT),
    (Key::U, KeyCode::KeyU),
    (Key::V, KeyCode::KeyV),
    (Key::W, KeyCode::KeyW),
    (Key::X, KeyCode::KeyX),
    (Key::Y, KeyCode::KeyY),
    (Key::Z, KeyCode::KeyZ),
    (Key::Num0, KeyCode::Digit0),
    (Key::Num1, KeyCode::Digit1),
    (Key::Num2, KeyCode::Digit2),
    (Key::Num3, KeyCode::Digit3),
    (Key::Num4, KeyCode::Digit4),
    (Key::Num5, KeyCode::Digit5),
    (Key::Num6, KeyCode::Digit6),
    (Key::Num7, KeyCode::Digit7),
    (Key::Num8, KeyCode::Digit8),
    (Key::Num9, KeyCode::Digit9),
    (Key::F1, KeyCode::F1),
    (Key::F2, KeyCode::F2),
    (Key::F3, KeyCode::F3),
    (Key::F4, KeyCode::F4),
    (Key::F5, KeyCode::F5),
    (Key::F6, KeyCode::F6),
    (Key::F7, KeyCode::F7),
    (Key::F8, KeyCode::F8),
    (Key::F9, KeyCode::F9),
    (Key::F10, KeyCode::F10),
    (Key::F11, KeyCode::F11),
    (Key::F12, KeyCode::F12),
    (Key::Slash, KeyCode::Slash),
    (Key::Backslash, KeyCode::Backslash),
    (Key::Comma, KeyCode::Comma),
    (Key::Period, KeyCode::Period),
    (Key::Semicolon, KeyCode::Semicolon),
    (Key::Quote, KeyCode::Quote),
    (Key::Backtick, KeyCode::Backquote),
    (Key::Minus, KeyCode::Minus),
    (Key::Equals, KeyCode::Equal),
    (Key::OpenBracket, KeyCode::BracketLeft),
    (Key::CloseBracket, KeyCode::BracketRight),
    (Key::Insert, KeyCode::Insert),
    (Key::Delete, KeyCode::Delete),
    (Key::Home, KeyCode::Home),
    (Key::End, KeyCode::End),
    (Key::PageUp, KeyCode::PageUp),
    (Key::PageDown, KeyCode::PageDown),
    (Key::ArrowUp, KeyCode::ArrowUp),
    (Key::ArrowDown, KeyCode::ArrowDown),
    (Key::ArrowLeft, KeyCode::ArrowLeft),
    (Key::ArrowRight, KeyCode::ArrowRight),
    (Key::Space, KeyCode::Space),
    (Key::Tab, KeyCode::Tab),
    (Key::Enter, KeyCode::Enter),
    (Key::Escape, KeyCode::Escape),
    (Key::Backspace, KeyCode::Backspace),
];

fn code_of(key: Key) -> Option<KeyCode> {
    KEYS.iter().find(|(k, _)| *k == key).map(|(_, c)| *c)
}

fn key_of(code: KeyCode) -> Option<Key> {
    KEYS.iter().find(|(_, c)| *c == code).map(|(k, _)| *k)
}

fn chord_of(key: Key, m: Modifiers) -> Option<Chord> {
    Chord::new(code_of(key)?, m.ctrl || m.command, m.shift, m.alt)
}

/// The commands of the desktop's key map the web version carries out, and
/// for each the key the web app's own code already answers to, if it has
/// one (a key pressed for that command is turned into this one).
const WEB: &[(&str, Option<(Key, Modifiers)>)] = &[
    ("hold_size", Some((Key::S, Modifiers::NONE))),
    ("hold_strength", Some((Key::F, Modifiers::NONE))),
    ("hold_focal", Some((Key::D, Modifiers::NONE))),
    ("paint", Some((Key::P, Modifiers::NONE))),
    ("eyedropper", Some((Key::I, Modifiers::NONE))),
    ("mask_mode", Some((Key::M, Modifiers::NONE))),
    ("mirror_x", Some((Key::X, Modifiers::NONE))),
    ("mirror_y", None),
    ("mirror_z", None),
    ("transform", Some((Key::W, Modifiers::NONE))),
    ("remesh", None),
    ("decimate", None),
    ("subdivide", Some((Key::D, Modifiers::COMMAND))),
    ("level_up", Some((Key::PageUp, Modifiers::NONE))),
    ("level_down", Some((Key::PageDown, Modifiers::NONE))),
    ("mask_invert", None),
    ("mask_clear", None),
    ("mask_grow", None),
    ("mask_shrink", None),
    ("mask_blur", None),
    ("mask_sharpen", None),
    ("frame", None),
    ("ortho", None),
    ("view_front", None),
    ("view_back", None),
    ("view_right", None),
    ("view_left", None),
    ("view_top", None),
    ("view_bottom", None),
    ("wireframe", None),
    ("undo", Some((Key::Z, Modifiers::COMMAND))),
    ("redo", Some((Key::Y, Modifiers::COMMAND))),
];

/// What the key map holds here, and the card's "press a key".
struct Shortcuts {
    keys: Keymap,
    /// The brushes and tools the map was made for, by their ids.
    made_for: Vec<String>,
    /// The command waiting for its new key.
    capture: Option<String>,
    /// For each key held down, what its press was turned into (`None`:
    /// nothing), so its release goes the same way.
    held: HashMap<Key, Option<(Key, Modifiers)>>,
    /// Commands pressed, to be carried out in the frame.
    run: Vec<String>,
    /// What a capture did, for the status line.
    said: Option<String>,
    loaded: bool,
}

thread_local! {
    static STATE: RefCell<Shortcuts> = RefCell::new(Shortcuts {
        keys: Keymap::default(),
        made_for: Vec::new(),
        capture: None,
        held: HashMap::new(),
        run: Vec::new(),
        said: None,
        loaded: false,
    });
}

/// Whether the web version carries out command `id`.
fn on_web(id: &str) -> bool {
    id.starts_with("brush:") || id.starts_with("tool:") || WEB.iter().any(|(w, _)| *w == id)
}

/// The key the web app's own code answers to for command `id`.
fn web_key(id: &str, keys: &Keymap) -> Option<(Key, Modifiers)> {
    if let Some((_, key)) = WEB.iter().find(|(w, _)| *w == id) {
        return *key;
    }
    // A brush or tool the app has a key for answers to the key it ships with.
    if id.starts_with("brush:") || id.starts_with("tool:") {
        let shipped = keys.commands().iter().find(|c| c.id == id)?.shipped.first().copied()?;
        if !shipped.ctrl && !shipped.alt && !shipped.shift {
            return Some((key_of(shipped.key.code())?, Modifiers::NONE));
        }
    }
    None
}

fn save(keys: &Keymap) {
    let text = serde_json::to_string(&keys.changes()).unwrap_or_default();
    crate::browser::set_pref(PREF, &text);
}

/// What a press of `key` with `mods` becomes: `Ok(Some(..))` the key it is
/// turned into, `Ok(None)` dropped, `Err(id)` a command to carry out here.
fn route(s: &Shortcuts, key: Key, mods: Modifiers) -> Result<Option<(Key, Modifiers)>, String> {
    let chord = chord_of(key, mods);
    if let Some(id) = chord.as_ref().and_then(|c| s.keys.command(c)).filter(|id| on_web(id)) {
        return match web_key(id, &s.keys) {
            Some(to) => Ok(Some(to)),
            None => Err(id.to_owned()),
        };
    }
    // Not a command's now: a key the app answers to for a command that has
    // moved away does nothing.
    let answers = WEB.iter().filter_map(|(_, k)| *k).chain(
        s.keys.commands().iter().filter(|c| c.id.contains(':')).filter_map(|c| web_key(&c.id, &s.keys)),
    );
    let same = |(k, m): (Key, Modifiers)| k == key && m.command == (mods.ctrl || mods.command) && m.shift == mods.shift && m.alt == mods.alt;
    let moved = answers.filter(|&k| same(k)).count() > 0;
    // Ctrl + Shift + Z is the app's other key for Redo.
    let redo_too = key == Key::Z && (mods.ctrl || mods.command) && mods.shift;
    if moved || redo_too { Ok(None) } else { Ok(Some((key, mods))) }
}

impl GolemApp {
    /// The brushes and hosted tools as the key map lists them (the
    /// desktop's `keys::listed`).
    fn listed(&self) -> (Vec<(String, Option<Chord>)>, Vec<(&'static str, &'static str, Option<Chord>)>) {
        let entries = self.sculptor.library.entries();
        let brushes = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let base = Library::base(entry.kernel) == i;
                let key = base
                    .then(|| entry.kernel.hotkey())
                    .flatten()
                    .filter(char::is_ascii_digit)
                    .and_then(|digit| Chord::parse(&digit.to_string()));
                (entry.name.clone(), key)
            })
            .collect();
        let mut tools = Vec::new();
        let mut i = 0;
        while let Some(info) = self.tubes.info(i) {
            let key = info.hotkey.and_then(|k| Chord::new(code_of(k)?, false, false, false));
            tools.push((info.id, info.name, key));
            i += 1;
        }
        (brushes, tools)
    }

    /// The key map, made for the brushes and tools as they are now, with
    /// the changes kept in the browser (the desktop's `keys::refresh`).
    fn shortcuts_refresh(&self, s: &mut Shortcuts) {
        let (brushes, tools) = self.listed();
        let wanted: Vec<String> = brushes
            .iter()
            .map(|(name, _)| keymap::brush_id(name))
            .chain(tools.iter().map(|(id, _, _)| keymap::tool_id(id)))
            .collect();
        if !s.loaded {
            s.loaded = true;
            s.keys = Keymap::new(&brushes, &tools);
            let kept: BTreeMap<String, Vec<String>> = serde_json::from_str(&crate::browser::pref(PREF)).unwrap_or_default();
            s.keys.take_changes(&kept);
            s.made_for = wanted;
        } else if s.made_for != wanted {
            s.keys = s.keys.renewed(&brushes, &tools);
            s.made_for = wanted;
        }
    }

    /// Before each frame (`eframe::App::raw_input_hook`): each key pressed
    /// becomes what the key map has it on, and the card's "press a key"
    /// takes the next one.
    pub(super) fn shortcut_keys(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            self.shortcuts_refresh(&mut s);
            let typing = ctx.egui_wants_keyboard_input();
            let events = std::mem::take(&mut raw.events);
            for event in events {
                let egui::Event::Key { key, physical_key, pressed, repeat, modifiers } = event else {
                    raw.events.push(event);
                    continue;
                };
                if pressed && s.capture.is_some() {
                    if !repeat {
                        capture(&mut s, key, modifiers);
                        ctx.request_repaint();
                    }
                    continue;
                }
                if typing && !s.held.contains_key(&key) {
                    raw.events.push(event);
                    continue;
                }
                let to = if pressed {
                    let to = match route(&s, key, modifiers) {
                        Ok(to) => to,
                        Err(id) => {
                            if !repeat {
                                s.run.push(id);
                            }
                            None
                        }
                    };
                    s.held.insert(key, to);
                    to
                } else {
                    s.held.remove(&key).unwrap_or(Some((key, modifiers)))
                };
                if let Some((key, mods)) = to {
                    // Ctrl is the browser's Command too.
                    let modifiers = Modifiers { ctrl: mods.ctrl || mods.command, command: mods.ctrl || mods.command, ..mods };
                    let physical_key = if to == Some((key, mods)) { physical_key } else { Some(key) };
                    raw.events.push(egui::Event::Key { key, physical_key, pressed, repeat, modifiers });
                }
            }
        });
    }

    /// Carries out the commands pressed that the app had no key of its own
    /// for, as the desktop's `run_key_command` does; once a frame.
    pub(super) fn shortcut_commands(&mut self) {
        let (run, said) = STATE.with(|state| {
            let mut s = state.borrow_mut();
            (std::mem::take(&mut s.run), s.said.take())
        });
        if let Some(said) = said {
            self.status = said;
        }
        for id in run {
            self.run_command(&id);
        }
    }

    fn run_command(&mut self, id: &str) {
        let idle = self.drag == super::Drag::None && !self.sculptor.is_active();
        if !idle {
            return;
        }
        match id {
            "mirror_y" | "mirror_z" => {
                let axis = if id == "mirror_y" { 1 } else { 2 };
                let s = &mut self.sculptor.settings.symmetry;
                s[axis] = !s[axis];
                let name = ["Mirror X", "Mirror Y", "Mirror Z"][axis];
                self.status = if s[axis] { name.to_owned() } else { "Mirror Off".to_owned() };
            }
            "remesh" => self.remesh(),
            "decimate" => self.decimate(),
            "mask_invert" => self.mask_op(MaskOp::Invert),
            "mask_clear" => self.mask_op(MaskOp::Clear),
            "mask_grow" => self.mask_op(MaskOp::Grow),
            "mask_shrink" => self.mask_op(MaskOp::Shrink),
            "mask_blur" => self.mask_op(MaskOp::Blur),
            "mask_sharpen" => self.mask_op(MaskOp::Sharpen),
            "frame" => self.frame_model(true),
            "ortho" => self.camera.ortho = !self.camera.ortho,
            "view_front" => self.camera.snap_to(0.0, 0.0, None),
            "view_back" => self.camera.snap_to(180.0, 0.0, None),
            "view_right" => self.camera.snap_to(-90.0, 0.0, None),
            "view_left" => self.camera.snap_to(90.0, 0.0, None),
            "view_top" => self.camera.snap_to(0.0, 90.0, None),
            "view_bottom" => self.camera.snap_to(0.0, -90.0, None),
            "wireframe" => {
                let on = !self.look.wire;
                self.set_wire(on);
                self.status = if on { "Wireframe".into() } else { "Wireframe Off".into() };
            }
            other => {
                if let Some(name) = other.strip_prefix("brush:") {
                    let (library, s) = (&mut self.sculptor.library, &mut self.sculptor.settings);
                    let Some(i) = library.entries().iter().position(|e| e.name == name) else { return };
                    library.select(i, s);
                    s.mask_mode = false;
                    if self.mode == Mode::Transform {
                        self.mode = Mode::Sculpt;
                    }
                    self.status = format!("{name} brush.");
                } else if let Some(tool) = other.strip_prefix("tool:") {
                    if let Some(i) = self.tubes.index_of(tool) {
                        self.pick_tool(i);
                    }
                }
            }
        }
    }

    /// The key command `id` is on now, short, for a button's corner.
    pub(super) fn shortcut_badge(&self, id: &str) -> String {
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            self.shortcuts_refresh(&mut s);
            s.keys.badge(id)
        })
    }

    /// The Settings tab's Keyboard Shortcuts card (the desktop's).
    pub(super) fn shortcuts_card(&mut self, ui: &mut egui::Ui) {
        let tips = self.ui.tips;
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            self.shortcuts_refresh(&mut s);
            let before = s.keys.clone();
            card(ui, &mut s, tips);
            if s.keys != before {
                save(&s.keys);
            }
        });
    }
}

/// The card is waiting for a key: this one is it (the desktop's
/// `shortcut_capture`). Esc stops waiting, Backspace or Delete takes the
/// command's key away, and a key kept for the app is refused with a word why.
fn capture(s: &mut Shortcuts, key: Key, mods: Modifiers) {
    let Some(id) = s.capture.take() else { return };
    let name = s.keys.commands().iter().find(|c| c.id == id).map_or(id.clone(), |c| c.name.clone());
    let said = match key {
        Key::Escape => None,
        Key::Backspace | Key::Delete => {
            s.keys.set(&id, None);
            Some(format!("{name} has no key now."))
        }
        _ => match chord_of(key, mods) {
            None => Some(format!("That key cannot be a shortcut. {name} keeps its key.")),
            Some(chord) => match keymap::reserved(&chord) {
                Some(why) => Some(format!("{why} {name} keeps its key.")),
                None => {
                    let took = s.keys.set(&id, Some(chord));
                    let was = took
                        .and_then(|other| s.keys.commands().iter().find(|c| c.id == other).map(|c| c.name.clone()))
                        .map_or(String::new(), |other| format!(" It was {other}'s, which has no key now."));
                    Some(format!("{name} is on {} now.{was}", chord.text()))
                }
            },
        },
    };
    save(&s.keys);
    s.said = said;
}

// ---- the card (the desktop's ui/shortcuts.rs) -------------------------------------

/// What the pen, mouse and keys held do here, and the keys kept for the
/// app: the desktop's list, as the web version has them.
const FIXED: &[(&str, &[(&str, &str)])] = &[
    ("Sculpting", &[
        ("Left drag on the model", "Sculpt"),
        ("Ctrl + drag, or right drag", "Invert the brush"),
        ("Shift + drag", "Smooth"),
    ]),
    ("Brush", &[("[ / ]", "Brush smaller / bigger"), ("Space (hold)", "Brush wheel")]),
    ("Tools", &[("Enter", "Finish: fuse, accept, build"), ("Esc", "Cancel, or put the tool away")]),
    ("View", &[
        ("Left drag on the background", "Orbit"),
        ("Alt + drag", "Orbit"),
        ("Middle drag", "Pan"),
        ("Shift + drag off the model", "Pan"),
        ("Scroll", "Zoom"),
        ("Ctrl + = / Ctrl + -", "Interface size"),
    ]),
    ("Help", &[("F1", "The welcome card")]),
];

const COLOR: Color32 = Color32::from_rgb(230, 204, 102);
const RESET_ALL_SECONDS: f64 = 4.0;

fn keys_text(keys: &[Chord]) -> String {
    if keys.is_empty() { "\u{2014}".to_owned() } else { keys.iter().map(Chord::text).collect::<Vec<_>>().join(", ") }
}

fn listed(c: &Listed, keys: &Keymap, find: &str) -> bool {
    if !on_web(&c.id) {
        return false;
    }
    let find = find.trim().to_lowercase();
    if find.is_empty() {
        return c.group != "Brushes" || !keys.keys(&c.id).is_empty();
    }
    c.name.to_lowercase().contains(&find) || keys_text(&keys.keys(&c.id)).to_lowercase().contains(&find)
}

fn card(ui: &mut egui::Ui, s: &mut Shortcuts, tips: bool) {
    let changed = s.keys.changes().len();
    let summary = match changed {
        0 => String::new(),
        1 => "1 changed".to_owned(),
        n => format!("{n} changed"),
    };
    theme::category(ui, "Keyboard Shortcuts", COLOR, &summary, false, |ui| {
        theme::hint(
            ui,
            "Click a key to change it, then press the new one, with Ctrl, Shift or Alt if you like. Esc leaves it as \
             it was; Backspace takes the key away. A key another command had is taken from it.",
        );
        let find_id = Id::new("shortcut_find");
        let mut find = ui.data(|d| d.get_temp::<String>(find_id)).unwrap_or_default();
        presets(ui, &mut s.keys, tips);
        let field = egui::TextEdit::singleline(&mut find)
            .hint_text("Find a command or a brush")
            .font(theme::font(12.0))
            .desired_width(ui.available_width());
        let field = ui.add(field);
        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            find.clear();
        }
        ui.data_mut(|d| d.insert_temp(find_id, find.clone()));

        let commands: Vec<Listed> = s.keys.commands().to_vec();
        for group in keymap::GROUPS {
            let rows: Vec<&Listed> = commands.iter().filter(|c| c.group == *group && listed(c, &s.keys, &find)).collect();
            if rows.is_empty() && !(*group == "Brushes" && find.trim().is_empty()) {
                continue;
            }
            theme::heading(ui, group);
            if *group == "Brushes" && find.trim().is_empty() {
                theme::hint(ui, "Type a brush's name above to give it a key.");
            }
            for c in rows {
                row(ui, &mut s.keys, c, &mut s.capture, tips);
            }
        }
        theme::hint(ui, "A browser cannot tell the keypad's number keys from the others: give the views other keys to use them here.");
        if changed > 0 {
            ui.add_space(6.0);
            reset_all(ui, &mut s.keys, tips);
        }
        ui.add_space(8.0);
        theme::heading(ui, "Not for changing");
        theme::hint(ui, "What the pen and mouse do, and the keys kept for the app.");
        fixed(ui);
    });
}

fn row(ui: &mut egui::Ui, keys: &mut Keymap, c: &Listed, capture: &mut Option<String>, tips: bool) {
    let waiting = capture.as_deref() == Some(c.id.as_str());
    let shipped = keys.is_shipped(&c.id);
    ui.horizontal(|ui| {
        let reset_w = if shipped { 0.0 } else { 24.0 };
        let key_w = 118.0f32.min(ui.available_width() * 0.5);
        let name_w = (ui.available_width() - key_w - reset_w - 8.0).max(40.0);
        let name = egui::Label::new(egui::RichText::new(&c.name).font(theme::font(11.0)).color(theme::TEXT())).wrap();
        let layout = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true);
        ui.allocate_ui_with_layout(egui::vec2(name_w, 22.0), layout, |ui| {
            ui.set_min_size(egui::vec2(name_w, 22.0));
            ui.add(name);
        });
        let text = if waiting { "Press a key\u{2026}".to_owned() } else { keys_text(&keys.keys(&c.id)) };
        let style = ButtonStyle { height: 22.0, active: waiting.then_some(ACCENT()), ..Default::default() };
        let button = theme::button(ui, key_w, &text, style);
        let help = format!("{}: click, then press the key to put it on. Esc leaves it as it is.", c.name);
        if theme::tip(button, tips, &help).clicked() {
            *capture = if waiting { None } else { Some(c.id.clone()) };
        }
        if !shipped {
            let back = format!("Back to {}, as the app ships it.", keys_text(&c.shipped));
            let reset = theme::button(ui, 22.0, "\u{21ba}", ButtonStyle { height: 22.0, ..Default::default() });
            if theme::tip(reset, tips, &back).clicked() {
                keys.reset(&c.id);
            }
        }
    });
}

fn presets(ui: &mut egui::Ui, keys: &mut Keymap, tips: bool) {
    let id = Id::new("shortcuts_preset_armed");
    let now = ui.input(|i| i.time);
    let armed: Option<String> =
        ui.data(|d| d.get_temp::<(String, f64)>(id)).filter(|(_, at)| now - at < RESET_ALL_SECONDS).map(|(name, _)| name);
    let ours: &[(&str, Option<&str>)] = &[];
    let sets = [
        ("GolemSculpt", ours, "Every key as the app ships it."),
        (
            "ZBrush",
            keymap::ZBRUSH,
            "ZBrush's keys where this app has the command: W gizmo, D / Shift + D levels, F frame, Shift + F \
             wireframe, P perspective, C colour pick, U and O held for strength and focal shift, Ctrl + I and \
             Ctrl + Shift + A for the mask. ZBrush's Alt to subtract and Ctrl to mask are the pen's, not keys: here \
             Ctrl inverts the brush and the Mask button paints the mask.",
        ),
    ];
    let custom = !sets.iter().any(|(_, set, _)| keys.is(set));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Keys like").font(theme::font(11.0)).color(theme::HINT()));
        let w = ((ui.available_width() - 6.0) / sets.len() as f32).max(40.0);
        for (name, set, help) in sets {
            let on = keys.is(set);
            let asking = armed.as_deref() == Some(name);
            let text = if asking { "Again to Switch" } else { name };
            let style = ButtonStyle { height: 22.0, active: (on || asking).then_some(ACCENT()), ..Default::default() };
            if theme::tip(theme::button(ui, w, text, style), tips, help).clicked() && !on {
                if custom && !asking {
                    ui.data_mut(|d| d.insert_temp(id, (name.to_owned(), now)));
                } else {
                    keys.apply(set);
                    ui.data_mut(|d| d.remove::<(String, f64)>(id));
                }
            }
        }
    });
    if armed.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
}

fn reset_all(ui: &mut egui::Ui, keys: &mut Keymap, tips: bool) {
    let id = Id::new("shortcuts_reset_all");
    let now = ui.input(|i| i.time);
    let armed = ui.data(|d| d.get_temp::<f64>(id)).is_some_and(|at| now - at < RESET_ALL_SECONDS);
    let text = if armed { "Click Again to Reset Every Key" } else { "Reset All Shortcuts" };
    let style = ButtonStyle { active: armed.then_some(ACCENT()), ..Default::default() };
    let button = theme::button(ui, ui.available_width(), text, style);
    let help = "Puts every command back on the key the app ships with. Press twice: it cannot be undone.";
    if theme::tip(button, tips, help).clicked() {
        if armed {
            keys.reset_all();
            ui.data_mut(|d| d.remove::<f64>(id));
        } else {
            ui.data_mut(|d| d.insert_temp(id, now));
        }
    }
    if armed {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
}

fn fixed(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let widest = FIXED
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .map(|(key, _)| ui.painter().layout_no_wrap((*key).to_owned(), theme::font(11.0), theme::KEY).size().x)
        .fold(0.0f32, f32::max);
    let keys = (widest + 10.0).min(width * 0.46);
    for (heading, rows) in FIXED {
        theme::heading(ui, heading);
        for (key, what) in rows.iter() {
            let p = ui.painter();
            let key = p.layout((*key).to_owned(), theme::font(11.0), theme::KEY, keys - 8.0);
            let what = p.layout((*what).to_owned(), theme::font(11.0), theme::TEXT(), (width - keys).max(40.0));
            let height = key.size().y.max(what.size().y) + 3.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            ui.painter().galley(rect.left_top(), key, theme::KEY);
            ui.painter().galley(rect.left_top() + egui::vec2(keys, 0.0), what, theme::TEXT());
        }
    }
}
