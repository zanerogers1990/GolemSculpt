//! Brush alphas: how a picture is sampled and laid on the surface, the
//! pictures that come with the app, reading the user's own from disk, and
//! what a whole stroke prints.

use glam::{Vec2, Vec3};

use super::library::{built_in, copy_in, pictures_in, read_picture};
use super::*;
use crate::camera::{Camera, View};
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::{PointerSample, Sculptor, symmetry};

const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

fn sphere(tris: usize) -> Mesh {
    primitives::build(Primitive::Sphere, tris, 0.5)
}

fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// Drags the pointer along `points` (pixels), a few samples per frame.
fn run_stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, points: &[Vec2]) {
    let sample = |i: usize| PointerSample { pos: points[i], pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(mesh, view, sample(0), false, false), "stroke missed the mesh");
    for i in 1..points.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let _ = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
}

/// A picture for a test to print with, kept for the rest of the run as the
/// library keeps its own.
fn kept(image: AlphaImage) -> &'static AlphaImage {
    Box::leak(Box::new(image))
}

fn the(name: &str) -> &'static AlphaImage {
    kept(built_in(name).expect("an alpha that comes with the app"))
}

/// Dark on its left, bright on its right.
fn left_to_right() -> &'static AlphaImage {
    kept(AlphaImage::drawn("ramp", 64, |x, _| x))
}

/// Dark at its top, bright at its bottom.
fn top_to_bottom() -> &'static AlphaImage {
    kept(AlphaImage::drawn("ramp", 64, |_, y| y))
}

fn settings_with(image: &'static AlphaImage) -> AlphaSettings {
    AlphaSettings { image: Some(image), ..Default::default() }
}

/// A frame lying flat in the XY plane, upright.
const UPRIGHT: Frame = Frame { right: Vec3::X, up: Vec3::Y };

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.02
}

// ---------------------------------------------------------------- sampling

#[test]
fn a_picture_is_blended_between_samples_and_wraps_or_holds_at_its_edge() {
    let image = AlphaImage::new("two", 2, vec![0.0, 1.0, 0.0, 1.0]);
    // Sample centres are a quarter and three quarters of the way across.
    assert_eq!(image.sample(0.25, 0.5, false), 0.0);
    assert_eq!(image.sample(0.75, 0.5, false), 1.0);
    assert_eq!(image.sample(0.5, 0.5, false), 0.5);
    // Past the edge: the edge's own value, or the far side come round again.
    assert_eq!(image.sample(1.2, 0.5, false), 1.0);
    assert_eq!(image.sample(-0.3, 0.5, false), 0.0);
    assert_eq!(image.sample(1.0, 0.5, true), 0.5);
    assert_eq!(image.sample(1.25, 0.5, true), 0.0);
    assert_eq!(image.sample(-0.25, 0.5, true), 1.0);
}

#[test]
fn a_thumbnail_is_the_picture_in_small() {
    let image = AlphaImage::drawn("ramp", 64, |x, _| x);
    let thumb = image.thumbnail(8);
    assert_eq!(thumb.len(), 64);
    assert!(thumb[0] < 30 && thumb[7] > 225, "{} .. {}", thumb[0], thumb[7]);
    assert!(thumb.windows(2).take(7).all(|w| w[0] < w[1]), "brighter left to right");
    // Smaller than the tile: every pixel still has a sample to show.
    assert_eq!(AlphaImage::new("dot", 1, vec![1.0]).thumbnail(4), vec![255; 16]);
}

// ------------------------------------------------------------------- stamp

#[test]
fn a_stamp_lays_the_picture_upright_and_ends_at_its_edge() {
    let across = settings_with(left_to_right()).stamp(1.0).unwrap();
    assert!(near(across.at(&UPRIGHT, Vec3::X * 0.5), 0.75));
    assert!(near(across.at(&UPRIGHT, Vec3::X * -0.5), 0.25));
    // Along the surface only: height above it does not move the print.
    assert_eq!(across.at(&UPRIGHT, Vec3::new(0.5, 0.2, 0.0)), across.at(&UPRIGHT, Vec3::new(0.5, 0.2, 0.7)));
    // The top of the picture is up.
    let down = settings_with(top_to_bottom()).stamp(1.0).unwrap();
    assert!(near(down.at(&UPRIGHT, Vec3::Y * 0.5), 0.25));
    assert!(near(down.at(&UPRIGHT, Vec3::Y * -0.5), 0.75));
    // Past the picture's edge there is nothing, inverted or not.
    assert_eq!(across.at(&UPRIGHT, Vec3::X * 1.2), 0.0);
    assert_eq!(across.at(&UPRIGHT, Vec3::Y * -1.01), 0.0);
    let inverted = AlphaSettings { invert: true, ..settings_with(left_to_right()) }.stamp(1.0).unwrap();
    assert!(near(inverted.at(&UPRIGHT, Vec3::X * 0.5), 0.25));
    assert_eq!(inverted.at(&UPRIGHT, Vec3::X * 1.2), 0.0);
}

#[test]
fn size_and_repeat_set_how_much_of_the_brush_a_picture_covers() {
    // Half size: the same picture over half the width, and nothing beyond.
    let half = AlphaSettings { scale: 0.5, ..settings_with(left_to_right()) }.stamp(1.0).unwrap();
    assert!(near(half.at(&UPRIGHT, Vec3::X * 0.25), 0.75));
    assert_eq!(half.at(&UPRIGHT, Vec3::X * 0.6), 0.0);
    // It follows the brush: twice the radius, twice the print.
    let big = AlphaSettings { scale: 0.5, ..settings_with(left_to_right()) }.stamp(2.0).unwrap();
    assert!(near(big.at(&UPRIGHT, Vec3::X * 0.5), 0.75));
    // Repeated, it comes round again instead of ending.
    let tiled = AlphaSettings { scale: 0.5, tile: true, ..settings_with(left_to_right()) }.stamp(1.0).unwrap();
    assert!(near(tiled.at(&UPRIGHT, Vec3::X * 0.75), 0.25));
    assert!(near(tiled.at(&UPRIGHT, Vec3::X * 0.25), tiled.at(&UPRIGHT, Vec3::X * -0.75)));
    // A setting out of the slider's range is brought into it, not divided by.
    let none = AlphaSettings { scale: 0.0, ..settings_with(left_to_right()) }.stamp(1.0).unwrap();
    assert!(none.at(&UPRIGHT, Vec3::X * 0.05).is_finite());
    assert!(AlphaSettings::default().stamp(1.0).is_none(), "no picture, no stamp");
}

/// A camera looking down -Z at a surface that faces it.
fn facing_camera() -> Aim {
    let mut aim = Aim::default();
    (aim.up, aim.right) = (Vec3::Y, Vec3::X);
    aim
}

#[test]
fn the_picture_faces_up_the_screen_and_rotation_turns_it_anticlockwise() {
    let mut aim = facing_camera();
    let stamp = settings_with(left_to_right()).stamp(1.0).unwrap();
    let frame = aim.frame(Some(&stamp), Vec3::Z, None, 1.0);
    assert!(frame.up.distance(Vec3::Y) < 1e-6 && frame.right.distance(Vec3::X) < 1e-6, "{frame:?}");
    // On a surface tilted back, it lies on the surface, still as upright as it can be.
    let tilted = Vec3::new(0.0, 0.6, 0.8);
    let frame = aim.frame(Some(&stamp), tilted, None, 1.0);
    assert!(frame.up.dot(tilted).abs() < 1e-6 && frame.right.dot(tilted).abs() < 1e-6);
    assert!(frame.up.y > 0.7 && frame.right.distance(Vec3::X) < 1e-6, "{frame:?}");
    // A quarter turn anticlockwise: the picture's bright right side is now at the top.
    let turned = AlphaSettings { rotation: 90.0, ..settings_with(left_to_right()) }.stamp(1.0).unwrap();
    let frame = aim.frame(Some(&turned), Vec3::Z, None, 1.0);
    assert!(near(turned.at(&frame, Vec3::Y * 0.5), 0.75));
    assert!(near(turned.at(&frame, Vec3::Y * -0.5), 0.25));
    assert!(near(turned.at(&frame, Vec3::X * 0.5), 0.5));
    // A dab with no alpha has no frame.
    assert_eq!(aim.frame(None, Vec3::Z, None, 1.0), Frame::NONE);
    // Seen exactly edge on, there is still a frame, flat on the surface.
    let frame = aim.frame(Some(&stamp), Vec3::Y, None, 1.0);
    assert!((frame.up.length() - 1.0).abs() < 1e-5 && frame.up.dot(Vec3::Y).abs() < 1e-6, "{frame:?}");
}

#[test]
fn a_picture_that_turns_with_the_stroke_points_the_way_it_is_going() {
    let mut aim = facing_camera();
    let stamp = AlphaSettings { follow: true, ..settings_with(left_to_right()) }.stamp(0.1).unwrap();
    // Before the stroke has gone anywhere: up the screen.
    assert!(aim.frame(Some(&stamp), Vec3::Z, None, 0.1).up.distance(Vec3::Y) < 1e-6);
    // Going right: the top points right, and the picture's right is down the screen.
    let frame = aim.frame(Some(&stamp), Vec3::Z, Some(Vec3::X * 0.02), 0.1);
    assert!(frame.up.distance(Vec3::X) < 1e-6 && frame.right.distance(-Vec3::Y) < 1e-6, "{frame:?}");
    // A dab made while the pointer rests keeps that heading.
    assert!(aim.frame(Some(&stamp), Vec3::Z, Some(Vec3::ZERO), 0.1).up.distance(Vec3::X) < 1e-6);
    assert!(aim.frame(Some(&stamp), Vec3::Z, Some(Vec3::Y * 1e-6), 0.1).up.distance(Vec3::X) < 1e-6);
    // Then down and to the left.
    let frame = aim.frame(Some(&stamp), Vec3::Z, Some(Vec3::new(-0.02, -0.02, 0.0)), 0.1);
    assert!(frame.up.distance(Vec3::new(-1.0, -1.0, 0.0).normalize()) < 1e-6);
    // A picture that does not turn ignores all of it.
    let fixed = settings_with(left_to_right()).stamp(0.1).unwrap();
    assert!(aim.frame(Some(&fixed), Vec3::Z, Some(Vec3::X * 0.02), 0.1).up.distance(Vec3::Y) < 1e-6);
    assert!(Aim::waits(Some(&stamp)) && !Aim::waits(Some(&fixed)) && !Aim::waits(None));
}

#[test]
fn a_mirror_copy_prints_the_mirror_image_to_the_bit() {
    let mut settings = crate::sculpt::Settings::default();
    settings.symmetry = [true, true, false];
    let ops = symmetry::copies(&settings.symmetry_group());
    assert_eq!(ops.len(), 4);
    let stamp = AlphaSettings { rotation: 33.0, scale: 0.7, ..settings_with(the("Scales")) }.stamp(0.3).unwrap();
    let mut aim = facing_camera();
    let frame = aim.frame(Some(&stamp), Vec3::new(0.2, 0.3, 0.9).normalize(), None, 0.3);
    let mut printed = 0;
    for i in 0..400 {
        let t = i as f32;
        let rel = Vec3::new((t * 0.37).sin(), (t * 0.53).cos(), (t * 0.11).sin()) * 0.2;
        let here = stamp.at(&frame, rel);
        printed += usize::from(here > 0.0);
        for &op in &ops {
            assert_eq!(stamp.at(&frame.mirrored(op), op.apply_dir(rel)).to_bits(), here.to_bits(), "at {rel}");
        }
    }
    assert!(printed > 200, "the test looked at the picture itself: {printed}");
}

// ---------------------------------------------------- the built-in pictures

#[test]
fn the_alphas_that_come_with_the_app_are_whole_pictures_that_repeat() {
    for (name, _) in patterns::ALL {
        let image = the(name);
        let n = image.size;
        assert_eq!(image.name, name);
        let (low, high) = image.samples.iter().fold((f32::MAX, f32::MIN), |(l, h), &s| (l.min(s), h.max(s)));
        assert!(low >= 0.0 && high <= 1.0, "{name}: {low}..{high}");
        assert!(low < 0.1 && high > 0.9, "{name} uses the whole range: {low}..{high}");
        if name == "Square" {
            // One shape in the middle, clear of the edge: it is not for tiling.
            assert_eq!(image.sample(0.5, 0.5, false), 1.0);
            assert_eq!(image.sample(0.02, 0.5, false), 0.0);
            continue;
        }
        // The step across each seam is no bigger than the steps inside.
        let at = |x: usize, y: usize| image.samples[y * n + x];
        let column_step = |a: usize, b: usize| (0..n).map(|y| (at(a, y) - at(b, y)).abs()).sum::<f32>() / n as f32;
        let row_step = |a: usize, b: usize| (0..n).map(|x| (at(x, a) - at(x, b)).abs()).sum::<f32>() / n as f32;
        let inside_columns = (0..n - 1).map(|x| column_step(x, x + 1)).fold(0.0, f32::max);
        let inside_rows = (0..n - 1).map(|y| row_step(y, y + 1)).fold(0.0, f32::max);
        assert!(column_step(n - 1, 0) <= inside_columns + 1e-4, "{name} has a seam down its side");
        assert!(row_step(n - 1, 0) <= inside_rows + 1e-4, "{name} has a seam along its top");
    }
}

#[test]
fn the_alphas_that_come_with_the_app_are_listed_first_and_found_by_name() {
    let listing = listing();
    for (i, (name, _)) in patterns::ALL.iter().enumerate() {
        assert_eq!(listing[i].name, *name);
        assert_eq!(listing[i].thumb.pixels.len(), listing[i].thumb.side * listing[i].thumb.side);
    }
    let rake = find("rake").expect("found whatever the case");
    assert_eq!(rake.name, "Rake");
    assert!(std::ptr::eq(rake, find("Rake").unwrap()), "one picture, however often it is asked for");
    assert!(find("no such alpha").is_none() && find("").is_none());
}

// ------------------------------------------------------- pictures from disk

fn scratch_folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sculpt-alpha-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_picture_from_disk_is_read_as_a_square_of_brightness() {
    let dir = scratch_folder("read");
    // Wider than tall, bright on the right: centred, black above and below.
    let wide = dir.join("wide.png");
    image::GrayImage::from_fn(8, 4, |x, _| image::Luma([(x * 36) as u8])).save(&wide).unwrap();
    let alpha = read_picture(&wide, "wide").unwrap();
    assert_eq!((alpha.name.as_str(), alpha.size), ("wide", 8));
    assert!(alpha.samples[..16].iter().all(|&s| s == 0.0) && alpha.samples[48..].iter().all(|&s| s == 0.0));
    assert_eq!(alpha.samples[16], 0.0);
    assert!((alpha.samples[16 + 7] - 252.0 / 255.0).abs() < 1e-3);

    // Colour counts by its brightness, and what is see-through as black.
    let clear = dir.join("clear.png");
    image::RgbaImage::from_fn(4, 4, |x, _| image::Rgba([255, 255, 255, if x < 2 { 0 } else { 255 }])).save(&clear).unwrap();
    let alpha = read_picture(&clear, "clear").unwrap();
    assert_eq!((alpha.samples[0], alpha.samples[3]), (0.0, 1.0));

    // A big picture is kept at a size a dab can afford to look up.
    let big = dir.join("big.png");
    image::GrayImage::from_pixel(1200, 600, image::Luma([255])).save(&big).unwrap();
    let alpha = read_picture(&big, "big").unwrap();
    assert_eq!(alpha.size, 512);
    assert_eq!((alpha.sample(0.5, 0.5, false), alpha.sample(0.5, 0.1, false)), (1.0, 0.0));

    let notes = dir.join("notes.png");
    std::fs::write(&notes, "not a picture").unwrap();
    assert!(read_picture(&notes, "notes").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn importing_copies_a_picture_under_a_free_name_and_the_folder_is_listed_in_order() {
    let dir = scratch_folder("import");
    let picture = dir.join("pores.png");
    image::GrayImage::from_pixel(4, 4, image::Luma([128])).save(&picture).unwrap();
    let folder = dir.join("alphas");
    let imported = folder.join("Imported");
    assert_eq!(copy_in(&picture, &imported).unwrap(), imported.join("pores.png"));
    assert_eq!(copy_in(&picture, &imported).unwrap(), imported.join("pores 2.png"));
    std::fs::copy(&picture, folder.join("bark.PNG")).unwrap();

    // What is not a picture, or is one in name only, is refused and nothing
    // of it is left behind.
    let notes = dir.join("notes.txt");
    std::fs::write(&notes, "not a picture").unwrap();
    assert!(copy_in(&notes, &imported).is_err());
    let damaged = dir.join("damaged.jpg");
    std::fs::write(&damaged, "not a picture either").unwrap();
    assert!(copy_in(&damaged, &imported).is_err());
    assert!(copy_in(&dir.join("missing.png"), &imported).is_err());
    std::fs::write(folder.join("readme.txt"), "notes are not listed").unwrap();

    let listed = pictures_in(&folder);
    assert_eq!(listed, vec![imported.join("pores 2.png"), imported.join("pores.png"), folder.join("bark.PNG")]);
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------ whole strokes

/// A sculptor holding `brush` at a fixed size on the model, at full
/// strength with a hard edge, so a picture prints whole.
fn sculptor_with(brush: Brush, radius: f32, alpha: AlphaSettings) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    s.brush = brush;
    s.screen_space_size = false;
    s.radius_world = radius;
    s.per_brush[brush.index()].strength = 1.0;
    s.per_brush[brush.index()].focal_shift = -1.0;
    s.alpha[brush.index()] = alpha;
    sculptor
}

/// The pixels of a straight pointer path between two points of the model's
/// space.
fn path(view: &View, from: Vec3, to: Vec3, n: usize) -> Vec<Vec2> {
    (0..=n).map(|i| view.project(from.lerp(to, i as f32 / n as f32)).expect("in front of the camera")).collect()
}

/// The point of the half-unit sphere nearest the camera.
fn front(view: &View) -> Vec3 {
    view.pos.normalize() * 0.5
}

fn height(p: Vec3) -> f32 {
    p.length() - 0.5
}

/// The brushes that print an alpha on a plain ball. Erase Layer takes one
/// too, and does nothing without a sculpt layer to rub out: what it does
/// through a picture is in `erase/tests.rs`.
fn printers() -> impl Iterator<Item = Brush> {
    Brush::ALL.into_iter().filter(|&brush| brush.takes_alpha() && brush != Brush::EraseLayer)
}

#[test]
fn with_no_alpha_or_a_blank_white_one_a_stroke_is_the_same_to_the_bit() {
    let white = kept(AlphaImage::new("white", 4, vec![1.0; 16]));
    for brush in printers() {
        let stroke = |alpha: AlphaSettings| {
            let mut mesh = sphere(60_000);
            let view = view_of(&mesh);
            let mut sculptor = sculptor_with(brush, 0.12, alpha);
            sculptor.settings.per_brush[brush.index()].strength = 0.5;
            // Clay's finishing pass is left out of a stroke with an alpha.
            sculptor.settings.clay_relax = 0.0;
            let (c, right) = (front(&view), view.right);
            run_stroke(&mut mesh, &mut sculptor, &view, &path(&view, c - right * 0.15, c + right * 0.15, 40));
            mesh.positions
        };
        let plain = stroke(AlphaSettings::default());
        let blank = stroke(AlphaSettings { tile: true, ..settings_with(white) });
        assert!(plain.iter().any(|&p| height(p).abs() > 1e-3), "{brush:?} did nothing");
        let differing = plain.iter().zip(&blank).filter(|(a, b)| a.to_array().map(f32::to_bits) != b.to_array().map(f32::to_bits)).count();
        assert_eq!(differing, 0, "{brush:?}");
    }
}

#[test]
fn every_brush_that_takes_an_alpha_prints_it_and_the_others_do_not() {
    for brush in Brush::ALL.into_iter().filter(|&brush| brush != Brush::EraseLayer) {
        let stroke = |alpha: AlphaSettings| {
            let mut mesh = sphere(60_000);
            let view = view_of(&mesh);
            let mut sculptor = sculptor_with(brush, 0.12, alpha);
            sculptor.settings.clay_relax = 0.0;
            let (c, right) = (front(&view), view.right);
            run_stroke(&mut mesh, &mut sculptor, &view, &path(&view, c - right * 0.1, c + right * 0.1, 30));
            mesh.positions
        };
        let plain = stroke(AlphaSettings::default());
        let raked = stroke(AlphaSettings { follow: true, ..settings_with(the("Rake")) });
        let differing = plain.iter().zip(&raked).filter(|(a, b)| a != b).count();
        if brush.takes_alpha() {
            assert!(differing > 100, "{brush:?} ignored its alpha");
            // A picture only ever holds the brush back.
            let moved = |positions: &[Vec3]| positions.iter().map(|&p| height(p).abs() as f64).sum::<f64>();
            assert!(moved(&raked) < moved(&plain), "{brush:?}");
        } else {
            assert_eq!(differing, 0, "{brush:?} has no use for an alpha");
        }
    }
}

/// The surface's height across a stroke made along the screen's right
/// through the front of the sphere: the mean height in each of `BINS` steps
/// up the screen, over the middle half of the brush.
fn profile_across(mesh: &Mesh, view: &View, radius: f32) -> Vec<f32> {
    const BINS: usize = 18;
    let c = front(view);
    let mut bins = vec![(0.0f32, 0u32); BINS];
    for &p in &mesh.positions {
        let rel = p - c;
        let (along, across) = (rel.dot(view.right), rel.dot(view.up));
        if p.dot(c) > 0.0 && along.abs() < 0.012 && across.abs() < 0.5 * radius {
            let bin = (((across / (0.5 * radius) + 1.0) * 0.5 * BINS as f32) as usize).clamp(0, BINS - 1);
            bins[bin].0 += height(p);
            bins[bin].1 += 1;
        }
    }
    assert!(bins.iter().all(|&(_, count)| count > 0), "the mesh is too coarse to measure");
    bins.iter().map(|&(sum, count)| sum / count as f32).collect()
}

#[test]
fn a_rake_that_turns_with_the_stroke_rules_lines_along_it() {
    const RADIUS: f32 = 0.12;
    let stroke = |alpha: AlphaSettings| {
        let mut mesh = sphere(400_000);
        let view = view_of(&mesh);
        let mut sculptor = sculptor_with(Brush::Layer, RADIUS, alpha);
        sculptor.settings.layer_height = 0.3;
        let (c, right) = (front(&view), view.right);
        run_stroke(&mut mesh, &mut sculptor, &view, &path(&view, c - right * 0.2, c + right * 0.2, 80));
        profile_across(&mesh, &view, RADIUS)
    };
    let step = RADIUS * 0.3;
    let range = |profile: &[f32]| {
        let (low, high) = profile.iter().fold((f32::MAX, f32::MIN), |(l, h), &s| (l.min(s), h.max(s)));
        high - low
    };
    // No picture: one even step.
    let plain = stroke(AlphaSettings::default());
    assert!(plain.iter().all(|&h| (h - step).abs() < 0.15 * step), "{plain:?}");
    // The rake's teeth run along the stroke, so across it the surface goes
    // up and down by nearly the whole step.
    let along = stroke(AlphaSettings { follow: true, ..settings_with(the("Rake")) });
    assert!(range(&along) > 0.6 * step, "{along:?}");
    let rises = along.windows(2).filter(|w| w[0] < 0.5 * step && w[1] >= 0.5 * step).count();
    assert!((2..=4).contains(&rises), "three teeth cross the middle half of the brush: {rises} in {along:?}");
    // Turned a quarter, the teeth lie across the stroke and every point is
    // passed over by one: the lines smear into the plain step.
    let athwart = stroke(AlphaSettings { follow: true, rotation: 90.0, ..settings_with(the("Rake")) });
    assert!(range(&athwart) < 0.25 * range(&along), "{athwart:?}");
    // Not turning with the stroke, a stroke along the screen's right has the
    // teeth across it just the same.
    let fixed = stroke(settings_with(the("Rake")));
    assert!(range(&fixed) < 0.25 * range(&along), "{fixed:?}");
}

#[test]
fn a_stroke_with_a_picture_that_turns_opens_once_it_has_a_heading() {
    let mut mesh = sphere(20_000);
    let view = view_of(&mesh);
    let at = |x: f32| PointerSample { pos: SIZE * 0.5 + Vec2::new(x, 0.0), pressure: None, time: x as f64 / 2400.0 };
    let dabs = |sculptor: &Sculptor| sculptor.dabs_applied;

    // A picture that faces up the screen prints the moment the pen lands.
    let mut fixed = sculptor_with(Brush::Standard, 0.1, settings_with(the("Rake")));
    assert!(fixed.begin(&mut mesh, &view, at(0.0), false, false));
    assert_eq!(dabs(&fixed), 1);
    let _ = fixed.end(&mut mesh, &view);

    // One that turns with the stroke waits for the stroke to set off...
    let turning = AlphaSettings { follow: true, ..settings_with(the("Rake")) };
    let mut sculptor = sculptor_with(Brush::Standard, 0.1, turning);
    assert!(sculptor.begin(&mut mesh, &view, at(0.0), false, false));
    assert_eq!(dabs(&sculptor), 0);
    sculptor.sample(&mut mesh, &view, at(30.0));
    assert!(dabs(&sculptor) >= 1);
    let _ = sculptor.end(&mut mesh, &view);

    // ...and a press that goes nowhere still prints, once, when it ends.
    let mut sculptor = sculptor_with(Brush::Standard, 0.1, turning);
    assert!(sculptor.begin(&mut mesh, &view, at(0.0), false, false));
    assert!(sculptor.end(&mut mesh, &view).is_some());
    assert_eq!(dabs(&sculptor), 1);
}

/// Index of each vertex's mirror image across X.
fn twins(mesh: &Mesh) -> Vec<u32> {
    let key = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
    let map: std::collections::HashMap<[u32; 3], u32> =
        mesh.positions.iter().enumerate().map(|(i, &p)| (key(p), i as u32)).collect();
    mesh.positions
        .iter()
        .map(|&p| *map.get(&key(p * Vec3::new(-1.0, 1.0, 1.0))).expect("mesh is not mirror-exact"))
        .collect()
}

#[test]
fn mirrored_strokes_with_an_alpha_stay_mirror_images() {
    for brush in printers() {
        let mut mesh = sphere(80_000);
        let twins = twins(&mesh);
        let view = view_of(&mesh);
        // A picture with no symmetry of its own, turned, and turning.
        let alpha = AlphaSettings { follow: true, rotation: 20.0, tile: true, scale: 0.6, ..settings_with(the("Scales")) };
        let mut sculptor = sculptor_with(brush, 0.1, alpha);
        sculptor.settings.symmetry = [true, false, false];
        sculptor.settings.per_brush[brush.index()].strength = 0.6;
        // Starts well to one side, crosses the mirror plane at an angle and
        // runs along it, as in the test every brush shares.
        let c = SIZE * 0.5;
        let mut points: Vec<Vec2> = (0..=40).map(|i| (c + Vec2::new(-110.0, 60.0)).lerp(c + Vec2::new(6.0, 10.0), i as f32 / 40.0)).collect();
        points.extend((0..=40).map(|i| (c + Vec2::new(6.0, 10.0)).lerp(c + Vec2::new(14.0, -90.0), i as f32 / 40.0)));
        run_stroke(&mut mesh, &mut sculptor, &view, &points);

        let flip = Vec3::new(-1.0, 1.0, 1.0);
        let (mut worst, mut biggest) = (0.0f32, 0.0f32);
        for (v, &t) in twins.iter().enumerate() {
            worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * flip));
            biggest = biggest.max(height(mesh.positions[v]).abs());
        }
        assert!(biggest > 1e-4, "{brush:?} did nothing");
        println!("{brush:?}: displacement {biggest:.5}, mirror error {worst:.2e}");
        assert!(worst / biggest < 0.01, "{brush:?} drifted {:.2}% off symmetry", worst / biggest * 100.0);
    }
}

// ----------------------------------------------------------- drag to place

/// The mean height of the surface near the point `across` and `up` from the
/// front of the sphere, as the camera sees it.
fn height_near(mesh: &Mesh, view: &View, across: f32, up: f32) -> f32 {
    let c = front(view);
    let (mut sum, mut count) = (0.0, 0);
    for &p in &mesh.positions {
        let rel = p - c;
        if p.dot(c) > 0.0 && (rel.dot(view.right) - across).abs() < 0.008 && (rel.dot(view.up) - up).abs() < 0.008 {
            sum += height(p);
            count += 1;
        }
    }
    assert!(count > 0, "the mesh is too coarse to measure");
    sum / count as f32
}

/// A pointer sample over the point `across` and `up` from the front of the
/// sphere.
fn over(view: &View, across: f32, up: f32, step: usize) -> PointerSample {
    let at = front(view) + view.right * across + view.up * up;
    PointerSample { pos: view.project(at).unwrap(), pressure: None, time: step as f64 / 60.0 }
}

fn same_bits(a: &[Vec3], b: &[Vec3]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.to_array().map(f32::to_bits) == b.to_array().map(f32::to_bits))
}

#[test]
fn drag_to_place_prints_once_at_the_size_dragged_and_takes_back_what_it_outgrew() {
    let mut mesh = sphere(200_000);
    let before = mesh.positions.clone();
    let view = view_of(&mesh);
    let alpha = AlphaSettings { drag: true, ..settings_with(the("Square")) };
    let mut sculptor = sculptor_with(Brush::Standard, 0.05, alpha);
    sculptor.settings.per_brush[Brush::Standard.index()].strength = 0.4;

    // The press alone prints nothing: there is no size yet.
    assert!(sculptor.begin(&mut mesh, &view, over(&view, 0.0, 0.0, 0), false, false));
    sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
    assert!(same_bits(&mesh.positions, &before));

    // Dragged out to 0.2: the square stands a fifth of that tall (Strength
    // 40%, of half the radius),
    // flat on top, and the surface round it inside the ring is untouched.
    sculptor.sample(&mut mesh, &view, over(&view, 0.2, 0.0, 1));
    sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
    let top = 0.2 * 0.2;
    assert!((height_near(&mesh, &view, 0.0, 0.0) - top).abs() < 0.1 * top);
    assert!((height_near(&mesh, &view, 0.1, 0.0) - top).abs() < 0.1 * top);
    // (A ball's own vertices are not at its radius to the last bit.)
    assert!(height_near(&mesh, &view, 0.165, 0.0).abs() < 1e-6);
    assert!(height_near(&mesh, &view, 0.0, -0.165).abs() < 1e-6);

    // Dragged back in to 0.1: a smaller, lower square, and what the bigger
    // one had raised beyond it is back where the stroke found it, exactly.
    sculptor.sample(&mut mesh, &view, over(&view, 0.1, 0.0, 2));
    sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
    let top = 0.2 * 0.1;
    assert!((height_near(&mesh, &view, 0.0, 0.0) - top).abs() < 0.1 * top);
    assert!(height_near(&mesh, &view, 0.1, 0.0).abs() < 1e-6);
    let beyond: Vec<usize> = (0..before.len()).filter(|&v| before[v].distance(front(&view)) > 0.1).collect();
    assert!(beyond.iter().all(|&v| mesh.positions[v].to_array().map(f32::to_bits) == before[v].to_array().map(f32::to_bits)));
    let raised = mesh.positions.iter().zip(&before).filter(|(a, b)| a != b).count();

    // Lifting leaves that print, as one step to undo.
    let delta = sculptor.end(&mut mesh, &view);
    assert!(delta.is_some());
    assert_eq!(mesh.positions.iter().zip(&before).filter(|(a, b)| a != b).count(), raised);
    assert!(raised > 500, "{raised}");
}

#[test]
fn a_tap_with_drag_to_place_leaves_the_surface_alone() {
    let mut mesh = sphere(20_000);
    let before = mesh.positions.clone();
    let view = view_of(&mesh);
    let alpha = AlphaSettings { drag: true, ..settings_with(the("Square")) };
    let mut sculptor = sculptor_with(Brush::Layer, 0.1, alpha);
    assert!(sculptor.begin(&mut mesh, &view, over(&view, 0.0, 0.0, 0), false, false));
    // The pen wobbles a pixel as it lifts.
    let mut wobble = over(&view, 0.0, 0.0, 1);
    wobble.pos += Vec2::new(1.0, 1.0);
    sculptor.sample(&mut mesh, &view, wobble);
    sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
    assert!(sculptor.end(&mut mesh, &view).is_none(), "nothing to undo");
    assert!(same_bits(&mesh.positions, &before));
}

#[test]
fn a_dragged_print_stands_upright_or_points_the_way_it_was_dragged() {
    // Bright at its bottom: the low side of the print is the picture's top.
    let print = |follow: bool| {
        let mut mesh = sphere(200_000);
        let view = view_of(&mesh);
        let alpha = AlphaSettings { drag: true, follow, ..settings_with(top_to_bottom()) };
        let mut sculptor = sculptor_with(Brush::Inflate, 0.05, alpha);
        assert!(sculptor.begin(&mut mesh, &view, over(&view, 0.0, 0.0, 0), false, false));
        sculptor.sample(&mut mesh, &view, over(&view, 0.2, 0.0, 1));
        let _ = sculptor.end(&mut mesh, &view);
        let at = |across: f32, up: f32| height_near(&mesh, &view, across, up);
        [at(0.1, 0.0), at(-0.1, 0.0), at(0.0, 0.1), at(0.0, -0.1)]
    };
    // Upright whichever way it was dragged: higher below the middle than above.
    let [right, left, above, below] = print(false);
    assert!(below > 2.0 * above && (right - left).abs() < 0.2 * below, "{:?}", print(false));
    // Turning with the stroke, its top points along the drag, to the right.
    let [right, left, above, below] = print(true);
    assert!(left > 2.0 * right && (above - below).abs() < 0.2 * left, "{:?}", print(true));
}

#[test]
fn a_dragged_print_and_its_mirror_image_stay_mirror_images_where_they_meet() {
    let mut mesh = sphere(80_000);
    let twins = twins(&mesh);
    let view = view_of(&mesh);
    let alpha = AlphaSettings { drag: true, follow: true, rotation: 20.0, ..settings_with(the("Scales")) };
    let mut sculptor = sculptor_with(Brush::Clay, 0.05, alpha);
    sculptor.settings.symmetry = [true, false, false];
    sculptor.settings.per_brush[Brush::Clay.index()].strength = 0.3;
    // Lands beside the mirror plane and is dragged out well across it.
    let c = SIZE * 0.5;
    let at = |offset: Vec2, step: usize| PointerSample { pos: c + offset, pressure: None, time: step as f64 / 60.0 };
    assert!(sculptor.begin(&mut mesh, &view, at(Vec2::new(-30.0, 10.0), 0), false, false));
    for step in 1..=6 {
        sculptor.sample(&mut mesh, &view, at(Vec2::new(-30.0 + 15.0 * step as f32, 10.0 - 8.0 * step as f32), step));
        sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
    }
    assert!(sculptor.end(&mut mesh, &view).is_some());

    let flip = Vec3::new(-1.0, 1.0, 1.0);
    let (mut worst, mut biggest) = (0.0f32, 0.0f32);
    for (v, &t) in twins.iter().enumerate() {
        worst = worst.max(mesh.positions[t as usize].distance(mesh.positions[v] * flip));
        biggest = biggest.max(height(mesh.positions[v]).abs());
    }
    println!("displacement {biggest:.5}, mirror error {worst:.2e}");
    assert!(biggest > 1e-3 && worst / biggest < 0.01, "drifted {:.2}% off symmetry", worst / biggest * 100.0);
}

// ---------------------------------------------------------------- settings

#[test]
fn a_brush_keeps_its_alpha_between_runs() {
    use crate::prefs::BrushPrefs;
    let mut settings = crate::sculpt::Settings::default();
    settings.alpha[Brush::Clay.index()] = AlphaSettings {
        image: find("Rake"),
        rotation: 45.0,
        scale: 0.5,
        invert: true,
        tile: true,
        follow: true,
        drag: true,
        random_rotation: 120.0,
    };
    // Set on a brush that has no picture: nothing of it is worth keeping.
    settings.alpha[Brush::Layer.index()].rotation = 90.0;
    let kept = BrushPrefs::of(&settings);
    assert_eq!(kept.alpha.keys().collect::<Vec<_>>(), ["Clay"]);
    let text = serde_json::to_string(&kept).unwrap();
    let read: BrushPrefs = serde_json::from_str(&text).unwrap();

    let mut back = crate::sculpt::Settings::default();
    back.alpha[Brush::Inflate.index()].image = find("Noise");
    read.apply(&mut back);
    let clay = back.alpha[Brush::Clay.index()];
    assert_eq!(clay.image.map(|i| i.name.as_str()), Some("Rake"));
    assert_eq!((clay.rotation, clay.scale, clay.invert, clay.tile, clay.follow, clay.drag), (45.0, 0.5, true, true, true, true));
    assert_eq!(clay.random_rotation, 120.0);
    assert!(back.alpha[Brush::Inflate.index()].image.is_none(), "a brush with no alpha kept goes back to none");

    // A picture that is gone, and numbers that are not ones, leave a plain brush in range.
    let mut odd = kept.clone();
    let entry = odd.alpha.get_mut("Clay").unwrap();
    (entry.name, entry.scale, entry.rotation) = ("Deleted Since".into(), f32::NAN, 9000.0);
    entry.random_rotation = 9000.0;
    odd.apply(&mut back);
    let clay = back.alpha[Brush::Clay.index()];
    assert!(clay.image.is_none());
    assert_eq!((clay.scale, clay.rotation, clay.random_rotation), (1.0, 360.0, 180.0));
}
