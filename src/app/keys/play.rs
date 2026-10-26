//! `--demo-keys`: the keyboard shortcuts worked through the routines a key
//! press goes through, printing `ok` or `FAIL` for each thing that must then
//! be true. The keys go in as the window would hand them over, a key code
//! with the modifiers held; nothing here opens a file box or touches a file.

use winit::keyboard::{KeyCode, ModifiersState};

use super::super::Running;
use crate::keymap::Chord;

fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

impl Running {
    /// A press of `code` with `mods` held, as `key_pressed` runs it from the
    /// shortcuts on (the questions, the turntable and the hosted tools have
    /// nothing up here).
    fn play_press(&mut self, code: KeyCode, mods: ModifiersState) -> bool {
        self.input.mods = mods;
        let taken = self.shortcut_capture(code) || self.run_key_command(code);
        self.input.mods = ModifiersState::empty();
        taken
    }

    /// The card's "press a key" for command `id`, answered with `code`.
    fn play_rebind(&mut self, id: &str, code: KeyCode, mods: ModifiersState) {
        self.ui.shortcut_capture = Some(id.to_owned());
        self.play_press(code, mods);
    }

    pub(in crate::app) fn keys_play(&mut self) {
        let none = ModifiersState::empty();
        let (ctrl, shift) = (ModifiersState::CONTROL, ModifiersState::SHIFT);
        let in_hand = |app: &Running| {
            let library = &app.doc.sculptor.library;
            library.entries()[library.selected(&app.doc.sculptor.settings)].name.clone()
        };

        // The keys as the app ships them.
        self.play_press(KeyCode::Digit2, none);
        say("2 takes Clay", in_hand(self) == "Clay");
        let mask = self.doc.sculptor.settings.mask_mode;
        self.play_press(KeyCode::KeyM, none);
        say("M turns mask painting on", self.doc.sculptor.settings.mask_mode != mask);
        self.play_press(KeyCode::KeyM, none);
        let mirror = self.doc.scene.active_symmetry().mirror[0];
        self.play_press(KeyCode::KeyX, none);
        say("X turns Mirror X over", self.doc.scene.active_symmetry().mirror[0] != mirror);
        self.play_press(KeyCode::KeyX, none);
        self.play_press(KeyCode::Numpad3, none);
        self.doc.camera.update(10.0);
        // The same way round whether the camera keeps it as -90 or 270 degrees.
        let right = (self.doc.camera.yaw + 90.0).rem_euclid(360.0);
        say("Numpad 3 turns to the right view", right.min(360.0 - right) < 0.5);
        self.play_press(KeyCode::Numpad1, none);
        self.doc.camera.update(10.0);
        say("Numpad 1 turns to the front", self.doc.camera.yaw.abs() < 0.5);
        self.play_press(KeyCode::KeyG, none);
        say("G brings the Transform gizmo up", self.transform_active());
        self.play_press(KeyCode::KeyG, none);
        say("and G again puts it away", !self.transform_active());
        self.play_press(KeyCode::KeyL, none);
        say("L brings Measure up", self.tools_active());
        self.tools_leave();
        say("Shift + D runs nothing as shipped", !self.play_press(KeyCode::KeyD, shift));

        // Changing them, as the card does.
        self.play_rebind("mask_mode", KeyCode::KeyQ, none);
        say("mask painting goes on Q", self.keys().command(&Chord::parse("Q").unwrap()) == Some("mask_mode"));
        let mask = self.doc.sculptor.settings.mask_mode;
        self.play_press(KeyCode::KeyQ, none);
        say("and Q turns it on", self.doc.sculptor.settings.mask_mode != mask);
        self.play_press(KeyCode::KeyQ, none);
        say("while M does nothing now", !self.play_press(KeyCode::KeyM, none));
        self.play_rebind("remesh", KeyCode::KeyD, ctrl);
        let keys = self.keys();
        say("Remesh takes Ctrl + D", keys.badge("remesh") == "Ctrl+D");
        say("and Subdivide, which had it, has no key", keys.keys("subdivide").is_empty());
        say("so its button names none", keys.badge("subdivide").is_empty());
        self.play_rebind("remesh", KeyCode::Space, none);
        say("Space is refused, and Remesh keeps Ctrl + D", self.keys().badge("remesh") == "Ctrl+D");
        say("with a word why", self.ui.status.0.contains("brush wheel"));
        self.play_rebind("remesh", KeyCode::Backspace, none);
        say("Backspace takes Remesh's key away", self.keys().keys("remesh").is_empty());
        self.play_rebind("remesh", KeyCode::Escape, none);
        say("Esc leaves it as it was", self.keys().keys("remesh").is_empty() && self.ui.shortcut_capture.is_none());
        self.ui.shortcut_capture = Some("remesh".to_owned());
        self.play_press(KeyCode::ShiftLeft, shift);
        say("Shift alone is not a key yet: the card still waits", self.ui.shortcut_capture.is_some());
        self.ui.shortcut_capture = None;

        // Any brush of the library, and the hold keys.
        self.play_rebind("brush:Trim", KeyCode::KeyT, shift);
        self.play_press(KeyCode::KeyT, shift);
        say("Trim, a brush with no key as shipped, is taken up by its new key", in_hand(self) == "Trim");
        self.play_rebind("hold_size", KeyCode::KeyU, none);
        self.play_press(KeyCode::KeyU, none);
        say("the size drag is on U now", self.input.gauge.is_some());
        self.key_released(KeyCode::KeyU);
        say("and letting U go ends it", self.input.gauge.is_none());
        self.play_press(KeyCode::KeyS, none);
        say("S no longer starts it", self.input.gauge.is_none());

        // Kept in the settings, only what changed.
        let kept = self.prefs_now().shortcuts;
        let wanted = ["brush:Trim", "hold_size", "mask_mode", "remesh", "subdivide"];
        say("the settings keep the five commands changed", kept.keys().map(String::as_str).eq(wanted));
        say("Subdivide kept with no key", kept.get("subdivide").is_some_and(Vec::is_empty));
        self.doc.opts.keys.reset_all();
        say("Reset All puts every key back", self.prefs_now().shortcuts.is_empty());
        self.play_press(KeyCode::Digit2, none);
        say("and 2 takes Clay again", in_hand(self) == "Clay");
    }
}
