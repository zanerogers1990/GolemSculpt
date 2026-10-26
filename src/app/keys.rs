//! The keyboard shortcuts on the app's side: a key pressed becomes the
//! command the key map has it on (`keymap.rs`), and the Keyboard Shortcuts
//! card's "press a key" puts a command on the next key pressed.
//!
//! The key map is built from the brush library and the hosted tools, so it
//! follows them: a brush made, renamed or deleted gets or loses its line on
//! the card the next time the map is looked at ([`refresh`]).
//!
//! A command does here what its key did before it could be changed, with the
//! same care about when: most wait while a stroke or a drag is under way
//! (`idle`), as they always have; Undo, Save and the like do not, since
//! `Running::act` already waits for the scene to be free.

use winit::keyboard::KeyCode;

use super::{DOUBLE_CLICK, Drag, Gauge, GaugeKind, Running};
use crate::doc::{Action, Doc, MaskOp, Tool};
use crate::keymap::{self, Chord, Key, Keymap};

mod play;

/// The brushes and hosted tools as the key map lists them: each library
/// brush with the number key a base brush ships with, and each tool with the
/// key it asks for.
fn listed(doc: &Doc) -> (Vec<(String, Option<Chord>)>, Vec<(&'static str, &'static str, Option<Chord>)>) {
    let entries = doc.sculptor.library.entries();
    let brushes = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            // A base brush (its kernel's own entry) keeps its number key; P is the Paint command's.
            let base = crate::sculpt::library::Library::base(entry.kernel) == i;
            let key = base
                .then(|| entry.kernel.hotkey())
                .flatten()
                .filter(char::is_ascii_digit)
                .and_then(|digit| Chord::parse(&digit.to_string()));
            (entry.name.clone(), key)
        })
        .collect();
    let tools = (0..doc.tools.len())
        .filter_map(|i| doc.tools.info(i))
        .map(|info| (info.id, info.name, super::tools::chord_of(info.hotkey)))
        .collect();
    (brushes, tools)
}

/// Brings `doc`'s key map up to date with the library and the tools,
/// keeping every change made to it. Cheap when nothing has changed.
pub(crate) fn refresh(doc: &mut Doc) {
    let (brushes, tools) = listed(doc);
    let wanted: Vec<String> = brushes
        .iter()
        .map(|(name, _)| keymap::brush_id(name))
        .chain(tools.iter().map(|(id, _, _)| keymap::tool_id(id)))
        .collect();
    let have: Vec<&str> = doc.opts.keys.commands().iter().map(|c| c.id.as_str()).filter(|id| id.contains(':')).collect();
    let mut wanted_sorted: Vec<&str> = wanted.iter().map(String::as_str).collect();
    let mut have_sorted = have.clone();
    wanted_sorted.sort_unstable();
    have_sorted.sort_unstable();
    if wanted_sorted != have_sorted {
        doc.opts.keys = doc.opts.keys.renewed(&brushes, &tools);
    }
}

/// The chord `code` makes with the modifiers held now.
fn chord(code: KeyCode, mods: winit::keyboard::ModifiersState) -> Option<Chord> {
    Chord::new(code, mods.control_key(), mods.shift_key(), mods.alt_key())
}

/// The modifier keys themselves: pressing one is not yet choosing a key.
fn is_modifier(code: KeyCode) -> bool {
    use KeyCode as K;
    matches!(code, K::ShiftLeft | K::ShiftRight | K::ControlLeft | K::ControlRight | K::AltLeft | K::AltRight)
        || matches!(code, K::SuperLeft | K::SuperRight)
}

/// The commands that keep their keys while a hosted tool is up: the view's
/// and the Transform gizmo's, which have nothing to do with the tool.
fn stays_with_app(id: &str) -> bool {
    matches!(id, "transform" | "fullscreen" | "ortho" | "frame" | "home_view" | "wireframe") || id.starts_with("view_")
}

impl Running {
    /// The key map, up to date (see [`refresh`]).
    pub(super) fn keys(&mut self) -> &Keymap {
        refresh(&mut self.doc);
        &self.doc.opts.keys
    }

    /// The command key `code` runs with the modifiers held now, if any.
    pub(super) fn key_command(&mut self, code: KeyCode) -> Option<String> {
        let chord = chord(code, self.input.mods)?;
        self.keys().command(&chord).map(str::to_owned)
    }

    /// Whether `code` is the key of command `id`, whatever is held with it:
    /// for the hold keys, whose release ends the drag however it began.
    pub(super) fn is_key_of(&mut self, id: &str, code: KeyCode) -> bool {
        let Some(key) = Key::of(code) else { return false };
        self.keys().keys(id).iter().any(|k| k.key == key)
    }

    /// While a hosted tool is up, whether `code` (with what is held) is a key
    /// the app keeps for itself.
    pub(super) fn key_stays_with_app(&mut self, code: KeyCode) -> bool {
        code == KeyCode::Tab || self.key_command(code).is_some_and(|id| stays_with_app(&id))
    }

    /// The Keyboard Shortcuts card is waiting for a key: this one is it. Esc
    /// stops waiting, Backspace or Delete takes the command's key away, and a
    /// key kept for the app is refused with a word why. True when the card
    /// was waiting, whatever the key did.
    pub(super) fn shortcut_capture(&mut self, code: KeyCode) -> bool {
        let Some(id) = self.ui.shortcut_capture.clone() else { return false };
        if is_modifier(code) {
            // Still choosing: Ctrl, Shift or Alt first, then the key.
            return true;
        }
        self.ui.shortcut_capture = None;
        let name = self.doc.opts.keys.commands().iter().find(|c| c.id == id).map_or(id.clone(), |c| c.name.clone());
        match code {
            KeyCode::Escape => {}
            KeyCode::Backspace | KeyCode::Delete => {
                self.doc.opts.keys.set(&id, None);
                self.status(format!("{name} has no key now."), true);
            }
            _ => {
                let Some(chord) = chord(code, self.input.mods) else {
                    return self.status_then(format!("That key cannot be a shortcut. {name} keeps its key."), false);
                };
                if let Some(why) = keymap::reserved(&chord) {
                    return self.status_then(format!("{why} {name} keeps its key."), false);
                }
                let took = self.doc.opts.keys.set(&id, Some(chord));
                let was = took
                    .and_then(|other| self.doc.opts.keys.commands().iter().find(|c| c.id == other).map(|c| c.name.clone()))
                    .map_or(String::new(), |other| format!(" It was {other}'s, which has no key now."));
                self.status(format!("{name} is on {} now.{was}", chord.text()), true);
            }
        }
        self.redraw = true;
        true
    }

    /// [`Running::status`], then true: for the capture's refusals.
    fn status_then(&mut self, text: String, ok: bool) -> bool {
        self.status(text, ok);
        self.redraw = true;
        true
    }

    /// Runs the command a key pressed is on. True if the key was a
    /// command's, whether or not it could run just now.
    pub(super) fn run_key_command(&mut self, code: KeyCode) -> bool {
        let Some(id) = self.key_command(code) else { return false };
        let idle = self.input.drag == Drag::None;
        let anchor = self.input.pointer;
        let gauge = move |kind, start| Gauge { kind, anchor, start };
        match id.as_str() {
            "undo" => self.act(Action::Undo),
            "redo" => self.act(Action::Redo),
            "save" => self.act(Action::Save),
            "save_as" => self.act(Action::SaveAs),
            "open" => self.act(Action::Open),
            "fullscreen" => {
                use winit::window::Fullscreen;
                let full = self.window.fullscreen().is_some();
                self.window.set_fullscreen((!full).then_some(Fullscreen::Borderless(None)));
            }
            // Everything below waits while a stroke or a drag holds the scene.
            _ if !idle => {}
            "hold_size" => self.input.gauge = Some(gauge(GaugeKind::Size, self.doc.sculptor.settings.size())),
            "hold_strength" => {
                // Tapped twice it frames the model; held and dragged it is strength.
                let now = std::time::Instant::now();
                if self.input.last_f.is_some_and(|t| now - t < DOUBLE_CLICK) {
                    self.input.last_f = None;
                    self.frame_view();
                } else {
                    self.input.last_f = Some(now);
                    let strength = self.doc.sculptor.settings.current().strength;
                    self.input.gauge = Some(gauge(GaugeKind::Strength, strength));
                }
            }
            "hold_focal" => {
                let focal = self.doc.sculptor.settings.current().focal_shift;
                self.input.gauge = Some(gauge(GaugeKind::Focal, focal));
            }
            "paint" => self.paint_key(),
            "eyedropper" => self.act(Action::Paint(crate::doc::PaintAction::PickAt(self.input.pointer))),
            "mask_mode" => {
                let s = &mut self.doc.sculptor.settings;
                s.mask_mode = !s.mask_mode;
                let on = s.mask_mode;
                self.toast(if on { "Mask" } else { "Mask Off" });
            }
            "mirror_x" | "mirror_y" | "mirror_z" => {
                // The selected object's own mirror, as in the Unity app.
                let axis = match id.as_str() {
                    "mirror_x" => 0,
                    "mirror_y" => 1,
                    _ => 2,
                };
                self.doc.edit_symmetry(|symmetry| symmetry.mirror[axis] = !symmetry.mirror[axis]);
                let on = self.doc.scene.active_symmetry().mirror[axis];
                let name = ["Mirror X", "Mirror Y", "Mirror Z"][axis];
                self.toast(if on { name.to_owned() } else { "Mirror Off".to_owned() });
            }
            "transform" => self.set_tool(if self.transform_active() { Tool::Sculpt } else { Tool::Transform }),
            "remesh" => self.act(Action::Remesh),
            "quad_remesh" => self.act(Action::QuadRemesh),
            "decimate" => self.act(Action::Decimate),
            "subdivide" => self.act(Action::Subdivide),
            "level_up" => self.act(Action::LevelBy(1)),
            "level_down" => self.act(Action::LevelBy(-1)),
            "reconstruct_subdiv" => self.act(Action::ReconstructSubdiv),
            "project_detail" => self.act(Action::ProjectDetail),
            "mask_invert" => self.act(Action::Mask(MaskOp::Invert)),
            "mask_clear" => self.act(Action::Mask(MaskOp::Clear)),
            "mask_grow" => self.act(Action::Mask(MaskOp::Grow)),
            "mask_shrink" => self.act(Action::Mask(MaskOp::Shrink)),
            "mask_blur" => self.act(Action::Mask(MaskOp::Blur)),
            "mask_sharpen" => self.act(Action::Mask(MaskOp::Sharpen)),
            "frame" => self.frame_view(),
            "home_view" => self.act(Action::HomeView),
            "ortho" => self.act(Action::ToggleOrtho),
            // As the view gizmo's bubbles turn it (ui/overlay.rs).
            "view_front" => self.act(Action::Snap(0.0, 0.0)),
            "view_back" => self.act(Action::Snap(180.0, 0.0)),
            "view_right" => self.act(Action::Snap(-90.0, 0.0)),
            "view_left" => self.act(Action::Snap(90.0, 0.0)),
            "view_top" => self.act(Action::Snap(0.0, 90.0)),
            "view_bottom" => self.act(Action::Snap(0.0, -90.0)),
            "wireframe" => {
                self.doc.opts.wireframe = !self.doc.opts.wireframe;
                let on = self.doc.opts.wireframe;
                self.toast(if on { "Wireframe" } else { "Wireframe Off" });
            }
            "hide" => self.hide_selection(),
            "hide_others" => self.hide_others(),
            "show_all" => self.show_everything(),
            "local_view" => self.local_view_key(),
            "delete" => self.delete_key(),
            "clone" => self.act(Action::CloneObject),
            "instance" => self.act(Action::InstanceObject),
            "import" => self.act(Action::Import),
            "new_scene" => self.act(Action::NewScene),
            other => {
                if let Some(name) = other.strip_prefix("brush:") {
                    self.pick_brush(name);
                } else if let Some(tool) = other.strip_prefix("tool:") {
                    self.toggle_tool(tool);
                }
            }
        }
        true
    }

    /// Takes the library brush named `name` in hand, as its tile does.
    fn pick_brush(&mut self, name: &str) {
        let sculptor = &mut self.doc.sculptor;
        let Some(i) = sculptor.library.entries().iter().position(|e| e.name == name) else { return };
        sculptor.library.select(i, &mut sculptor.settings);
        // With the Transform tool up, the brush is picked to sculpt with.
        self.back_to_sculpting();
        self.toast(name.to_owned());
    }

    /// Brings up the hosted tool `id`, or puts it away if it is up. Not while
    /// the SSphere tool has the view, which keeps its own keys.
    fn toggle_tool(&mut self, id: &str) {
        if self.doc.tool == Tool::SSpheres {
            return;
        }
        let Some(index) = self.doc.tools.info_index(id) else { return };
        if self.doc.tools.active(self.doc.tool) == Some(index) {
            self.tools_leave();
        } else {
            self.tools_activate(index);
        }
    }
}
