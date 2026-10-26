//! The keyboard shortcuts a person can change: what each key does, as the
//! app ships it and as they have changed it.
//!
//! A shortcut is a [`Chord`], a key with Ctrl, Shift and Alt held or not, and
//! what it does is a command named by a short id that never changes
//! (`"subdivide"`, `"brush:Clay"`, `"tool:measure"`), so a settings file keeps
//! working whatever the app later calls the command on screen. The commands
//! are the app's own (the [`COMMANDS`] table), one for each brush in the
//! library, and one for each hosted tool.
//!
//! The settings file keeps only what differs from how the app ships: for each
//! command changed, the keys it now has (none, to take its key away). So a
//! shortcut the app later adds or moves reaches a person who never touched
//! it, and one they did change stays theirs.
//!
//! Not everything is a shortcut here. What the pen and mouse do with a key
//! held (Alt to orbit, Ctrl to invert the brush, Shift to smooth) is how
//! sculpting works, not a command, and a few keys have a job too close to the
//! app's workings to move: Space for the brush wheel, Esc and Enter for
//! cancelling and finishing, Tab for the clean view, [ and ] for the brush's
//! size, O for the turntable, and Ctrl + Q, Ctrl + = and Ctrl + -. Those are [`reserved`], and the
//! Keyboard Shortcuts card lists them as they are.

use std::collections::BTreeMap;

use winit::keyboard::KeyCode;

/// A key, with the modifiers that must be held with it (and no others).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A key a shortcut can be on, by its place in [`KEYS`]. The number keys and
/// the keypad's are different keys, as in Blender.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(u16);

/// Every key a shortcut may be on, with its name: what the card shows and the
/// settings file keeps.
const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "A"),
    (KeyCode::KeyB, "B"),
    (KeyCode::KeyC, "C"),
    (KeyCode::KeyD, "D"),
    (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"),
    (KeyCode::KeyG, "G"),
    (KeyCode::KeyH, "H"),
    (KeyCode::KeyI, "I"),
    (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"),
    (KeyCode::KeyL, "L"),
    (KeyCode::KeyM, "M"),
    (KeyCode::KeyN, "N"),
    (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"),
    (KeyCode::KeyQ, "Q"),
    (KeyCode::KeyR, "R"),
    (KeyCode::KeyS, "S"),
    (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"),
    (KeyCode::KeyV, "V"),
    (KeyCode::KeyW, "W"),
    (KeyCode::KeyX, "X"),
    (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"),
    (KeyCode::Digit1, "1"),
    (KeyCode::Digit2, "2"),
    (KeyCode::Digit3, "3"),
    (KeyCode::Digit4, "4"),
    (KeyCode::Digit5, "5"),
    (KeyCode::Digit6, "6"),
    (KeyCode::Digit7, "7"),
    (KeyCode::Digit8, "8"),
    (KeyCode::Digit9, "9"),
    (KeyCode::Digit0, "0"),
    (KeyCode::Numpad1, "Numpad 1"),
    (KeyCode::Numpad2, "Numpad 2"),
    (KeyCode::Numpad3, "Numpad 3"),
    (KeyCode::Numpad4, "Numpad 4"),
    (KeyCode::Numpad5, "Numpad 5"),
    (KeyCode::Numpad6, "Numpad 6"),
    (KeyCode::Numpad7, "Numpad 7"),
    (KeyCode::Numpad8, "Numpad 8"),
    (KeyCode::Numpad9, "Numpad 9"),
    (KeyCode::Numpad0, "Numpad 0"),
    (KeyCode::NumpadDivide, "Numpad /"),
    (KeyCode::NumpadMultiply, "Numpad *"),
    (KeyCode::NumpadSubtract, "Numpad -"),
    (KeyCode::NumpadAdd, "Numpad +"),
    (KeyCode::NumpadDecimal, "Numpad ."),
    (KeyCode::F1, "F1"),
    (KeyCode::F2, "F2"),
    (KeyCode::F3, "F3"),
    (KeyCode::F4, "F4"),
    (KeyCode::F5, "F5"),
    (KeyCode::F6, "F6"),
    (KeyCode::F7, "F7"),
    (KeyCode::F8, "F8"),
    (KeyCode::F9, "F9"),
    (KeyCode::F10, "F10"),
    (KeyCode::F11, "F11"),
    (KeyCode::F12, "F12"),
    (KeyCode::Slash, "/"),
    (KeyCode::Backslash, "\\"),
    (KeyCode::Comma, ","),
    (KeyCode::Period, "."),
    (KeyCode::Semicolon, ";"),
    (KeyCode::Quote, "'"),
    (KeyCode::Backquote, "`"),
    (KeyCode::Minus, "-"),
    (KeyCode::Equal, "="),
    (KeyCode::BracketLeft, "["),
    (KeyCode::BracketRight, "]"),
    (KeyCode::Insert, "Insert"),
    (KeyCode::Delete, "Delete"),
    (KeyCode::Home, "Home"),
    (KeyCode::End, "End"),
    (KeyCode::PageUp, "Page Up"),
    (KeyCode::PageDown, "Page Down"),
    (KeyCode::ArrowUp, "Up"),
    (KeyCode::ArrowDown, "Down"),
    (KeyCode::ArrowLeft, "Left"),
    (KeyCode::ArrowRight, "Right"),
    (KeyCode::Space, "Space"),
    (KeyCode::Tab, "Tab"),
    (KeyCode::Enter, "Enter"),
    (KeyCode::NumpadEnter, "Numpad Enter"),
    (KeyCode::Escape, "Esc"),
    (KeyCode::Backspace, "Backspace"),
];

impl Key {
    /// The key `code` is, if a shortcut can be on it.
    pub fn of(code: KeyCode) -> Option<Key> {
        KEYS.iter().position(|(c, _)| *c == code).map(|i| Key(i as u16))
    }

    pub fn code(self) -> KeyCode {
        KEYS[self.0 as usize].0
    }

    pub fn name(self) -> &'static str {
        KEYS[self.0 as usize].1
    }

    fn named(name: &str) -> Option<Key> {
        KEYS.iter().position(|(_, n)| n.eq_ignore_ascii_case(name)).map(|i| Key(i as u16))
    }
}

impl Chord {
    pub fn new(code: KeyCode, ctrl: bool, shift: bool, alt: bool) -> Option<Chord> {
        Some(Chord { key: Key::of(code)?, ctrl, shift, alt })
    }

    /// "Ctrl + Shift + D": the README's way of writing a key, which is also how
    /// the settings file keeps it.
    pub fn text(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(self.key.name());
        parts.join(" + ")
    }

    /// The same, as short as a button's corner has room for: "Ctrl+D",
    /// "PgUp".
    pub fn badge(&self) -> String {
        let short = match self.key.name() {
            "Page Up" => "PgUp",
            "Page Down" => "PgDn",
            "Delete" => "Del",
            other => other,
        };
        let mut text = String::new();
        for (held, name) in [(self.ctrl, "Ctrl+"), (self.shift, "Shift+"), (self.alt, "Alt+")] {
            if held {
                text.push_str(name);
            }
        }
        text + short
    }

    /// The chord [`Self::text`] wrote, whatever the case and spacing.
    pub fn parse(text: &str) -> Option<Chord> {
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        // "Numpad +" ends in an empty part after the split.
        let (key, mods) = match parts.split_last()? {
            (&"", rest) if rest.last().is_some_and(|k| k.eq_ignore_ascii_case("numpad")) => {
                (Key::named("Numpad +")?, &rest[..rest.len() - 1])
            }
            (key, rest) => (Key::named(key)?, rest),
        };
        let mut chord = Chord { key, ctrl: false, shift: false, alt: false };
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" => chord.alt = true,
                _ => return None,
            }
        }
        Some(chord)
    }
}

/// Why `chord` cannot be a shortcut, in words for the status line; `None`
/// when it can.
pub fn reserved(chord: &Chord) -> Option<&'static str> {
    use KeyCode as K;
    let ctrl_only = chord.ctrl && !chord.shift && !chord.alt;
    Some(match chord.key.code() {
        K::Space => "Space is kept for the brush wheel (hold it).",
        K::Escape => "Esc is kept for cancelling and putting tools away.",
        K::Enter | K::NumpadEnter => "Enter is kept for finishing what a tool is doing.",
        K::Tab => "Tab is kept for the clean view.",
        K::Backspace => "Backspace takes a key away while you choose one.",
        K::BracketLeft | K::BracketRight => "[ and ] are kept for the brush's size and strength.",
        K::KeyO if !chord.ctrl && !chord.shift && !chord.alt => "O is kept for spinning the view (the turntable).",
        K::KeyQ if ctrl_only => "Ctrl + Q is kept for quitting.",
        K::Equal | K::Minus if ctrl_only => "Ctrl + = and Ctrl + - are kept for the interface size.",
        _ => return None,
    })
}

/// What a group of commands is about, the card's headings in order.
pub const GROUPS: &[&str] =
    &["Brushes", "Brush", "Sculpting", "Tools", "Mesh", "Masking", "View", "Objects", "File and undo"];

/// A command of the app's own: its id, what the card calls it, its group,
/// and the keys it ships with.
pub struct Command {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub keys: &'static [&'static str],
}

/// The app's own commands (brushes and hosted tools come from the library and
/// the tools themselves: see [`Keymap::new`]).
pub const COMMANDS: &[Command] = &[
    Command { id: "hold_size", name: "Size (hold and drag)", group: "Brush", keys: &["S"] },
    Command { id: "hold_strength", name: "Strength (hold and drag; tap twice to frame)", group: "Brush", keys: &["F"] },
    Command { id: "hold_focal", name: "Focal shift (hold and drag)", group: "Brush", keys: &["D"] },
    Command { id: "paint", name: "Paint brush; again, the one before", group: "Sculpting", keys: &["P"] },
    Command { id: "eyedropper", name: "Eyedropper: pick up paint", group: "Sculpting", keys: &["I"] },
    Command { id: "mask_mode", name: "Mask painting", group: "Sculpting", keys: &["M"] },
    Command { id: "mirror_x", name: "Mirror X", group: "Sculpting", keys: &["X"] },
    Command { id: "mirror_y", name: "Mirror Y", group: "Sculpting", keys: &[] },
    Command { id: "mirror_z", name: "Mirror Z", group: "Sculpting", keys: &[] },
    Command { id: "transform", name: "Transform gizmo", group: "Tools", keys: &["G"] },
    Command { id: "remesh", name: "Voxel remesh", group: "Mesh", keys: &["R"] },
    Command { id: "quad_remesh", name: "Quad remesh", group: "Mesh", keys: &[] },
    Command { id: "decimate", name: "Decimate", group: "Mesh", keys: &[] },
    Command { id: "subdivide", name: "Subdivide", group: "Mesh", keys: &["Ctrl + D"] },
    Command { id: "level_up", name: "Finer level", group: "Mesh", keys: &["Page Up"] },
    Command { id: "level_down", name: "Coarser level", group: "Mesh", keys: &["Page Down"] },
    Command { id: "reconstruct_subdiv", name: "Reconstruct Subdiv", group: "Mesh", keys: &[] },
    Command { id: "project_detail", name: "Project Detail", group: "Mesh", keys: &[] },
    Command { id: "mask_invert", name: "Invert the mask", group: "Masking", keys: &[] },
    Command { id: "mask_clear", name: "Clear the mask", group: "Masking", keys: &[] },
    Command { id: "mask_grow", name: "Grow the mask", group: "Masking", keys: &[] },
    Command { id: "mask_shrink", name: "Shrink the mask", group: "Masking", keys: &[] },
    Command { id: "mask_blur", name: "Blur the mask", group: "Masking", keys: &[] },
    Command { id: "mask_sharpen", name: "Sharpen the mask", group: "Masking", keys: &[] },
    Command { id: "frame", name: "Frame the selection", group: "View", keys: &[] },
    Command { id: "home_view", name: "Home view", group: "View", keys: &[] },
    Command { id: "ortho", name: "Orthographic / perspective", group: "View", keys: &["Numpad 5"] },
    Command { id: "view_front", name: "Front view", group: "View", keys: &["Numpad 1"] },
    Command { id: "view_back", name: "Back view", group: "View", keys: &["Ctrl + Numpad 1"] },
    Command { id: "view_right", name: "Right view", group: "View", keys: &["Numpad 3"] },
    Command { id: "view_left", name: "Left view", group: "View", keys: &["Ctrl + Numpad 3"] },
    Command { id: "view_top", name: "Top view", group: "View", keys: &["Numpad 7"] },
    Command { id: "view_bottom", name: "Bottom view", group: "View", keys: &["Ctrl + Numpad 7"] },
    Command { id: "wireframe", name: "Wireframe on / off", group: "View", keys: &[] },
    Command { id: "fullscreen", name: "Fullscreen", group: "View", keys: &["F11"] },
    Command { id: "hide", name: "Hide the selection", group: "Objects", keys: &["H"] },
    Command { id: "hide_others", name: "Hide everything else", group: "Objects", keys: &["Shift + H"] },
    Command { id: "show_all", name: "Show everything", group: "Objects", keys: &["Alt + H"] },
    Command { id: "local_view", name: "Local View: solo the selection", group: "Objects", keys: &["/", "Numpad /"] },
    Command { id: "delete", name: "Delete the object (asks)", group: "Objects", keys: &["Delete"] },
    Command { id: "clone", name: "Clone the object", group: "Objects", keys: &[] },
    Command { id: "instance", name: "Instance the object", group: "Objects", keys: &[] },
    Command { id: "undo", name: "Undo", group: "File and undo", keys: &["Ctrl + Z"] },
    Command { id: "redo", name: "Redo", group: "File and undo", keys: &["Ctrl + Y", "Ctrl + Shift + Z"] },
    Command { id: "save", name: "Save", group: "File and undo", keys: &["Ctrl + S"] },
    Command { id: "save_as", name: "Save as", group: "File and undo", keys: &["Ctrl + Shift + S"] },
    Command { id: "open", name: "Open", group: "File and undo", keys: &["Ctrl + O"] },
    Command { id: "import", name: "Import", group: "File and undo", keys: &[] },
    Command { id: "new_scene", name: "New scene (asks first)", group: "File and undo", keys: &["Ctrl + N"] },
];

/// A command the card can list: id, name, group and the keys it ships with,
/// whether the app's own, a brush's or a tool's.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub group: &'static str,
    pub shipped: Vec<Chord>,
}

/// The shortcuts in force: the shipped ones with a person's changes on top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    /// Every command there is, in the card's order.
    commands: Vec<Listed>,
    /// The commands whose keys were changed, and the keys each now has.
    changed: BTreeMap<String, Vec<Chord>>,
}

/// The id of brush `name`'s command.
pub fn brush_id(name: &str) -> String {
    format!("brush:{name}")
}

/// The id of hosted tool `id`'s command.
pub fn tool_id(id: &str) -> String {
    format!("tool:{id}")
}

impl Keymap {
    /// The shipped shortcuts, for the brushes `brushes` (each name with the
    /// key it ships with, if any) and the hosted tools `tools` (id, name and
    /// key). A brush or tool key that one of the app's own commands already
    /// ships with is left off, as the app always has: its own come first.
    pub fn new(brushes: &[(String, Option<Chord>)], tools: &[(&str, &str, Option<Chord>)]) -> Keymap {
        let own: Vec<Listed> = COMMANDS
            .iter()
            .map(|c| Listed {
                id: c.id.to_owned(),
                name: c.name.to_owned(),
                group: c.group,
                shipped: c.keys.iter().filter_map(|k| Chord::parse(k)).collect(),
            })
            .collect();
        let taken = |chord: &Chord| own.iter().any(|c| c.shipped.contains(chord));
        let brushes = brushes.iter().map(|(name, key)| Listed {
            id: brush_id(name),
            name: name.clone(),
            group: "Brushes",
            shipped: key.filter(|k| !taken(k)).into_iter().collect(),
        });
        let tools = tools.iter().map(|(id, name, key)| Listed {
            id: tool_id(id),
            name: (*name).to_owned(),
            group: "Tools",
            shipped: key.filter(|k| !taken(k)).into_iter().collect(),
        });
        let (brushes, tools): (Vec<Listed>, Vec<Listed>) = (brushes.collect(), tools.collect());
        let mut commands: Vec<Listed> = brushes.into_iter().chain(own).chain(tools).collect();
        commands.sort_by_key(|c| GROUPS.iter().position(|g| *g == c.group).unwrap_or(GROUPS.len()));
        Keymap { commands, changed: BTreeMap::new() }
    }

    /// Every command, in the card's order.
    pub fn commands(&self) -> &[Listed] {
        &self.commands
    }

    /// The keys command `id` has now.
    pub fn keys(&self, id: &str) -> Vec<Chord> {
        if let Some(keys) = self.changed.get(id) {
            return keys.clone();
        }
        self.commands.iter().find(|c| c.id == id).map_or(Vec::new(), |c| c.shipped.clone())
    }

    /// The command `chord` runs now, if any.
    pub fn command(&self, chord: &Chord) -> Option<&str> {
        self.commands.iter().map(|c| c.id.as_str()).find(|id| self.keys(id).contains(chord))
    }

    /// Whether command `id`'s keys are as the app ships them.
    pub fn is_shipped(&self, id: &str) -> bool {
        !self.changed.contains_key(id)
    }

    /// The key a button names in its corner: command `id`'s first, short; or
    /// nothing if it has none.
    pub fn badge(&self, id: &str) -> String {
        self.keys(id).first().map_or(String::new(), Chord::badge)
    }

    /// Puts command `id` on `chord` alone, or on no key for `None`. A command
    /// that had the chord loses it, and its id comes back so the status line
    /// can say so.
    pub fn set(&mut self, id: &str, chord: Option<Chord>) -> Option<String> {
        let mut took = None;
        if let Some(chord) = chord {
            let others: Vec<String> =
                self.commands.iter().map(|c| c.id.clone()).filter(|other| other != id).collect();
            for other in others {
                let keys = self.keys(&other);
                if keys.contains(&chord) {
                    let left: Vec<Chord> = keys.into_iter().filter(|k| *k != chord).collect();
                    self.put(&other, left);
                    took = Some(other);
                }
            }
        }
        self.put(id, chord.into_iter().collect());
        took
    }

    /// Command `id` back on the keys it ships with. A command that has one of
    /// those keys now loses it.
    pub fn reset(&mut self, id: &str) {
        self.changed.remove(id);
        for chord in self.keys(id) {
            let others: Vec<String> =
                self.commands.iter().map(|c| c.id.clone()).filter(|other| other != id).collect();
            for other in others {
                let keys = self.keys(&other);
                if keys.contains(&chord) {
                    self.put(&other, keys.into_iter().filter(|k| *k != chord).collect());
                }
            }
        }
    }

    /// Every command back on the keys it ships with.
    pub fn reset_all(&mut self) {
        self.changed.clear();
    }

    /// Keeps `keys` for command `id`, as a change only if they differ from
    /// what it ships with.
    fn put(&mut self, id: &str, keys: Vec<Chord>) {
        let shipped = self.commands.iter().find(|c| c.id == id).map(|c| &c.shipped);
        if shipped == Some(&keys) {
            self.changed.remove(id);
        } else {
            self.changed.insert(id.to_owned(), keys);
        }
    }

    /// What the settings file keeps: each changed command's keys, written out.
    pub fn changes(&self) -> BTreeMap<String, Vec<String>> {
        self.changed.iter().map(|(id, keys)| (id.clone(), keys.iter().map(Chord::text).collect())).collect()
    }

    /// Takes the changes a settings file kept. A key that cannot be read, or
    /// that is [`reserved`], is passed over; a command the app no longer has
    /// (a brush since deleted) is kept, so that the brush's key comes back if
    /// the brush does.
    pub fn take_changes(&mut self, changes: &BTreeMap<String, Vec<String>>) {
        self.changed = changes
            .iter()
            .map(|(id, keys)| {
                let keys = keys.iter().filter_map(|k| Chord::parse(k)).filter(|k| reserved(k).is_none()).collect();
                (id.clone(), keys)
            })
            .collect();
    }

    /// A new set of brushes and tools, keeping the changes.
    pub fn renewed(&self, brushes: &[(String, Option<Chord>)], tools: &[(&str, &str, Option<Chord>)]) -> Keymap {
        Keymap { changed: self.changed.clone(), ..Keymap::new(brushes, tools) }
    }
}

#[cfg(test)]
mod tests;
