//! The brush library: that picking a brush never changes another, whether
//! the two share a kernel or not; duplicate, rename, delete and Reset; the
//! brush Shift smooths with; and that a sculptor nobody picks a brush on
//! strokes exactly as one did before there was a library.

use super::library::{Library, Numbers, Origin, Record};
use super::stroke_tests::{across, draw, fingerprint, sculptor_with, sphere, view_of};
use super::*;

/// Every brush's settings as they are now, by name.
fn all_records(library: &Library, s: &Settings) -> Vec<(String, Record)> {
    library.entries().iter().enumerate().map(|(i, e)| (e.name.clone(), library.record(i, s))).collect()
}

/// A library with its settings, and the place of the brush called `name`.
fn at(library: &Library, name: &str) -> usize {
    library.named(name).unwrap_or_else(|| panic!("there is no brush called {name}"))
}

/// Changes about everything a record holds of the brush in hand, the way
/// the panel does: straight in the working settings, and of the kernels'
/// numbers only the ones the panel shows for that brush.
fn scribble(s: &mut Settings, salt: f32) {
    let slot = s.brush.index();
    let b = &mut s.per_brush[slot];
    b.strength = 0.11 + salt;
    b.focal_shift = -0.3 + salt;
    b.spacing = 0.9 + salt;
    b.scatter = 0.4 + salt;
    b.pressure_size = 0.5 + salt;
    b.accumulate = !b.accumulate;
    b.front_facing = !b.front_facing;
    b.smooth_mode = SmoothMode::Valleys;
    b.flatten_mode = FlattenMode::Fill;
    s.falloff[slot] = Some(FalloffCurve::preset(FalloffPreset::Plateau));
    s.alpha[slot].scale = 1.7 + salt;
    s.alpha[slot].follow = !s.alpha[slot].follow;
    match s.brush {
        Brush::Clay => (s.clay_depth, s.clay_softness, s.clay_relax) = (0.7 + salt, 0.2 + salt, 0.5 + salt),
        Brush::Crease => (s.crease_pinch, s.crease_depth) = (0.1 + salt, 0.8 + salt),
        Brush::Flatten => s.flatten_offset = 0.2 + salt,
        Brush::Layer => s.layer_height = 0.6 + salt,
        Brush::Move => s.move_connected = !s.move_connected,
        Brush::Pose => s.pose.rigidity = 0.2 + salt,
        _ => {}
    }
}

#[test]
fn the_library_starts_as_the_thirteen_base_brushes_then_the_shipped_ones() {
    let library = Library::default();
    let s = Settings::default();
    for (i, kernel) in Brush::ALL.into_iter().enumerate() {
        let entry = &library.entries()[i];
        assert_eq!((entry.name.as_str(), entry.kernel, entry.origin), (kernel.name(), kernel, Origin::Base));
        // All but Erase Layer, which is pinned by whoever wants it there.
        assert_eq!(entry.pinned, kernel != Brush::EraseLayer, "{}: the base brushes start on the wheel", entry.name);
        assert!(library.record(i, &s) == Record::base(kernel), "{} starts as its kernel does", entry.name);
    }
    let shipped = &library.entries()[Brush::COUNT..];
    assert!(shipped.len() >= 10, "a starter library ships");
    assert!(shipped.iter().all(|e| e.origin == Origin::Shipped && !e.pinned));
    for name in ["Clay Strips", "Dam Standard", "Trim", "Smooth Peaks", "Rake"] {
        assert!(library.named(name).is_some(), "{name} ships");
    }
    // Names are what files, the bridge and the settings file know a brush
    // by: no two the same, whatever the case.
    for (i, a) in library.entries().iter().enumerate() {
        assert!(library.entries()[i + 1..].iter().all(|b| !a.name.eq_ignore_ascii_case(&b.name)), "{} twice", a.name);
        assert!(library::about(&a.name).is_some() == (a.origin == Origin::Shipped), "{} says what it is for", a.name);
    }
    assert_eq!(library.shift(), Library::base(Brush::Smooth), "Shift starts on the base Smooth");
}

#[test]
fn every_shipped_brush_with_an_alpha_has_its_picture() {
    let library = Library::default();
    let s = Settings::default();
    for name in ["Clay Strips", "Rake", "Surface Noise", "Scales"] {
        let Some(i) = library.named(name) else { continue };
        assert!(library.record(i, &s).alpha.image.is_some(), "{name} found its picture");
    }
}

#[test]
fn a_sculptor_nobody_picks_a_brush_on_has_the_settings_it_always_had() {
    let fresh = Sculptor::default();
    let was = Settings::default();
    assert!(fresh.settings.shift.is_none(), "Shift smooths with the working Smooth, as it did");
    for kernel in Brush::ALL {
        let i = kernel.index();
        assert_eq!(fresh.settings.per_brush[i], was.per_brush[i]);
        assert_eq!(fresh.library.selected(&Settings { brush: kernel, ..was.clone() }), Library::base(kernel));
    }
}

#[test]
fn picking_a_brush_never_changes_another_of_the_same_kernel_or_of_a_different_one() {
    let mut library = Library::default();
    let mut s = Settings::default();
    let names: Vec<String> = library.entries().iter().map(|e| e.name.clone()).collect();
    // Walk the whole library twice in an order that mixes kernels, editing
    // each brush in hand, and check after every pick and every edit that no
    // other brush moved.
    let order: Vec<usize> = (0..names.len()).map(|i| (i * 7) % names.len()).chain((0..names.len()).rev()).collect();
    for (step, &i) in order.iter().enumerate() {
        let before = all_records(&library, &s);
        library.select(i, &mut s);
        assert_eq!(library.selected(&s), i);
        assert_eq!(s.brush, library.entries()[i].kernel);
        let after = all_records(&library, &s);
        for ((name, was), (_, now)) in before.iter().zip(&after) {
            assert!(was == now, "picking {} changed {name}", names[i]);
        }
        scribble(&mut s, step as f32 * 0.001);
        let edited = all_records(&library, &s);
        for (at, ((name, was), (_, now))) in after.iter().zip(&edited).enumerate() {
            assert!(at == i || was == now, "editing {} changed {name}", names[i]);
        }
        assert!(edited[i].1 != after[i].1, "the edit to {} took", names[i]);
    }
}

#[test]
fn an_edit_sticks_to_its_brush_through_any_number_of_picks() {
    let mut library = Library::default();
    let mut s = Settings::default();
    let (strips, tubes) = (at(&library, "Clay Strips"), at(&library, "Clay Tubes"));
    library.select(strips, &mut s);
    s.current_mut().strength = 0.77;
    s.clay_depth = 1.1;
    library.select(tubes, &mut s);
    assert!(s.current().strength != 0.77 && s.clay_depth != 1.1, "Clay Tubes has its own");
    s.current_mut().strength = 0.21;
    library.select(Library::base(Brush::Smooth), &mut s);
    library.select(Library::base(Brush::Clay), &mut s);
    assert_eq!((s.current().strength, s.clay_depth), (0.1, 0.3), "the base Clay is untouched");
    library.select(strips, &mut s);
    assert_eq!((s.current().strength, s.clay_depth), (0.77, 1.1));
    library.select(tubes, &mut s);
    assert_eq!(s.current().strength, 0.21);
}

#[test]
fn a_kernels_numbers_belong_to_its_brushes_alone() {
    let mut library = Library::default();
    let mut s = Settings::default();
    // Editing Crease's numbers while a Clay brush is in hand cannot be done
    // from the panel, but the bridge and old code can write any field: the
    // numbers stay with whichever brush of THEIR kernel is live.
    library.select(at(&library, "Clay Strips"), &mut s);
    s.crease_depth = 0.9;
    library.select(at(&library, "Clay Tubes"), &mut s);
    assert_eq!(s.crease_depth, 0.9, "a Clay brush did not carry Crease's depth off");
    library.select(at(&library, "Dam Standard"), &mut s);
    assert!(s.crease_depth != 0.9, "Dam Standard cuts to its own depth");
    library.select(Library::base(Brush::Crease), &mut s);
    assert_eq!(s.crease_depth, 0.9, "and the base Crease kept the edit made while it was live");
}

#[test]
fn numbers_move_only_for_their_own_kernel() {
    let d = Numbers::of(&Settings::default());
    let mut other = d;
    (other.clay_depth, other.crease_depth, other.flatten_offset, other.layer_height) = (1.0, 1.0, 0.4, 0.9);
    other.move_connected = true;
    other.pose.rigidity = 0.1;
    assert_eq!(d.with_own(Brush::Clay, &other), Numbers { clay_depth: 1.0, ..d });
    assert_eq!(d.with_own(Brush::Crease, &other), Numbers { crease_depth: 1.0, ..d });
    assert_eq!(d.with_own(Brush::Flatten, &other), Numbers { flatten_offset: 0.4, ..d });
    assert_eq!(d.with_own(Brush::Layer, &other), Numbers { layer_height: 0.9, ..d });
    assert_eq!(d.with_own(Brush::Move, &other), Numbers { move_connected: true, ..d });
    assert_eq!(d.with_own(Brush::Pose, &other), Numbers { pose: other.pose, ..d });
    for kernel in [Brush::Smooth, Brush::Inflate, Brush::Standard, Brush::SnakeHook, Brush::Pinch, Brush::Nudge] {
        assert_eq!(d.with_own(kernel, &other), d, "{} has no numbers of its own", kernel.name());
    }
}

#[test]
fn picking_a_brush_leaves_mask_painting() {
    let mut library = Library::default();
    let mut s = Settings { mask_mode: true, ..Settings::default() };
    library.select(at(&library, "Rake"), &mut s);
    assert!(!s.mask_mode);
    assert!(library.select_named("  dam standard ", &mut s), "names are found whatever their case");
    assert_eq!(library.entries()[library.selected(&s)].name, "Dam Standard");
    assert!(!library.select_named("No Such Brush", &mut s));
    assert_eq!(library.entries()[library.selected(&s)].name, "Dam Standard", "a wrong name changes nothing");
}

#[test]
fn a_duplicate_is_the_users_own_and_starts_as_the_brush_is_now() {
    let mut library = Library::default();
    let mut s = Settings::default();
    let rake = at(&library, "Rake");
    library.select(rake, &mut s);
    s.current_mut().strength = 0.33;
    let copy = library.duplicate(rake, &mut s);
    let entry = &library.entries()[copy];
    assert_eq!((entry.name.as_str(), entry.kernel, entry.origin), ("Rake Copy", Brush::Standard, Origin::User));
    assert_eq!(library.selected(&s), copy, "the copy is in hand");
    assert!(library.record(copy, &s) == library.record(rake, &s), "and is the brush as it was, edit and all");
    // The two go their own ways from here.
    s.current_mut().strength = 0.9;
    assert_eq!(library.record(rake, &s).settings.strength, 0.33);
    assert_eq!(library.record(copy, &s).settings.strength, 0.9);
    // A second copy gets a name of its own, and so does a copy of a copy.
    let second = library.duplicate(rake, &mut s);
    assert_eq!(library.entries()[second].name, "Rake Copy 2");
    let third = library.duplicate(second, &mut s);
    assert_eq!(library.entries()[third].name, "Rake Copy 2 Copy");
    // A brush that is not in hand can be duplicated too.
    let far = library.duplicate(at(&library, "Trim"), &mut s);
    assert_eq!((library.entries()[far].name.as_str(), s.brush), ("Trim Copy", Brush::Flatten));
    assert_eq!(library.record(far, &s).settings.flatten_mode, FlattenMode::Scrape);
}

#[test]
fn only_the_users_brushes_can_be_renamed_and_never_to_a_name_in_use() {
    let mut library = Library::default();
    let mut s = Settings::default();
    let copy = library.duplicate(at(&library, "Clay Strips"), &mut s);
    assert!(library.rename(Library::base(Brush::Clay), "Mud").is_err(), "a base brush keeps its name");
    assert!(library.rename(at(&library, "Trim"), "Shave").is_err(), "and so does a shipped one");
    assert!(library.rename(copy, "   ").is_err(), "a brush needs a name");
    assert!(library.rename(copy, "clay").is_err(), "not a base brush's name");
    assert!(library.rename(copy, "DAM STANDARD").is_err(), "nor a shipped one's");
    assert!(library.rename(copy, &"x".repeat(library::MAX_NAME + 1)).is_err(), "nor one too long for the list");
    assert_eq!(library.entries()[copy].name, "Clay Strips Copy", "a refused name changes nothing");
    assert!(library.rename(copy, "  Blocking In ").is_ok());
    assert_eq!(library.entries()[copy].name, "Blocking In");
    assert!(library.rename(copy, "blocking in").is_ok(), "a brush may be given its own name again");
    assert_eq!(library.selected(&s), copy, "renaming does not put the brush down");
}

#[test]
fn deleting_a_brush_hands_its_kernels_base_brush_back() {
    let mut library = Library::default();
    let mut s = Settings::default();
    assert!(library.delete(Library::base(Brush::Clay), &mut s).is_err(), "the base brushes stay");
    let count = library.entries().len();

    // In hand: the kernel's base brush takes its place, as it was left.
    library.select(Library::base(Brush::Clay), &mut s);
    s.current_mut().strength = 0.44;
    let strips = at(&library, "Clay Strips");
    library.select(strips, &mut s);
    let others = all_records(&library, &s);
    library.delete(strips, &mut s).unwrap();
    assert_eq!(library.entries().len(), count - 1);
    assert!(library.named("Clay Strips").is_none());
    assert_eq!((library.selected(&s), s.current().strength), (Library::base(Brush::Clay), 0.44));
    assert_eq!(library.removed(), ["Clay Strips".to_owned()], "a shipped brush is remembered as deleted");
    for (name, was) in others.iter().filter(|(name, _)| name != "Clay Strips") {
        assert!(library.record(at(&library, name), &s) == *was, "deleting Clay Strips changed {name}");
    }

    // Live but not in hand (another kernel is): the same, and the brush in
    // hand stays in hand.
    library.select(at(&library, "Trim"), &mut s);
    library.select(at(&library, "Rake"), &mut s);
    let others = all_records(&library, &s);
    library.delete(at(&library, "Trim"), &mut s).unwrap();
    assert_eq!(library.entries()[library.selected(&s)].name, "Rake");
    assert_eq!(library.selected(&Settings { brush: Brush::Flatten, ..s.clone() }), Library::base(Brush::Flatten));
    assert_eq!(s.per_brush[Brush::Flatten.index()].flatten_mode, FlattenMode::Both, "the base Flatten is back in its set");
    for (name, was) in others.iter().filter(|(name, _)| name != "Trim") {
        assert!(library.record(at(&library, name), &s) == *was, "deleting Trim changed {name}");
    }

    // One of the user's own leaves no trace, and its name is free again.
    let copy = library.duplicate(at(&library, "Rake"), &mut s);
    library.delete(copy, &mut s).unwrap();
    assert_eq!(library.removed().len(), 2);
    assert!(library.name_refused("Rake Copy", None).is_none());
    assert!(library.name_refused("Trim", None).is_some(), "a deleted shipped brush keeps its name for coming back");
}

#[test]
fn deleted_shipped_brushes_come_back_as_they_shipped() {
    let mut library = Library::default();
    let mut s = Settings::default();
    let fresh: Vec<String> = library.entries().iter().map(|e| e.name.clone()).collect();
    let mine = library.duplicate(at(&library, "Relax"), &mut s);
    s.current_mut().strength = 0.61;
    library.set_shift(mine, &mut s);
    for name in ["Trim", "Clay Tubes"] {
        library.delete(at(&library, name), &mut s).unwrap();
    }
    library.restore_shipped();
    assert!(library.removed().is_empty());
    let names: Vec<String> = library.entries().iter().map(|e| e.name.clone()).collect();
    assert_eq!(names[..fresh.len()], fresh[..], "the shipped brushes are back in their places");
    assert_eq!(names.last().map(String::as_str), Some("Relax Copy"));
    let mine = at(&library, "Relax Copy");
    assert_eq!((library.selected(&s), library.shift()), (mine, mine), "the user's brush is still in hand, and Shift's");
    assert_eq!(s.current().strength, 0.61);
}

#[test]
fn reset_puts_a_brush_back_as_it_shipped_or_was_made() {
    let mut library = Library::default();
    let mut s = Settings::default();
    // A base brush, in hand.
    library.select(Library::base(Brush::Crease), &mut s);
    scribble(&mut s, 0.0);
    library.reset(Library::base(Brush::Crease), &mut s);
    assert!(library.record(Library::base(Brush::Crease), &s) == Record::base(Brush::Crease));
    assert_eq!(s.crease_depth, Settings::default().crease_depth, "its numbers are back in the working set");
    // A shipped one, not in hand.
    let dam = at(&library, "Dam Standard");
    let shipped = library.record(dam, &s);
    library.select(dam, &mut s);
    scribble(&mut s, 0.0);
    library.select(Library::base(Brush::Move), &mut s);
    assert!(library.record(dam, &s) != shipped);
    library.reset(dam, &mut s);
    assert!(library.record(dam, &s) == shipped);
    // One of the user's own goes back to how it was made.
    library.select(dam, &mut s);
    s.current_mut().strength = 0.42;
    let copy = library.duplicate(dam, &mut s);
    s.current_mut().strength = 0.99;
    library.reset(copy, &mut s);
    assert_eq!(s.current().strength, 0.42);
    // And Reset on one brush leaves the rest alone.
    let before = all_records(&library, &s);
    library.select(at(&library, "Rake"), &mut s);
    scribble(&mut s, 0.0);
    library.reset(at(&library, "Rake"), &mut s);
    assert!(all_records(&library, &s) == before);
}

#[test]
fn exactly_one_smooth_brush_is_shifts_and_it_follows_the_marker() {
    let mut library = Library::default();
    let mut s = Settings::default();
    assert!(!library.set_shift(at(&library, "Trim"), &mut s), "only a Smooth-kernel brush can be Shift's");
    assert_eq!(library.shift(), Library::base(Brush::Smooth));
    let peaks = at(&library, "Smooth Peaks");
    assert!(library.set_shift(peaks, &mut s));
    assert_eq!(library.shift(), peaks);
    // Smooth Peaks is not the live Smooth, so strokes are handed its record.
    let handed = s.shift.clone().expect("the Shift brush is handed to strokes");
    assert_eq!(handed.settings.smooth_mode, SmoothMode::Peaks);
    assert_eq!(s.per_brush[Brush::Smooth.index()].smooth_mode, SmoothMode::All, "the base Smooth is still the live one");
    // In hand, it is the working set itself, and an edit is Shift's at once.
    library.select(peaks, &mut s);
    assert!(s.shift.is_none());
    s.current_mut().strength = 0.9;
    // Another Smooth brush in hand: Shift still uses the marked one, edit and all.
    library.select(at(&library, "Relax"), &mut s);
    let handed = s.shift.clone().unwrap();
    assert_eq!((handed.settings.smooth_mode, handed.settings.strength), (SmoothMode::Peaks, 0.9));
    // Deleting the marked brush hands Shift back to the base Smooth.
    library.delete(at(&library, "Smooth Peaks"), &mut s).unwrap();
    assert_eq!(library.shift(), Library::base(Brush::Smooth));
    let handed = s.shift.clone().expect("Relax is live, so the base Smooth is handed over");
    assert_eq!(handed.settings.smooth_mode, SmoothMode::All);
}

#[test]
fn a_shift_stroke_smooths_with_the_marked_brush_whatever_is_in_hand() {
    // A ball with a ridge on it, smoothed with Shift held: once by a
    // sculptor whose marked brush is Relax (which keeps the shape), once by
    // one whose marked brush is the base Smooth.
    let ridge = || {
        let mut mesh = sphere(20_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Standard, 0.12, 0.6);
        draw(&mut mesh, &mut sculptor, &view, &across(&view, -0.5, 0.5, 40), 4, None);
        mesh
    };
    let view = view_of(&ridge());
    let shift_stroke = |marked: &str, in_hand: &str| {
        let mut mesh = ridge();
        let mut sculptor = sculptor_with(Brush::Standard, 0.15, 0.6);
        let (library, s) = (&mut sculptor.library, &mut sculptor.settings);
        library.select(library.named(in_hand).unwrap(), s);
        library.set_shift(library.named(marked).unwrap(), s);
        let points = across(&view, -0.5, 0.5, 40);
        let sample = |i: usize| PointerSample { pos: points[i], pressure: None, time: 50.0 + i as f64 / 240.0 };
        assert!(sculptor.begin(&mut mesh, &view, sample(0), false, true));
        for i in 1..points.len() {
            sculptor.sample(&mut mesh, &view, sample(i));
        }
        sculptor.end(&mut mesh, &view);
        fingerprint(&mesh)
    };
    let with_smooth = shift_stroke("Smooth", "Standard");
    let with_relax = shift_stroke("Relax", "Standard");
    assert_ne!(with_smooth, with_relax, "Shift used the brush marked for it");
    // What is in hand makes no difference to a Shift stroke, another Smooth
    // brush included.
    assert_eq!(shift_stroke("Relax", "Clay"), with_relax);
    assert_eq!(shift_stroke("Relax", "Smooth Peaks"), with_relax);
    assert_eq!(shift_stroke("Relax", "Relax"), with_relax);
    assert_eq!(shift_stroke("Smooth", "Relax"), with_smooth);
}

/// The cursor's inner ring is where the brush is at half weight. With Shift
/// held it is the marked brush's, whatever Smooth brush was last in hand,
/// and the settings the cursor reads are the ones the stroke will use.
#[test]
fn the_ring_with_shift_held_is_the_marked_brushs() {
    let mut library = Library::default();
    let mut s = Settings::default();
    let (peaks, relax) = (at(&library, "Smooth Peaks"), at(&library, "Relax"));
    // Two Smooth brushes told apart by their falloff and strength.
    library.select(peaks, &mut s);
    (s.current_mut().focal_shift, s.current_mut().strength) = (-0.9, 0.8);
    library.select(relax, &mut s);
    (s.current_mut().focal_shift, s.current_mut().strength) = (0.9, 0.2);
    let ring_of = |focal: f32| {
        let alone = Settings { per_brush: [BrushSettings { focal_shift: focal, ..s.per_brush[0] }; Brush::COUNT], ..Settings::default() };
        alone.half_weight_radius(Brush::Smooth)
    };
    let (hard, soft) = (ring_of(-0.9), ring_of(0.9));
    assert!(hard > soft + 0.3, "the two rings are far apart: {hard} and {soft}");

    library.set_shift(peaks, &mut s);
    library.select(Library::base(Brush::Clay), &mut s);
    // Relax is the live Smooth, Smooth Peaks the one Shift uses.
    assert_eq!(s.half_weight_radius(Brush::Smooth), soft, "the working Smooth is Relax");
    assert_eq!(s.half_weight_radius_in_use(Brush::Smooth, true), hard, "with Shift held the ring is Smooth Peaks'");
    assert_eq!(s.in_use(Brush::Smooth, true).0.strength, 0.8);
    // Without Shift (Smooth picked from the panel) it is the brush in hand.
    assert_eq!(s.half_weight_radius_in_use(Brush::Smooth, false), soft);
    assert_eq!(s.in_use(Brush::Smooth, false).0.strength, 0.2);
    // Marked and live at once, there is only the one to show.
    library.set_shift(relax, &mut s);
    assert_eq!(s.half_weight_radius_in_use(Brush::Smooth, true), soft);
    // And another kernel's ring is its own either way.
    assert_eq!(s.half_weight_radius_in_use(Brush::Clay, false), s.half_weight_radius(Brush::Clay));
}

#[test]
fn a_stroke_goes_by_the_name_of_its_brush() {
    let mut library = Library::default();
    let mut s = Settings::default();
    library.select(at(&library, "Dam Standard"), &mut s);
    assert_eq!(library.stroke_name(&s, false), "Dam Standard", "not Crease, its kernel");
    assert_eq!(library.stroke_name(&s, true), "Smooth", "with Shift, the brush Shift uses");
    library.set_shift(at(&library, "Relax"), &mut s);
    assert_eq!(library.stroke_name(&s, true), "Relax");
    s.mask_mode = true;
    assert_eq!((library.stroke_name(&s, false), library.stroke_name(&s, true)), ("Mask", "Mask"));
}

#[test]
fn pins_are_each_brushs_own() {
    let mut library = Library::default();
    let rake = at(&library, "Rake");
    assert!(!library.entries()[rake].pinned);
    library.set_pinned(rake, true);
    library.set_pinned(Library::base(Brush::Pose), false);
    assert!(library.entries()[rake].pinned && !library.entries()[Library::base(Brush::Pose)].pinned);
    let pinned = library.entries().iter().filter(|e| e.pinned).count();
    // Every base brush but Erase Layer starts on the wheel.
    assert_eq!(pinned, Brush::COUNT - 1, "one on, one off");
}

#[test]
fn the_settings_file_always_gets_the_base_brushes() {
    let mut library = Library::default();
    let mut s = Settings::default();
    library.select(Library::base(Brush::Clay), &mut s);
    s.current_mut().strength = 0.37;
    s.clay_depth = 0.9;
    library.select(at(&library, "Clay Strips"), &mut s);
    library.select(at(&library, "Smooth Peaks"), &mut s);
    library.set_shift(at(&library, "Relax"), &mut s);
    let image = library.base_image(&s);
    assert_eq!((image.per_brush[Brush::Clay.index()].strength, image.clay_depth), (0.37, 0.9));
    assert!(image.alpha[Brush::Clay.index()].image.is_none(), "not Clay Strips' alpha");
    assert_eq!(image.per_brush[Brush::Smooth.index()].smooth_mode, SmoothMode::All, "not Smooth Peaks' mode");
    assert!(image.shift.is_none());
    // And what is in hand is not disturbed by the asking.
    assert_eq!(library.entries()[library.selected(&s)].name, "Smooth Peaks");
    assert_eq!(s.per_brush[Brush::Smooth.index()].smooth_mode, SmoothMode::Peaks);
}

#[test]
fn start_up_puts_back_the_brush_in_hand_the_shift_brush_the_pins_and_the_deletions() {
    let mut library = Library::default();
    // The settings file has been laid in the working sets: it left the app
    // on Flatten with the base Clay at a strength of its own.
    let mut s = Settings { brush: Brush::Flatten, ..Settings::default() };
    s.per_brush[Brush::Clay.index()].strength = 0.37;
    let pins = ["clay".to_owned(), "Rake".to_owned(), "Gone Since".to_owned()];
    library.restore(&["Fill".to_owned(), "Clay".to_owned()], Some(&pins), "Relax", "Planar", &mut s);
    assert!(library.named("Fill").is_none(), "the deleted shipped brush stays deleted");
    assert!(library.named("Clay").is_some(), "a base brush cannot be deleted by a file either");
    let pinned: Vec<&str> = library.entries().iter().filter(|e| e.pinned).map(|e| e.name.as_str()).collect();
    assert_eq!(pinned, ["Clay", "Rake"]);
    assert_eq!(library.entries()[library.shift()].name, "Relax");
    assert_eq!(library.entries()[library.selected(&s)].name, "Planar");
    assert!(s.per_brush[Brush::Flatten.index()].lock_plane, "Planar is laid in Flatten's set");
    assert_eq!(library.record(Library::base(Brush::Clay), &s).settings.strength, 0.37);

    // A file from before there was a library names nothing: the kernel it
    // left the app on is that kernel's base brush, and mask painting stays.
    let mut library = Library::default();
    let mut s = Settings { brush: Brush::Crease, mask_mode: true, ..Settings::default() };
    library.restore(&[], None, "", "", &mut s);
    assert_eq!((library.selected(&s), s.mask_mode), (Library::base(Brush::Crease), true));
    assert_eq!(library.entries().iter().filter(|e| e.pinned).count(), Brush::COUNT - 1, "the starting pins");
    assert!(s.shift.is_none());
}

#[test]
fn a_settings_file_read_while_other_brushes_are_in_hand_lands_on_the_base_brushes() {
    let mut library = Library::default();
    let mut s = Settings::default();
    let (strips, peaks) = (at(&library, "Clay Strips"), at(&library, "Smooth Peaks"));
    library.select(peaks, &mut s);
    library.select(strips, &mut s);
    let before = (library.record(strips, &s), library.record(peaks, &s));
    // What the app does with a settings file: base brushes live, the file's
    // settings laid, then the rest.
    library.lay_base(&mut s);
    assert_eq!(s.brush, Brush::Clay, "the kernel in hand is not changed by it");
    s.per_brush[Brush::Clay.index()].strength = 0.37;
    s.per_brush[Brush::Smooth.index()].strength = 0.73;
    library.restore(&[], None, "Smooth", "Clay Strips", &mut s);
    assert_eq!(library.record(Library::base(Brush::Clay), &s).settings.strength, 0.37);
    assert_eq!(library.record(Library::base(Brush::Smooth), &s).settings.strength, 0.73);
    assert!(library.record(strips, &s) == before.0 && library.record(peaks, &s) == before.1, "the brushes that were in hand are untouched");
    assert_eq!(library.selected(&s), strips);
}

#[test]
fn a_record_from_a_file_is_brought_into_range_and_laid_if_its_brush_is_live() {
    let mut library = Library::default();
    let mut s = Settings::default();
    let tubes = at(&library, "Clay Tubes");
    library.select(tubes, &mut s);
    let mut wild = library.record(tubes, &s);
    wild.settings.strength = 40.0;
    wild.settings.spacing = f32::NAN;
    wild.settings.focal_shift = -9.0;
    wild.numbers.clay_depth = 99.0;
    wild.alpha.scale = 0.0;
    library.set_record(tubes, wild, &mut s);
    let now = library.record(tubes, &s);
    assert_eq!((now.settings.strength, now.settings.focal_shift, now.numbers.clay_depth), (1.0, -1.0, 1.5));
    assert_eq!(now.settings.spacing, 0.2, "a number that is not one falls back");
    assert_eq!(now.alpha.scale, *alpha::AlphaSettings::SCALE.start());
    assert_eq!(s.clay_depth, 1.5, "and it is in the working set, the brush being in hand");
}

/// Every brush that ships, picked from the library and drawn as it ships
/// with a pen across a mirror plane: it does something, and what it does is
/// the mirror image of itself, to the closeness of a plain stroke. A preset
/// that set its kernel up to do nothing, or leaned on something symmetry
/// does not carry, would show here.
#[test]
fn every_shipped_brush_draws_and_stays_a_mirror_image_under_symmetry() {
    use super::stroke_tests::{SIZE, twins};
    use glam::{Vec2, Vec3};
    let names: Vec<String> =
        Library::default().entries().iter().filter(|e| e.origin == Origin::Shipped).map(|e| e.name.clone()).collect();
    for name in names {
        // Something for the smoothing and flattening brushes to work on,
        // itself a mirror image.
        let ball = sphere(80_000);
        let rough = ball.positions.iter().map(|&p| p * (1.0 + 0.01 * (p.x.abs() * 60.0).sin() * (p.y * 60.0).sin())).collect();
        let mut mesh = Mesh::new(rough, ball.indices.clone(), None);
        let before = mesh.positions.clone();
        let twins = twins(&mesh);
        let view = view_of(&mesh);
        let mut sculptor = Sculptor::default();
        let (library, s) = (&mut sculptor.library, &mut sculptor.settings);
        assert!(library.select_named(&name, s));
        (s.screen_space_size, s.radius_world, s.symmetry) = (false, 0.09, [true, false, false]);
        // From one side, across the mirror plane and back along it.
        let c = SIZE * 0.5;
        let (side, plane, along) = (c + Vec2::new(-110.0, 60.0), c + Vec2::new(6.0, 10.0), c + Vec2::new(14.0, -90.0));
        let mut path: Vec<Vec2> = (0..=40).map(|i| side.lerp(plane, i as f32 / 40.0)).collect();
        path.extend((1..=40).map(|i| plane.lerp(along, i as f32 / 40.0)));
        draw(&mut mesh, &mut sculptor, &view, &path, 4, Some(&|along| 0.3 + 0.7 * along));
        let flip = Vec3::new(-1.0, 1.0, 1.0);
        let (mut worst, mut biggest) = (0.0f32, 0.0f32);
        for (v, &t) in twins.iter().enumerate() {
            worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * flip));
            biggest = biggest.max(mesh.positions[v].distance(before[v]));
        }
        println!("{name}: moved {biggest:.5}, mirror error {worst:.2e}");
        assert!(biggest > 1e-4, "{name} did nothing");
        assert!(worst / biggest < 0.01, "{name} drifted {:.2}% off symmetry", worst / biggest * 100.0);
    }
}
