//! A settings file from before 2026-10-21, when the brushes' numbers meant
//! something else: its numbers come over once, a starting value becoming
//! today's starting value and a chosen one doing what it did.

use super::*;
use crate::sculpt::strength;

/// A settings file as the version before wrote it: today's, less the key
/// that says what its numbers mean, with the old starting values in it.
fn old_file() -> serde_json::Value {
    let mut file = serde_json::to_value(Prefs::default()).unwrap();
    let brush = file["brush"].as_object_mut().unwrap();
    assert!(brush.remove("feel_version").is_some(), "feel_version is in the file");
    brush.insert("move_strength".into(), 0.5.into());
    for (name, tuning) in brush["brushes"].as_object_mut().unwrap() {
        let b = Brush::ALL.into_iter().find(|b| b.name() == name).unwrap();
        tuning["strength"] = old_default_strength(b).into();
        tuning["pressure_strength"] = 0.88.into();
        tuning["front_facing"] = false.into();
    }
    file
}

fn read(file: &serde_json::Value) -> Settings {
    let mut settings = Settings::default();
    Prefs::parse(&file.to_string()).brush.apply(&mut settings);
    settings
}

/// The old starting values were never chosen: they become today's, so the
/// new starting points reach a settings file that has the old ones in it.
#[test]
fn an_old_files_starting_values_become_todays() {
    let settings = read(&old_file());
    let today = Settings::default();
    for b in Brush::ALL {
        let (got, want) = (settings.per_brush[b.index()], today.per_brush[b.index()]);
        assert_eq!(
            (got.strength, got.pressure_strength, got.front_facing),
            (want.strength, want.pressure_strength, want.front_facing),
            "{}",
            b.name()
        );
    }
    assert_eq!(settings.per_brush[Brush::Move.index()].strength, 1.0, "Move follows the pen all the way");
}

/// What was chosen is kept for what it did: a Strength on a mapped kernel
/// becomes the one that does the same, as near as the slider reaches;
/// Pressure -> Strength and Front Facing as they were set. Snake Hook's
/// Strength did nothing, so whatever it was left at goes.
#[test]
fn an_old_files_chosen_values_keep_what_they_did() {
    let mut file = old_file();
    let brushes = &mut file["brush"]["brushes"];
    // Zane's own: Standard and Inflate at 40%, Layer and Crease at 80%, Clay at 50%.
    brushes["Standard"]["strength"] = 0.4.into();
    brushes["Inflate"]["strength"] = 0.03.into();
    brushes["Layer"]["strength"] = 0.8.into();
    brushes["Crease"]["strength"] = 0.8.into();
    brushes["Clay"]["strength"] = 0.5.into();
    brushes["Smooth"]["strength"] = 0.3.into();
    brushes["Snake Hook"]["strength"] = 0.7.into();
    brushes["Smooth"]["pressure_strength"] = 0.5.into();
    brushes["Crease"]["front_facing"] = true.into();
    file["brush"]["move_strength"] = 0.3.into();
    let s = read(&file);
    let at = |b: Brush| s.per_brush[b.index()];
    assert_eq!(at(Brush::Standard).strength, 1.0, "past what 100% now does");
    assert!((strength::effective(Brush::Inflate, at(Brush::Inflate).strength) - 0.03).abs() < 1e-5);
    assert_eq!(at(Brush::Layer).strength, 1.0);
    assert_eq!((at(Brush::Crease).strength, at(Brush::Clay).strength), (0.8, 0.5), "Clay and Crease are not mapped");
    assert!((at(Brush::Smooth).strength - 0.448).abs() < 1e-3);
    assert_eq!(at(Brush::SnakeHook).strength, 1.0);
    assert_eq!(at(Brush::Smooth).pressure_strength, 0.5);
    assert!(at(Brush::Crease).front_facing);
    assert_eq!(at(Brush::Move).strength, 0.3);
}

/// Brought over once: what this version writes says so, and reads back as
/// it was written, however far from the old numbers it is.
#[test]
fn a_file_this_version_wrote_reads_back_as_it_is() {
    let mut settings = read(&old_file());
    settings.per_brush[Brush::Standard.index()].strength = 0.1;
    settings.per_brush[Brush::Standard.index()].front_facing = false;
    settings.per_brush[Brush::Move.index()].strength = 0.5;
    let written = serde_json::to_value(Prefs { brush: BrushPrefs::of(&settings), ..Prefs::default() }).unwrap();
    assert_eq!(written["brush"]["feel_version"], FEEL_VERSION);
    let back = read(&written);
    let standard = back.per_brush[Brush::Standard.index()];
    assert_eq!((standard.strength, standard.front_facing), (0.1, false));
    assert_eq!(back.per_brush[Brush::Move.index()].strength, 0.5);
}

/// An older file's one Light-Touch Floor at its starting value: today's
/// Pressure -> Strength; at any other, that floor for every brush, as before.
#[test]
fn an_old_light_touch_floor_at_its_start_gives_way_to_todays() {
    let floor = |value: f32| {
        let mut file = old_file();
        file["brush"]["pressure_floor"] = value.into();
        for (_, tuning) in file["brush"]["brushes"].as_object_mut().unwrap() {
            tuning.as_object_mut().unwrap().remove("pressure_strength");
        }
        read(&file)
    };
    assert!(floor(0.12).per_brush.iter().all(|b| b.pressure_strength == 1.0));
    assert!(floor(0.3).per_brush.iter().all(|b| (b.pressure_strength - 0.7).abs() < 1e-6));
}
