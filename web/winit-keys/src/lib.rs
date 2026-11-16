//! The web build's stand-in for the one piece of winit the desktop's key
//! map (`src/keymap.rs`, compiled into the web version as it is) names:
//! `winit::keyboard::KeyCode`, the keys by where they are on the keyboard.
//! web/Cargo.toml puts this crate in under the name `winit`. Only the keys
//! the key map lists are here; the web app turns egui's keys into these
//! (web/src/shortcuts.rs).

pub mod keyboard {
    /// A key by its place on the keyboard, as winit 0.30 names it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub enum KeyCode {
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
        KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
        Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
        Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9,
        NumpadDivide, NumpadMultiply, NumpadSubtract, NumpadAdd, NumpadDecimal, NumpadEnter,
        F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
        Slash, Backslash, Comma, Period, Semicolon, Quote, Backquote, Minus, Equal,
        BracketLeft, BracketRight,
        Insert, Delete, Home, End, PageUp, PageDown,
        ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
        Space, Tab, Enter, Escape, Backspace,
        ShiftLeft, ShiftRight, ControlLeft, ControlRight, AltLeft, AltRight, SuperLeft, SuperRight,
    }
}
