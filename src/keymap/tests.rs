use std::collections::BTreeMap;

use winit::keyboard::KeyCode;

use super::*;

fn chord(text: &str) -> Chord {
    Chord::parse(text).unwrap_or_else(|| panic!("{text} reads"))
}

/// The shipped keys with two brushes (Clay on 2, a brush of the user's own
/// on nothing) and two tools (Measure on L, and one asking for R, which
/// Remesh ships with).
fn shipped() -> Keymap {
    let brushes = [("Clay".to_owned(), Some(chord("2"))), ("My Trim".to_owned(), None)];
    let tools = [("measure", "Measure", Some(chord("L"))), ("greedy", "Greedy", Some(chord("R")))];
    Keymap::new(&brushes, &tools)
}

#[test]
fn a_chord_is_written_the_readmes_way_and_read_back_whatever_the_case() {
    for text in ["Ctrl + Shift + D", "Page Up", "Numpad /", "Numpad +", "Alt + H", "F11", "/", "Ctrl + Numpad 1"] {
        assert_eq!(chord(text).text(), text);
    }
    assert_eq!(chord("ctrl+shift+d"), chord("Ctrl + Shift + D"));
    assert_eq!(chord("Ctrl + Numpad +").text(), "Ctrl + Numpad +");
    assert!(Chord::parse("Hyper + D").is_none());
    assert!(Chord::parse("Ctrl + ").is_none());
    assert!(Chord::parse("Windows").is_none());
    assert_eq!(chord("Ctrl + Page Down").badge(), "Ctrl+PgDn");
    assert_eq!(Chord::new(KeyCode::KeyD, true, false, false), Some(chord("Ctrl + D")));
}

#[test]
fn the_shipped_keys_are_the_ones_the_app_has_always_had() {
    let keys = shipped();
    assert_eq!(keys.command(&chord("Ctrl + D")), Some("subdivide"));
    assert_eq!(keys.command(&chord("D")), Some("hold_focal"));
    assert_eq!(keys.command(&chord("Ctrl + Shift + Z")), Some("redo"));
    assert_eq!(keys.command(&chord("Ctrl + Y")), Some("redo"));
    assert_eq!(keys.command(&chord("2")), Some("brush:Clay"));
    assert_eq!(keys.command(&chord("L")), Some("tool:measure"));
    assert_eq!(keys.command(&chord("R")), Some("remesh"), "the app's own keys come before a tool's");
    assert!(keys.keys("tool:greedy").is_empty());
    assert!(keys.keys("brush:My Trim").is_empty());
    assert_eq!(keys.command(&chord("Shift + D")), None, "a chord is its key and its modifiers exactly");
    assert_eq!(keys.badge("subdivide"), "Ctrl+D");
    assert_eq!(keys.badge("decimate"), "");
}

#[test]
fn every_command_has_a_group_on_the_card_and_no_two_ship_on_one_key() {
    let keys = shipped();
    let mut seen = std::collections::HashMap::new();
    for c in keys.commands() {
        assert!(GROUPS.contains(&c.group), "{} is in no group", c.id);
        for k in &c.shipped {
            assert!(reserved(k).is_none(), "{} ships on a reserved key", c.id);
            if let Some(other) = seen.insert(*k, c.id.clone()) {
                panic!("{} and {other} both ship on {}", c.id, k.text());
            }
        }
    }
    for c in COMMANDS {
        for k in c.keys {
            assert!(Chord::parse(k).is_some(), "{}'s {k} reads", c.id);
        }
    }
}

#[test]
fn a_key_taken_for_one_command_is_taken_from_the_one_that_had_it() {
    let mut keys = shipped();
    let took = keys.set("decimate", Some(chord("Ctrl + D")));
    assert_eq!(took.as_deref(), Some("subdivide"));
    assert_eq!(keys.command(&chord("Ctrl + D")), Some("decimate"));
    assert!(keys.keys("subdivide").is_empty());
    // One of Redo's two keys: it keeps the other.
    assert_eq!(keys.set("undo", Some(chord("Ctrl + Y"))).as_deref(), Some("redo"));
    assert_eq!(keys.keys("redo"), vec![chord("Ctrl + Shift + Z")]);
    // A command put on no key.
    assert_eq!(keys.set("remesh", None), None);
    assert_eq!(keys.command(&chord("R")), None);
}

#[test]
fn reset_puts_a_command_back_and_takes_its_key_from_whoever_has_it_now() {
    let mut keys = shipped();
    keys.set("decimate", Some(chord("Ctrl + D")));
    keys.reset("subdivide");
    assert_eq!(keys.command(&chord("Ctrl + D")), Some("subdivide"));
    assert!(keys.keys("decimate").is_empty());
    assert!(keys.is_shipped("subdivide"));
    keys.set("remesh", Some(chord("Q")));
    keys.reset_all();
    assert_eq!(keys, shipped());
}

#[test]
fn only_the_changes_go_in_the_settings_file_and_come_back_out() {
    let mut keys = shipped();
    keys.set("decimate", Some(chord("Ctrl + Shift + D")));
    keys.set("remesh", None);
    // A command put back on its own keys is no change at all.
    keys.set("hide", Some(chord("H")));
    let changes = keys.changes();
    let wanted: BTreeMap<String, Vec<String>> = [
        ("decimate".to_owned(), vec!["Ctrl + Shift + D".to_owned()]),
        ("remesh".to_owned(), Vec::new()),
    ]
    .into_iter()
    .collect();
    assert_eq!(changes, wanted);
    let mut again = shipped();
    again.take_changes(&changes);
    assert_eq!(again, keys);
}

#[test]
fn a_settings_file_cannot_put_a_command_on_a_reserved_or_unreadable_key() {
    let mut keys = shipped();
    let changes: BTreeMap<String, Vec<String>> =
        [("remesh".to_owned(), vec!["Space".to_owned(), "Hyper + R".to_owned(), "Ctrl + R".to_owned()])]
            .into_iter()
            .collect();
    keys.take_changes(&changes);
    assert_eq!(keys.keys("remesh"), vec![chord("Ctrl + R")]);
}

#[test]
fn the_keys_kept_for_the_app_are_refused_with_a_reason() {
    for text in ["Space", "Esc", "Enter", "Tab", "[", "Shift + ]", "Ctrl + Q", "Ctrl + =", "Backspace"] {
        assert!(reserved(&chord(text)).is_some(), "{text} is kept");
    }
    for text in ["Q", "Ctrl + Shift + Q", "=", "Alt + Space", "O"] {
        let kept = reserved(&chord(text)).is_some();
        assert_eq!(kept, text == "Alt + Space", "{text}");
    }
}

#[test]
fn the_zbrush_preset_puts_zbrushs_keys_where_a_zbrush_hand_expects_them() {
    let mut keys = shipped();
    keys.set("decimate", Some(chord("Q")));
    keys.apply(ZBRUSH);
    assert_eq!(keys.command(&chord("W")), Some("transform"));
    assert_eq!(keys.command(&chord("D")), Some("level_up"));
    assert_eq!(keys.command(&chord("Shift + D")), Some("level_down"));
    assert_eq!(keys.command(&chord("F")), Some("frame"));
    assert_eq!(keys.command(&chord("U")), Some("hold_strength"));
    assert_eq!(keys.command(&chord("O")), Some("hold_focal"));
    assert_eq!(keys.command(&chord("P")), Some("ortho"));
    assert_eq!(keys.command(&chord("Ctrl + D")), Some("subdivide"), "the keys they share stay");
    assert_eq!(keys.command(&chord("R")), None, "R is ZBrush's rotate, never a surprise remesh");
    assert!(keys.keys("decimate").is_empty(), "a preset starts from the shipped keys");
    assert!(keys.is(ZBRUSH));
    keys.set("decimate", Some(chord("Q")));
    assert!(!keys.is(ZBRUSH), "a change on top is no longer the preset");
    for (id, key) in ZBRUSH {
        assert!(keys.commands().iter().any(|c| c.id == *id), "{id} is a command");
        assert!(key.is_none_or(|k| Chord::parse(k).is_some_and(|c| reserved(&c).is_none())), "{id}'s key");
    }
    // The shipped keys are a preset too: none changed.
    keys.reset_all();
    assert!(keys.is(&[]));
}

#[test]
fn a_renamed_library_keeps_the_changes_and_a_brush_comes_back_with_its_key() {
    let mut keys = shipped();
    keys.set("brush:My Trim", Some(chord("Shift + T")));
    let without = keys.renewed(&[("Clay".to_owned(), Some(chord("2")))], &[]);
    assert_eq!(without.command(&chord("Shift + T")), None, "the brush has gone");
    let back = without.renewed(&[("My Trim".to_owned(), None)], &[]);
    assert_eq!(back.command(&chord("Shift + T")), Some("brush:My Trim"));
}
