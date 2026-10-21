//! A settings file from before 2026-10-21, when the brushes' numbers meant
//! something else: its numbers come over once, a starting value becoming
//! today's starting value and a chosen one doing what it did.

use super::*;
use crate::sculpt::strength;

/// A settings file as the version before wrote it: today's, less the key
/// that says what its numbers mean and the keys it did not have, with the
/// old starting values in it.
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
        tuning["accumulate"] = ACCUMULATED.contains(&b).into();
        let tuning = tuning.as_object_mut().unwrap();
        assert!(tuning.remove("relax_normals").is_some() && tuning.remove("steady_plane").is_some());
    }
    file
}

/// A settings file as the first pen-feel version wrote it (version 1): the
/// numbers it would have written for `settings`, with Accumulate on where it
/// then started on, and no Relax Normals or Steady Plane.
fn version_1_file(settings: &Settings) -> serde_json::Value {
    let mut file = serde_json::to_value(Prefs { brush: BrushPrefs::of(settings), ..Prefs::default() }).unwrap();
    file["brush"]["feel_version"] = 1.into();
    for (name, tuning) in file["brush"]["brushes"].as_object_mut().unwrap() {
        let b = Brush::ALL.into_iter().find(|b| b.name() == name).unwrap();
        if ACCUMULATED.contains(&b) {
            tuning["accumulate"] = true.into();
        }
        let tuning = tuning.as_object_mut().unwrap();
        tuning.remove("relax_normals");
        tuning.remove("steady_plane");
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
    for b in Brush::ALL {
        let (got, want) = (settings.per_brush[b.index()], today.per_brush[b.index()]);
        assert_eq!(
            (got.accumulate, got.relax_normals, got.steady_plane),
            (want.accumulate, want.relax_normals, want.steady_plane),
            "{}: Accumulate starts off, Relax Normals and Steady Plane where they start",
            b.name()
        );
    }
}

/// A version 1 file (Accumulate starting on): its Accumulate on Standard,
/// Inflate and Clay was never chosen and goes off, the new settings take
/// their starting values, and nothing else it says changes.
#[test]
fn a_version_1_file_has_accumulate_go_off_once() {
    let mut chosen = Settings::default();
    chosen.per_brush[Brush::Standard.index()].strength = 0.7;
    chosen.per_brush[Brush::Clay.index()].strength = 0.35;
    chosen.per_brush[Brush::Inflate.index()].front_facing = false;
    chosen.per_brush[Brush::Crease.index()].accumulate = true;
    let s = read(&version_1_file(&chosen));
    let at = |b: Brush| s.per_brush[b.index()];
    for b in ACCUMULATED {
        assert!(!at(b).accumulate, "{}", b.name());
    }
    assert!(at(Brush::Crease).accumulate, "Crease's started off: on was chosen");
    assert_eq!((at(Brush::Standard).strength, at(Brush::Clay).strength), (0.7, 0.35));
    assert!(!at(Brush::Inflate).front_facing);
    assert_eq!(at(Brush::Inflate).relax_normals, crate::sculpt::DEFAULT_RELAX_NORMALS);
    assert_eq!((at(Brush::Clay).steady_plane, at(Brush::Flatten).steady_plane), (0.5, 0.5));
    // Read once, written back, read again: as it was.
    let written = serde_json::to_value(Prefs { brush: BrushPrefs::of(&s), ..Prefs::default() }).unwrap();
    assert_eq!(written["brush"]["feel_version"], FEEL_VERSION);
    assert_eq!(read(&written).per_brush, s.per_brush);
}

/// Standard with Accumulate chosen off, in a file from before version 2:
/// its Strength becomes the one that builds a pass about as tall as it did.
#[test]
fn a_standard_set_to_build_to_a_height_keeps_its_height() {
    let mut file = old_file();
    file["brush"]["brushes"]["Standard"]["accumulate"] = false.into();
    file["brush"]["brushes"]["Standard"]["strength"] = 0.3.into();
    let got = read(&file).per_brush[Brush::Standard.index()];
    assert!(!got.accumulate);
    assert!((got.strength - strength::from_old_seeking(0.3)).abs() < 1e-6, "{}", got.strength);
    // A version 1 file says it in the slider's units of then.
    let mut s = Settings::default();
    (s.per_brush[Brush::Standard.index()].strength, s.per_brush[Brush::Standard.index()].accumulate) = (0.6, false);
    let mut file = version_1_file(&s);
    file["brush"]["brushes"]["Standard"]["accumulate"] = false.into();
    let got = read(&file).per_brush[Brush::Standard.index()];
    let then = strength::effective(Brush::Standard, 0.6);
    assert!((got.strength - strength::from_old_seeking(then)).abs() < 1e-6);
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
