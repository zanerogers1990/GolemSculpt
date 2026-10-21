//! Brush files: that every brush goes to a file and comes back the same,
//! that a file with fields missing, unknown or of the wrong kind still
//! reads, that one which is no brush is skipped and left alone, and that
//! the folder follows the library (a file per brush of the user's own,
//! written when the brush changes, moved aside when it is deleted).

use std::path::PathBuf;

use super::*;
use crate::prefs::{BrushPrefs, Home, Prefs};
use crate::sculpt::{FalloffPreset, FlattenMode, SmoothMode, alpha};

/// An empty folder of this test's own.
fn folder(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("sculpt_brush_file_tests").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

fn at(library: &Library, name: &str) -> usize {
    library.named(name).unwrap_or_else(|| panic!("there is no brush called {name}"))
}

#[test]
fn every_brush_goes_to_a_file_and_comes_back_the_same() {
    let mut library = Library::default();
    let mut s = Settings::default();
    // Something of everything on one brush of each kernel.
    for kernel in Brush::ALL {
        library.select(Library::base(kernel), &mut s);
        let slot = kernel.index();
        s.per_brush[slot] = BrushSettings {
            strength: 0.37,
            accumulate: true,
            build_up: 1.7,
            positive: false,
            focal_shift: -0.4,
            front_facing: true,
            stay_in_group: true,
            spacing: 0.55,
            scatter: 0.3,
            size_jitter: 0.25,
            pressure_size: 0.6,
            pressure_strength: 0.2,
            smooth_mode: SmoothMode::Valleys,
            keep_volume: 0.5,
            flatten_mode: FlattenMode::Fill,
            lock_plane: true,
        };
        s.falloff[slot] = Some(FalloffCurve::preset(FalloffPreset::Plateau));
        s.alpha[slot] = alpha::AlphaSettings {
            image: alpha::find("Rake"),
            rotation: 30.0,
            scale: 1.5,
            invert: true,
            tile: true,
            follow: true,
            drag: false,
            random_rotation: 45.0,
        };
        (s.clay_depth, s.clay_softness, s.clay_relax) = (0.9, 0.3, 0.8);
        (s.crease_pinch, s.crease_depth, s.flatten_offset, s.layer_height) = (0.2, 0.7, -0.2, 0.4);
        s.move_connected = true;
        s.pose = crate::sculpt::PoseSettings { mode: PoseMode::Stretch, joints: 3, offset: 1.2, rigidity: 0.1 };
    }
    for (i, entry) in library.entries().iter().enumerate() {
        let record = library.record(i, &s);
        let written = text(&entry.name, entry.kernel, &record, entry.made());
        let back = parse(&written, "fallback").unwrap_or_else(|why| panic!("{}: {why}", entry.name));
        assert_eq!((back.name.as_str(), back.kernel), (entry.name.as_str(), entry.kernel));
        // A record keeps every kernel's numbers; only its own are in a file.
        let own = |r: &Record| Record { numbers: Record::base(entry.kernel).numbers.with_own(entry.kernel, &r.numbers), ..r.clone() };
        assert!(back.record == own(&record), "{} came back changed:\n{written}", entry.name);
        assert!(back.made == own(entry.made()), "{} lost how it was made", entry.name);
        assert!(back.missing_alpha.is_none());
    }
}

#[test]
fn a_file_says_only_what_its_own_kernel_reads() {
    let clay = text("Mine", Brush::Clay, &Record::base(Brush::Clay), &Record::base(Brush::Clay));
    assert!(clay.contains("\"clay_depth\"") && clay.contains("\"strength\""), "{clay}");
    for other in ["crease_depth", "flatten_offset", "layer_height", "move_connected", "pose_mode", "\"alpha\"", "\"falloff\""] {
        assert!(!clay.contains(other), "a plain Clay brush's file has no {other}:\n{clay}");
    }
    let pose = text("Bender", Brush::Pose, &Record::base(Brush::Pose), &Record::base(Brush::Pose));
    assert!(pose.contains("\"pose_mode\": \"Bend\"") && !pose.contains("clay_depth"), "{pose}");
}

#[test]
fn a_file_with_fields_missing_unknown_or_of_the_wrong_kind_still_reads() {
    // The least a brush file can be: its kernel. Everything else is the
    // kernel's base brush, and the brush is named after the file.
    let least = parse(r#"{ "kernel": "crease" }"#, "My Cutter").unwrap();
    assert_eq!((least.name.as_str(), least.kernel), ("My Cutter", Brush::Crease));
    assert!(least.record == Record::base(Brush::Crease) && least.made == least.record);

    let odd = r#"{
        "name": "  Scraper\n ", "kernel": "Flatten", "made_with": "a newer version", "version": 9,
        "brush": {
            "strength": "lots", "spacing": 0.5, "flatten_mode": "sideways", "lock_plane": true,
            "focal_shift": -7, "keep_volume": null, "flatten_offset": 3.0, "clay_depth": 1.4,
            "falloff": "steep", "alpha": 5, "sparkle": true, "pressure_size": 0.4
        }
    }"#;
    let read = parse(odd, "fallback").unwrap();
    let base = Record::base(Brush::Flatten);
    assert_eq!(read.name, "Scraper");
    let b = read.record.settings;
    assert_eq!(b.strength, base.settings.strength, "a word where a number belongs is passed over");
    assert_eq!(b.flatten_mode, FlattenMode::Both, "and so is a mode this version has not heard of");
    assert_eq!((b.spacing, b.lock_plane, b.pressure_size), (0.5, true, 0.4), "without costing the fields around them");
    assert_eq!(b.focal_shift, -1.0, "a number is brought into its slider's range");
    assert_eq!(read.record.numbers.flatten_offset, 0.5);
    assert_eq!(read.record.numbers.clay_depth, base.numbers.clay_depth, "another kernel's number is not a Flatten brush's to set");
    assert!(read.record.falloff.is_none() && read.record.alpha.image.is_none());
    assert!(read.made == read.record, "with no `made`, Reset goes back to the brush as the file has it");
}

#[test]
fn a_file_that_is_no_brush_is_refused_with_the_reason() {
    for (text, why) in [
        ("this is a shopping list", "not JSON"),
        ("[1, 2, 3]", "names no kernel"),
        (r#"{ "name": "Lost", "brush": { "strength": 0.5 } }"#, "names no kernel"),
        (r#"{ "kernel": "Chainsaw" }"#, "Chainsaw"),
        (r#"{ "kernel": 7 }"#, "names no kernel"),
    ] {
        let refused = parse(text, "x").err().unwrap_or_else(|| panic!("{text} was read as a brush"));
        assert!(refused.contains(why), "{text}: {refused}");
    }
}

#[test]
fn a_brush_naming_an_alpha_that_is_not_here_has_none_and_says_so() {
    let text = r#"{ "name": "Lizard", "kernel": "Standard", "feel_version": 1,
        "brush": { "strength": 0.4, "alpha": { "name": "Somebody Else's Scales", "scale": 2.0, "follow": true } } }"#;
    let read = parse(text, "x").unwrap();
    assert!(read.record.alpha.image.is_none());
    assert_eq!(read.record.settings.strength, 0.4, "the rest of the brush is as the file has it");
    assert_eq!(read.missing_alpha.as_deref(), Some("Somebody Else's Scales"));
    let note = read.note().unwrap();
    assert!(note.contains("Lizard") && note.contains("Somebody Else's Scales") && note.contains("no alpha"), "{note}");
    // One that is here is found whatever else the file says, and no note.
    let here = parse(r#"{ "kernel": "Standard", "brush": { "alpha": { "name": "Rake" } } }"#, "x").unwrap();
    assert!(here.record.alpha.image.is_some() && here.note().is_none());
}

#[test]
fn a_brushs_file_name_is_one_windows_takes() {
    assert_eq!(file_stem("Blocking In"), "Blocking In");
    assert_eq!(file_stem("  a/b\\c:d*e?f\"g<h>i|j.k  "), "a_b_c_d_e_f_g_h_i_j_k");
    assert_eq!(file_stem("Écailles 2"), "Écailles 2");
    assert_eq!(file_stem("..."), "___");
    assert_eq!(file_stem(""), "brush");
    for device in ["CON", "nul", "COM1", "Lpt9"] {
        assert_eq!(file_stem(device), format!("brush {device}"), "{device} is a device, not a file");
    }
}

#[test]
fn import_and_export_go_by_path_and_a_taken_name_gets_a_number() {
    let dir = folder("import_export");
    let mut library = Library::default();
    let mut s = Settings::default();
    let rake = at(&library, "Rake");
    library.select(rake, &mut s);
    s.current_mut().strength = 0.21;
    let file = dir.join("given to a friend.json");
    let said = export(&file, &library, rake, &s).unwrap();
    assert!(said.contains("Rake") && file.is_file(), "{said}");
    assert_eq!(names_in(&dir), ["given to a friend.json"], "nothing is left beside it from the write");

    library.select(Library::base(Brush::Move), &mut s);
    let count = library.entries().len();
    let (i, said) = import(&file, &mut library, &mut s).unwrap();
    let entry = &library.entries()[i];
    assert_eq!((entry.name.as_str(), entry.kernel, entry.origin), ("Rake 2", Brush::Standard, Origin::User));
    assert!(said.contains("Rake 2"), "{said}");
    assert_eq!((library.entries().len(), library.selected(&s)), (count + 1, i), "it is in hand");
    assert_eq!(s.current().strength, 0.21);
    assert!(library.record(i, &s).alpha.image.is_some());
    assert_eq!(library.record(rake, &s).settings.strength, 0.21, "and the brush it was exported from is untouched");
    // Reset on the imported brush goes back to how Rake shipped: the file
    // carries that too.
    library.reset(i, &mut s);
    assert!(library.record(i, &s).settings == library.entries()[rake].made().settings);

    let missing = import(&dir.join("not there.json"), &mut library, &mut s);
    assert!(missing.is_err_and(|why| why.contains("not there.json")));
    std::fs::write(dir.join("list.json"), "eggs, milk").unwrap();
    let refused = import(&dir.join("list.json"), &mut library, &mut s).unwrap_err();
    assert!(refused.contains("list.json") && refused.contains("not a brush file"), "{refused}");
    assert_eq!(library.entries().len(), count + 1, "a refused file adds nothing");
}

#[test]
fn the_folder_follows_the_library() {
    let home = folder("follows");
    let dir = home.join(FOLDER);
    let mut shelf = Shelf::new(Some(&home), true);
    let mut library = Library::default();
    let mut s = Settings::default();
    assert!(shelf.load(&mut library, &mut s).is_empty());
    shelf.sync(&library, &s);
    assert!(names_in(&dir).is_empty(), "the base and shipped brushes as they shipped need no files");

    // A brush of the user's own gets a file; an edit to it is written.
    let mine = library.duplicate(at(&library, "Clay Strips"), &mut s);
    shelf.sync(&library, &s);
    assert_eq!(names_in(&dir), ["Clay Strips Copy.json"]);
    let before = std::fs::read_to_string(dir.join("Clay Strips Copy.json")).unwrap();
    s.current_mut().strength = 0.93;
    shelf.sync(&library, &s);
    let after = std::fs::read_to_string(dir.join("Clay Strips Copy.json")).unwrap();
    assert!(before != after && after.contains("0.93"), "{after}");

    // Renamed: the file goes under the new name.
    library.rename(mine, "Blocking: In?").unwrap();
    shelf.sync(&library, &s);
    assert_eq!(names_in(&dir), ["Blocking_ In_.json"]);

    // A shipped brush gets a file only while it differs from how it shipped.
    let trim = at(&library, "Trim");
    library.select(trim, &mut s);
    s.current_mut().strength = 0.11;
    shelf.sync(&library, &s);
    assert_eq!(names_in(&dir), ["Blocking_ In_.json", "Trim.json"]);
    library.reset(trim, &mut s);
    shelf.sync(&library, &s);
    assert_eq!(names_in(&dir), ["Blocking_ In_.json"]);

    // An edit to a base brush goes to the settings file, not here.
    library.select(Library::base(Brush::Clay), &mut s);
    s.current_mut().strength = 0.5;
    shelf.sync(&library, &s);
    assert_eq!(names_in(&dir), ["Blocking_ In_.json"]);

    // Deleted: its file is moved aside, as the brush was, not destroyed.
    let mine = at(&library, "Blocking: In?");
    let aside = shelf.set_aside(&library, mine, &s).expect("a copy is kept");
    library.delete(mine, &mut s).unwrap();
    shelf.sync(&library, &s);
    assert_eq!(names_in(&dir), ["deleted"]);
    assert_eq!(aside, dir.join("deleted").join("Blocking_ In_.json"));
    let kept = read(&aside).unwrap();
    assert_eq!((kept.name.as_str(), kept.record.settings.strength), ("Blocking: In?", 0.93));
    // A shipped brush nobody changed leaves nothing: it is in the app.
    assert!(shelf.set_aside(&library, at(&library, "Fill"), &s).is_none());
}

#[test]
fn the_brushes_come_back_at_the_next_start() {
    let home = folder("next_start");
    let (mut library, mut s) = (Library::default(), Settings::default());
    let mut shelf = Shelf::new(Some(&home), true);
    let mine = library.duplicate(at(&library, "Dam Standard"), &mut s);
    library.rename(mine, "Fine Liner").unwrap();
    s.current_mut().strength = 0.42;
    s.crease_depth = 0.9;
    library.set_pinned(mine, true);
    library.select(at(&library, "Polish"), &mut s);
    s.current_mut().strength = 0.31;
    library.set_shift(at(&library, "Relax"), &mut s);
    library.delete(at(&library, "Spread"), &mut s).unwrap();
    library.select(Library::base(Brush::Clay), &mut s);
    s.current_mut().strength = 0.66;
    library.select(at(&library, "Fine Liner"), &mut s);
    shelf.sync(&library, &s);
    let kept = Kept::of(&library, &s);
    let brush = BrushPrefs::of(&library.base_image(&s));
    assert_eq!(brush.brush, "Crease", "the settings file names the kernel, as it always did");
    assert_eq!(brush.brushes["Crease"].strength, 0.1, "under it the base Crease, not the brush in hand");
    assert_eq!(brush.crease_depth, Settings::default().crease_depth);

    // The next run: the settings file first, then the folder, then the rest.
    let (mut library2, mut s2) = (Library::default(), Settings::default());
    brush.apply(&mut s2);
    let mut shelf2 = Shelf::new(Some(&home), true);
    assert!(shelf2.load(&mut library2, &mut s2).is_empty());
    kept.apply(&mut library2, &mut s2);
    let names = |l: &Library| l.entries().iter().map(|e| (e.name.clone(), e.origin, e.pinned)).collect::<Vec<_>>();
    assert_eq!(names(&library2), names(&library));
    for (i, entry) in library.entries().iter().enumerate() {
        assert!(library2.record(i, &s2).settings == library.record(i, &s).settings, "{} came back changed", entry.name);
        assert!(library2.record(i, &s2).alpha == library.record(i, &s).alpha, "{} lost its alpha", entry.name);
    }
    assert_eq!(library2.entries()[library2.selected(&s2)].name, "Fine Liner");
    assert_eq!((s2.current().strength, s2.crease_depth), (0.42, 0.9));
    assert_eq!(library2.entries()[library2.shift()].name, "Relax");
    assert!(library2.named("Spread").is_none());
    // The folder is read once: asked again, nothing is added a second time.
    let count = library2.entries().len();
    assert!(shelf2.load(&mut library2, &mut s2).is_empty());
    assert_eq!(library2.entries().len(), count);
    // Nothing is written again for brushes that have not changed.
    let stamp = |name: &str| std::fs::metadata(home.join(FOLDER).join(name)).unwrap().modified().unwrap();
    let before = (stamp("Fine Liner.json"), stamp("Polish.json"));
    shelf2.sync(&library2, &s2);
    assert_eq!((stamp("Fine Liner.json"), stamp("Polish.json")), before);
}

#[test]
fn a_bad_file_in_the_folder_is_skipped_said_and_left_alone() {
    let home = folder("bad_file");
    let dir = home.join(FOLDER);
    std::fs::create_dir_all(&dir).unwrap();
    let good = text("Fine Liner", Brush::Crease, &Record::base(Brush::Crease), &Record::base(Brush::Crease));
    std::fs::write(dir.join("Fine Liner.json"), &good).unwrap();
    std::fs::write(dir.join("Half Edited.json"), "{ \"kernel\": \"Clay\", \"brush\": { \"strength\": 0.5 ").unwrap();
    std::fs::write(dir.join("notes.txt"), "not a brush, and not looked at").unwrap();
    // One that reads, with a field from a version to come, written by hand.
    std::fs::write(dir.join("hand made.json"), r#"{ "kernel": "Inflate", "glow": 3, "brush": { "strength": 0.8 } }"#).unwrap();

    let (mut library, mut s) = (Library::default(), Settings::default());
    let mut shelf = Shelf::new(Some(&home), true);
    let notes = shelf.load(&mut library, &mut s);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("Half Edited.json") && notes[0].contains("Skipped"), "{}", notes[0]);
    assert!(library.named("Fine Liner").is_some());
    let hand = at(&library, "hand made");
    // With no feel_version it is from before 2026-10-21: its Strength comes over (`sculpt::strength`).
    let brought = crate::sculpt::strength::from_old(Brush::Inflate, 0.8);
    assert_eq!((library.entries()[hand].kernel, library.record(hand, &s).settings.strength), (Brush::Inflate, brought));

    // Keeping the folder up to date touches neither the bad file nor the
    // hand-made one, whose unknown field is still there afterwards.
    shelf.sync(&library, &s);
    assert!(std::fs::read_to_string(dir.join("Half Edited.json")).unwrap().ends_with("0.5 "));
    assert!(std::fs::read_to_string(dir.join("hand made.json")).unwrap().contains("glow"));
    assert_eq!(std::fs::read_to_string(dir.join("Fine Liner.json")).unwrap(), good);
    // A new brush that would take the bad file's name gets another.
    let copy = library.duplicate(at(&library, "Clay"), &mut s);
    library.rename(copy, "Half Edited").unwrap();
    shelf.sync(&library, &s);
    assert!(names_in(&dir).contains(&"Half Edited 2.json".to_owned()), "{:?}", names_in(&dir));
    assert!(std::fs::read_to_string(dir.join("Half Edited.json")).unwrap().ends_with("0.5 "));
}

#[test]
fn a_run_that_may_not_write_reads_the_folder_and_leaves_it_be() {
    let home = folder("read_only");
    let dir = home.join(FOLDER);
    std::fs::create_dir_all(&dir).unwrap();
    let base = Record::base(Brush::Clay);
    std::fs::write(dir.join("Mine.json"), text("Mine", Brush::Clay, &base, &base)).unwrap();
    let (mut library, mut s) = (Library::default(), Settings::default());
    let mut shelf = Shelf::new(Some(&home), false);
    shelf.load(&mut library, &mut s);
    let mine = at(&library, "Mine");
    library.select(mine, &mut s);
    s.current_mut().strength = 0.9;
    library.duplicate(mine, &mut s);
    shelf.sync(&library, &s);
    assert!(shelf.set_aside(&library, mine, &s).is_none());
    assert_eq!(names_in(&dir), ["Mine.json"]);
    assert!(!std::fs::read_to_string(dir.join("Mine.json")).unwrap().contains("0.9"));
    // And one with no folder at all keeps nothing and breaks nothing.
    let mut nowhere = Shelf::new(None, false);
    assert!(nowhere.load(&mut library, &mut s).is_empty());
    nowhere.sync(&library, &s);
}

/// A settings file from before there was a library, of each age: the brush
/// values it holds become the base brushes', and nothing else moves.
#[test]
fn settings_files_from_before_the_library_carry_into_the_base_brushes() {
    // As phase 2 wrote it: per-brush stroke settings and kernel options.
    let phase_2 = r#"{ "brush": {
        "brush": "Flatten", "radius_px": 84.0,
        "brushes": {
            "Clay": { "strength": 0.45, "accumulate": false, "spacing": 0.3, "pressure_size": 0.5, "pressure_strength": 0.4 },
            "Smooth": { "strength": 0.6, "smooth_mode": "peaks", "keep_volume": 0.7 },
            "Flatten": { "strength": 0.25, "flatten_mode": "scrape", "lock_plane": true },
            "Pinch": { "strength": 0.2 }
        },
        "falloff": { "Clay": [[0.0, 1.0], [0.5, 0.9], [1.0, 0.0]] },
        "alpha": { "Standard": { "name": "Rake", "follow": true } },
        "clay_depth": 0.8, "crease_pinch": 0.2, "flatten_offset": 0.1, "layer_height": 0.3,
        "move_connected": true, "move_strength": 0.7, "pose_mode": "Stretch", "pressure_curve": 2.0
    } }"#;
    // As phase 1 wrote it: no kernel options yet.
    let phase_1 = r#"{ "brush": {
        "brush": "Clay",
        "brushes": { "Clay": { "strength": 0.45, "spacing": 0.3, "scatter": 0.2, "pressure_strength": 0.4 } },
        "clay_depth": 0.8, "move_strength": 0.7
    } }"#;
    // From before brushes had pressure settings of their own: one floor.
    let older = r#"{ "brush": {
        "brush": "Crease", "brushes": { "Crease": { "strength": 0.75, "focal_shift": -0.3 } },
        "crease_depth": 0.5, "pressure_floor": 0.3
    } }"#;

    let start = |text: &str| {
        let prefs = Prefs::parse(text);
        assert_eq!(prefs.library, Kept::default(), "an older file says nothing of the library");
        let (mut library, mut s) = (Library::default(), Settings::default());
        prefs.brush.apply(&mut s);
        prefs.library.apply(&mut library, &mut s);
        (library, s)
    };

    let (library, s) = start(phase_2);
    assert_eq!(library.selected(&s), Library::base(Brush::Flatten), "the kernel it was left on is its base brush");
    let record = |kernel: Brush| library.record(Library::base(kernel), &s);
    let clay = record(Brush::Clay);
    assert_eq!((clay.settings.strength, clay.settings.accumulate, clay.settings.spacing), (0.45, false, 0.3));
    assert_eq!((clay.settings.pressure_size, clay.settings.pressure_strength, clay.numbers.clay_depth), (0.5, 0.4, 0.8));
    assert_eq!(clay.falloff.as_ref().map(|curve| curve.points.len()), Some(3));
    let smooth = record(Brush::Smooth).settings;
    // Strengths on the kernels whose slider is now mapped come over as what does the same (`sculpt::strength`).
    let brought = |kernel: Brush, old: f32| crate::sculpt::strength::from_old(kernel, old);
    assert_eq!((smooth.strength, smooth.smooth_mode, smooth.keep_volume), (brought(Brush::Smooth, 0.6), SmoothMode::Peaks, 0.7));
    let flatten = record(Brush::Flatten);
    assert_eq!((flatten.settings.flatten_mode, flatten.settings.lock_plane, flatten.numbers.flatten_offset), (FlattenMode::Scrape, true, 0.1));
    assert!(record(Brush::Standard).alpha.image.is_some() && record(Brush::Standard).alpha.follow);
    assert_eq!((record(Brush::Crease).numbers.crease_pinch, record(Brush::Layer).numbers.layer_height), (0.2, 0.3));
    assert_eq!((record(Brush::Move).settings.strength, record(Brush::Move).numbers.move_connected), (0.7, true));
    assert_eq!(record(Brush::Pose).numbers.pose.mode, PoseMode::Stretch);
    assert_eq!((record(Brush::Pinch).settings.strength, s.radius_px, s.pressure_curve), (brought(Brush::Pinch, 0.2), 84.0, 2.0));
    // The shipped brushes are as they shipped, whatever the old file held:
    // its values were the base brushes'.
    let fresh = Library::default();
    for (i, entry) in library.entries().iter().enumerate().filter(|(_, e)| e.origin == Origin::Shipped) {
        assert!(library.record(i, &s) == fresh.record(i, &Settings::default()), "{} took something from the old file", entry.name);
    }
    assert_eq!(library.shift(), Library::base(Brush::Smooth));
    assert!(s.shift.is_none(), "Shift smooths with the base Smooth, with the old file's Peaks and Keep Volume");

    let (library, s) = start(phase_1);
    let clay = library.record(Library::base(Brush::Clay), &s);
    assert_eq!(library.selected(&s), Library::base(Brush::Clay));
    assert_eq!((clay.settings.strength, clay.settings.spacing, clay.settings.scatter), (0.45, 0.3, 0.2));
    assert_eq!((clay.settings.pressure_strength, clay.numbers.clay_depth), (0.4, 0.8));
    assert_eq!(clay.settings.smooth_mode, SmoothMode::All, "what the file had not heard of keeps its start");

    let (library, s) = start(older);
    let crease = library.record(Library::base(Brush::Crease), &s);
    assert_eq!(library.selected(&s), Library::base(Brush::Crease));
    assert_eq!((crease.settings.strength, crease.settings.focal_shift, crease.numbers.crease_depth), (0.75, -0.3, 0.5));
    for kernel in Brush::ALL {
        assert_eq!(library.record(Library::base(kernel), &s).settings.pressure_strength, 0.7, "{}", kernel.name());
    }
}

/// The settings file after this version has written it still opens in the
/// version before, and means the same there: the base brushes are where
/// they were, by the same names.
#[test]
fn the_settings_file_keeps_its_old_shape() {
    let home = folder("old_shape");
    let (mut library, mut s) = (Library::default(), Settings::default());
    library.select(Library::base(Brush::Clay), &mut s);
    s.current_mut().strength = 0.45;
    library.select(at(&library, "Clay Strips"), &mut s);
    let mut prefs = Prefs::default();
    prefs.brush = BrushPrefs::of(&library.base_image(&s));
    prefs.library = Kept::of(&library, &s);
    let settings = Home::at(&home);
    settings.save(&prefs).unwrap();
    assert_eq!(settings.load(), prefs);
    let written: Value = serde_json::from_str(&std::fs::read_to_string(home.join("settings.json")).unwrap()).unwrap();
    assert_eq!(written["brush"]["brush"], "Clay");
    assert_eq!(written["brush"]["brushes"]["Clay"]["strength"].as_f64().map(|v| (v * 100.0).round()), Some(45.0));
    assert!(written["brush"]["alpha"].get("Clay").is_none(), "Clay Strips' alpha is not the base Clay's");
    assert_eq!(written["library"]["selected"], "Clay Strips");
    assert_eq!(written["library"]["shift"], "Smooth");
    // Every base brush but Erase Layer and Paint, which start off the wheel.
    assert_eq!(written["library"]["pinned"].as_array().map(Vec::len), Some(Brush::COUNT - 2));
}

/// A brush file from before 2026-10-21 (no `feel_version`): its Strength on
/// a kernel whose slider is now mapped becomes the one that does what its
/// number did, for the brush and for what Reset goes back to; the rest of
/// it is the user's and stays. A dragged print's Strength is its height and
/// is not touched, nor is a file this version wrote.
#[test]
fn an_old_brush_files_strength_does_what_it_did() {
    use crate::sculpt::strength::{effective, from_old};
    let old = r#"{ "name": "My Trim", "kernel": "Flatten",
        "brush": { "strength": 0.755, "pressure_strength": 0.88, "front_facing": false, "flatten_mode": "scrape" },
        "made": { "strength": 0.6 } }"#;
    let read = parse(old, "x").unwrap();
    let (now, made) = (read.record.settings, read.made.settings);
    assert!((effective(Brush::Flatten, now.strength) - 0.755).abs() < 1e-5, "{}", now.strength);
    assert_eq!(made.strength, from_old(Brush::Flatten, 0.6));
    assert_eq!((now.pressure_strength, now.front_facing, now.flatten_mode), (0.88, false, FlattenMode::Scrape));
    // Clay is not mapped.
    let clay = parse(r#"{ "kernel": "Clay", "brush": { "strength": 0.21 } }"#, "c").unwrap();
    assert_eq!(clay.record.settings.strength, 0.21);
    // A print dragged out keeps its height.
    let print = r#"{ "kernel": "Layer", "brush": { "strength": 0.5, "alpha": { "name": "Scales", "drag": true } } }"#;
    assert_eq!(parse(print, "p").unwrap().record.settings.strength, 0.5);
    // Written by this version, read back as it is.
    let written = text("My Trim", Brush::Flatten, &read.record, &read.made);
    assert!(written.contains("\"feel_version\""), "{written}");
    let again = parse(&written, "x").unwrap();
    assert_eq!(again.record.settings.strength, now.strength);
    assert_eq!(again.made.settings.strength, made.strength);
}
