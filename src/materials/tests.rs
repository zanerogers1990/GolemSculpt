//! The material library: built-in materials pack as the Look does, files
//! read forgivingly and follow the library, custom shaders are checked
//! without the graphics card, and the palette keeps its entries.

use std::path::PathBuf;

use super::custom::{self, Shader, Shaders, Sidecar};
use super::files::{self, Shelf};
use super::library::{Library, Origin};
use super::palette::{self, ensure_in_palette, material_of};
use super::rows;
use super::*;
use crate::look::{self, Finish, Frame, Look};
use crate::mesh::paint::palette::ScenePaint;

fn folder(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("sculpt_material_tests").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn frame(size: f32) -> Frame {
    Frame {
        cam_right: glam::Vec3::X,
        cam_up: glam::Vec3::Y,
        cam_forward: glam::Vec3::NEG_Z,
        ortho_half_height: None,
        eye_offset: 0.0,
        scene_min: glam::Vec3::splat(-1.0),
        scene_max: glam::Vec3::splat(1.0),
        model_size: size,
        time: 1.0,
        gradient_background: false,
        smoothness: 0.4,
        shadow_size: 2048,
    }
}

fn library_material(library: &Library, name: &str) -> Material {
    library.entries()[library.named(name).unwrap_or_else(|| panic!("no {name}"))].material.clone()
}

#[test]
fn every_preset_is_a_built_in_material() {
    let library = Library::default();
    let count = 1 + look::LURES.len() + look::METALS.len() + look::CLAYS.len() + look::WOODS.len() + look::GLOWS.len();
    assert_eq!(count, 33);
    assert_eq!(library.entries().len(), count);
    assert!(library.entries().iter().all(|e| e.origin == Origin::BuiltIn));
    for name in ["Plain", "Metal - Rust", "Lure - Blue Flake", "Clay - Terracotta", "Wood - Golden Oak", "Glow - Lava Cracks"] {
        assert!(library.named(name).is_some(), "{name}");
    }
    // The plain material is the model's own colour as a new scene has it.
    let o = crate::doc::Options::default();
    assert_eq!(Plain::default(), Plain { base_color: o.base_color, metallic: 0.0, smoothness: o.smoothness });
}

#[test]
fn metal_rust_packs_byte_for_byte_as_the_look_does() {
    let library = Library::default();
    let shaders = Shaders::default();
    let size = 1.7;
    let rust = look::METALS.iter().position(|p| p.id == "rust").unwrap();
    let mut look = Look::default();
    look.pick_preset(Finish::Metal, rust, size);
    let packed = library_material(&library, "Metal - Rust").pack(size, &shaders);
    assert_eq!(packed.kind, Kind::Metal);
    assert_eq!(bytemuck::bytes_of(&packed.params), bytemuck::bytes_of(&look.finish_params(Finish::Metal, &frame(size))));
}

#[test]
fn every_built_in_material_packs_as_its_look_preset() {
    let library = Library::default();
    let shaders = Shaders::default();
    let size = 2.3;
    let check = |name: String, finish: Finish, preset: usize| {
        let mut look = Look::default();
        look.pick_preset(finish, preset, size);
        let want = look.finish_params(finish, &frame(size));
        let got = library_material(&library, &name).pack(size, &shaders).params;
        assert_eq!(bytemuck::bytes_of(&got), bytemuck::bytes_of(&want), "{name}");
    };
    look::LURES.iter().enumerate().for_each(|(i, p)| check(format!("Lure - {}", p.name), Finish::LurePlastic, i));
    look::METALS.iter().enumerate().for_each(|(i, p)| check(format!("Metal - {}", p.name), Finish::Metal, i));
    look::CLAYS.iter().enumerate().for_each(|(i, p)| check(format!("Clay - {}", p.name), Finish::Clay, i));
    look::WOODS.iter().enumerate().for_each(|(i, p)| check(format!("Wood - {}", p.name), Finish::Wood, i));
    for (i, p) in look::GLOWS.iter().enumerate() {
        let mut look = Look::default();
        look.pick_glow(i, size);
        let got = library_material(&library, &format!("Glow - {}", p.name)).pack(size, &shaders);
        assert_eq!(got.kind, Kind::Glow);
        assert_eq!(got.params, look.pack_glow(size), "{}", p.name);
    }
    // Plain: the Look's plain finish with the model's colour and gloss.
    let plain = library_material(&library, "Plain").pack(size, &shaders);
    assert_eq!(plain.params, Look::default().pack_pbr(0.4));
    assert_eq!(plain.base, look::linear([0.65, 0.65, 0.68]));
}

#[test]
fn a_material_reads_forgivingly_and_only_a_known_finish() {
    let rust = library_material(&Library::default(), "Metal - Rust");
    let text = files::text(&rust);
    assert_eq!(files::parse(&text, "x").unwrap(), rust, "a round trip changes nothing");

    // Missing fields keep the finish's starting values; unknown ones are
    // ignored; a wrong kind is passed over; numbers come into range.
    let text = r#"{ "name": "Mine", "future": 3, "tint": { "mode": "Multiply", "amount": 7 },
        "settings": { "finish": "clay", "gloss": "shiny", "grain": 99.0, "preset": "red_oil" } }"#;
    let m = files::parse(text, "fallback").unwrap();
    assert_eq!(m.name, "Mine");
    assert_eq!(m.tint, Tint { mode: TintMode::Multiply, amount: 1.0 });
    let Settings::Clay(c) = &m.settings else { panic!("{m:?}") };
    assert_eq!(c.gloss, Look::default().clay.gloss);
    assert_eq!(c.grain, 3.0);
    assert_eq!(c.preset, look::CLAYS.iter().position(|p| p.id == "red_oil").unwrap());

    let nameless = files::parse(r#"{ "settings": { "finish": "Plain" } }"#, "From File").unwrap();
    assert_eq!(nameless.name, "From File");
    for bad in ["not json", r#"{ "name": "x" }"#, r#"{ "settings": { "finish": "Velvet" } }"#] {
        assert!(files::parse(bad, "x").is_err(), "{bad}");
    }
}

#[test]
fn the_folder_follows_the_library() {
    let home = folder("shelf");
    let dir = home.join(files::FOLDER);
    let names = || -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(&dir)
            .map(|e| e.flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        out.sort();
        out
    };
    let mut shelf = Shelf::new(Some(&home), true);
    let mut library = Library::default();
    assert!(shelf.load(&mut library).is_empty());
    shelf.sync(&library);
    assert!(names().is_empty(), "built-in materials are never written");

    let copy = library.duplicate(library.named("Metal - Rust").unwrap()).unwrap();
    assert_eq!(library.entries()[copy].material.name, "Metal - Rust Copy");
    shelf.sync(&library);
    assert_eq!(names(), ["Metal - Rust Copy.json"]);

    // A change is written; a rename moves the file.
    let mut changed = library.entries()[copy].material.clone();
    changed.tint = Tint { mode: TintMode::Multiply, amount: 0.5 };
    library.set(copy, changed).unwrap();
    library.rename(copy, "Old Iron").unwrap();
    shelf.sync(&library);
    assert_eq!(names(), ["Old Iron.json"]);
    let read = files::read(&dir.join("Old Iron.json")).unwrap();
    assert_eq!(read.tint.amount, 0.5);
    assert!(library.rename(library.named("Plain").unwrap(), "Mine").is_err(), "built-ins keep their names");
    assert!(library.name_refused("old iron", None).is_some(), "names are told apart without case");

    // A file that is not a material is left alone and said so.
    std::fs::write(dir.join("shopping.json"), "eggs").unwrap();
    let mut again = Library::default();
    let mut fresh = Shelf::new(Some(&home), true);
    let notes = fresh.load(&mut again);
    assert!(notes.iter().any(|n| n.contains("shopping.json")), "{notes:?}");
    let old_iron = again.named("Old Iron").unwrap();
    assert_eq!(again.entries()[old_iron].origin, Origin::User);

    // Delete sets the file aside, where Import brings it back.
    let aside = fresh.set_aside(&again, old_iron).unwrap();
    again.delete(old_iron).unwrap();
    fresh.sync(&again);
    assert!(aside.ends_with(PathBuf::from("deleted").join("Old Iron.json")));
    assert_eq!(names(), ["shopping.json"]);
    let (back, said) = files::import(&aside, &mut again).unwrap();
    assert!(said.contains("Old Iron"));
    assert_eq!(again.entries()[back].material.tint.amount, 0.5);
    assert!(again.delete(again.named("Plain").unwrap()).is_err(), "built-ins stay");

    // Export writes what import reads.
    let out = home.join("exported.json");
    files::export(&out, &again, back).unwrap();
    assert_eq!(files::read(&out).unwrap(), again.entries()[back].material);
}

const SIDECAR: &str = r#"{ "name": "Test Paint", "needs": ["curvature"], "params": [
    { "name": "Tone", "kind": "colour", "default": [1.0, 0.5, 0.0], "at": [0, 0] },
    { "name": "Size", "kind": "number", "min": 0.0, "max": 0.5, "default": 0.1, "at": [1, 2], "scale": "size" },
    { "name": "Rough", "kind": "number", "min": 0.0, "max": 1.0, "default": 0.25, "at": [1, 3] },
    { "name": "On", "kind": "toggle", "default": true, "at": [2, 0] },
    { "name": "Lost", "kind": "number", "default": 1.0, "at": [16, 0] },
    { "name": "Spill", "kind": "colour", "default": [1, 1, 1], "at": [3, 2] } ] }"#;

#[test]
fn sidecar_sliders_pack_into_their_lanes() {
    let sidecar = Sidecar::parse(SIDECAR).unwrap();
    assert!(sidecar.needs("curvature") && !sidecar.needs("back_depth"));
    let p = custom::pack(&sidecar, &sidecar.defaults(), 2.0);
    assert_eq!(p.v[0], [1.0, look::linear([0.5; 3])[0], 0.0, 0.0], "a colour in linear across three lanes");
    assert_eq!(p.v[1], [0.0, 0.0, 0.2, 0.25], "a size is a fraction of the model");
    assert_eq!(p.v[2], [1.0, 0.0, 0.0, 0.0], "a toggle is 1 or 0");
    assert_eq!(p.v[3], [0.0; 4], "a colour that would run off its vec4 is left out");
    assert!(p.v[4..].iter().all(|v| *v == [0.0; 4]));

    let mut values = sidecar.defaults();
    values.insert("Size".into(), Value::Number(9.0));
    values.insert("Rough".into(), Value::Toggle(true));
    values.insert("On".into(), Value::Toggle(false));
    let p = custom::pack(&sidecar, &values, 4.0);
    assert_eq!(p.v[1], [0.0, 0.0, 2.0, 0.25], "out of range is brought in; the wrong kind is the default");
    assert_eq!(p.v[2][0], 0.0);
}

const GOOD: &str = "fn finish_test_paint(s: Surface, params: FinishParams) -> MaterialSample {\n    \
                    return material(s, params.v[0].rgb);\n}\n";

#[test]
fn a_shader_is_checked_without_the_graphics_card() {
    assert!(custom::check(GOOD, "finish_test_paint").unwrap().warnings.is_empty());

    // A mistake is named by the line of the user's own file.
    let broken = "fn finish_test_paint(s: Surface, params: FinishParams) -> MaterialSample {\n    \
                  let x = 1.0 +;\n    return material(s, vec3<f32>(x));\n}\n";
    let problem = custom::check(broken, "finish_test_paint").unwrap_err();
    assert_eq!(problem.line, Some(2), "{problem:?}");
    assert!(problem.words().starts_with("line 2: "), "{}", problem.words());

    // Validation errors too (a type mismatch), and wrong signatures.
    let mistyped = "fn finish_test_paint(s: Surface, params: FinishParams) -> MaterialSample {\n    \
                    let x: f32 = s.p;\n    return material(s, vec3<f32>(x));\n}\n";
    assert!(custom::check(mistyped, "finish_test_paint").unwrap_err().line.is_some());
    let wrong = "fn finish_test_paint(s: Surface) -> MaterialSample {\n    return material(s, vec3<f32>(1.0));\n}\n";
    assert!(custom::check(wrong, "finish_test_paint").unwrap_err().message.contains("must be fn"));
    assert!(custom::check(GOOD, "finish_other").unwrap_err().message.contains("no function called"));

    // Screen-space derivatives are a warning, also in a helper it calls.
    let derivative = "fn test_paint_edge(x: f32) -> f32 {\n    return fwidth(x);\n}\n\
                      fn finish_test_paint(s: Surface, params: FinishParams) -> MaterialSample {\n    \
                      return material(s, vec3<f32>(test_paint_edge(s.p.x)));\n}\n";
    let warned = custom::check(derivative, "finish_test_paint").unwrap();
    assert!(warned.warnings.iter().any(|w| w.contains("test_paint_edge uses fwidth")), "{warned:?}");

    // The example that ships checks clean.
    let example = custom::check(custom::EXAMPLE_WGSL, "finish_hammered_copper").unwrap();
    assert!(example.warnings.is_empty(), "{example:?}");
    assert!(Sidecar::parse(custom::EXAMPLE_SIDECAR).unwrap().params.len() >= 4);
}

#[test]
fn a_custom_material_that_does_not_compile_shows_as_its_swatch() {
    let home = folder("fallback");
    let dir = home.join(files::FOLDER).join(custom::FOLDER);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("good_one.wgsl"), GOOD.replace("test_paint", "good_one")).unwrap();
    std::fs::write(dir.join("good_one.json"), SIDECAR).unwrap();
    std::fs::write(dir.join("bad_one.wgsl"), "fn finish_bad_one(s: Surface) {\n  let = 3;\n}\n").unwrap();
    std::fs::write(dir.join("bad_one.json"), r#"{ "name": "Bad One", "swatch": [0.2, 0.9, 0.1] }"#).unwrap();
    let mut shaders = Shaders::new(Some(&home.join(files::FOLDER)));
    assert!(shaders.scan().is_empty());
    assert!(shaders.usable("good_one").is_some());
    let failure = shaders.get("bad_one").unwrap().failure().unwrap();
    assert!(failure.contains("Bad One") && failure.contains("line 2"), "{failure}");

    let mut library = Library::default();
    library.set_shaders(&shaders);
    let mut paint = ScenePaint::default();
    let good = ensure_in_palette(&mut paint, &library_material(&library, "Test Paint"), 1.0, &shaders).unwrap();
    let bad = ensure_in_palette(&mut paint, &library_material(&library, "Bad One"), 1.0, &shaders).unwrap();
    let upload = rows::upload(&paint, 1.0, &shaders);
    let row = |slot: u8| upload.rows[upload.row_of(slot) as usize];
    assert_eq!(row(good).head[0], Kind::Custom as u32 as f32);
    assert_eq!(row(bad).head[0], Kind::Swatch as u32 as f32, "drawn as its swatch");
    assert_eq!(row(bad).base[..3], look::linear([0.2, 0.9, 0.1]));
    assert_eq!(upload.customs.len(), 1, "only the shader that compiles is built in");
    assert!(upload.needs[upload.row_of(good) as usize].0, "its sidecar asks for curvature");
    assert!(upload.failures.iter().any(|f| f.contains("Bad One")));
    // The painted shader made for this palette compiles: the card never
    // sees the broken source.
    let source = crate::render::paint::source(&upload.kinds, &upload.customs);
    assert!(!source.contains("finish_bad_one"));
    crate::render::paint::validate(&source).unwrap();
}

#[test]
fn a_changed_shader_file_is_read_again() {
    let home = folder("reload");
    let dir = home.join(files::FOLDER).join(custom::FOLDER);
    std::fs::create_dir_all(&dir).unwrap();
    let wgsl = dir.join("live.wgsl");
    std::fs::write(&wgsl, GOOD.replace("test_paint", "live")).unwrap();
    std::fs::write(dir.join("live.json"), r#"{ "name": "Live" }"#).unwrap();
    let mut shaders = Shaders::new(Some(&home.join(files::FOLDER)));
    shaders.scan();
    assert!(shaders.reread_changed().is_empty(), "nothing changed");
    let hash = shaders.get("live").unwrap().hash();

    // Broken, with a later time on the file.
    std::fs::write(&wgsl, "fn finish_live(s: Surface, params: FinishParams) -> MaterialSample {\n  nope\n}\n").unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options().write(true).open(&wgsl).unwrap().set_modified(later).unwrap();
    assert_eq!(shaders.reread_changed(), ["live"]);
    assert!(!shaders.get("live").unwrap().ok());
    assert_ne!(shaders.get("live").unwrap().hash(), hash);

    // A new file in the folder is found; a deleted one goes.
    std::fs::write(dir.join("second.wgsl"), GOOD.replace("test_paint", "second")).unwrap();
    assert_eq!(shaders.reread_changed(), ["second"]);
    std::fs::remove_file(dir.join("second.wgsl")).unwrap();
    assert_eq!(shaders.reread_changed(), ["second"]);
    assert!(shaders.get("second").is_none());
    // Polling looks at most once a second.
    let now = std::time::Instant::now();
    shaders.poll(now);
    std::fs::write(&wgsl, GOOD.replace("test_paint", "live")).unwrap();
    std::fs::File::options().write(true).open(&wgsl).unwrap().set_modified(later + std::time::Duration::from_secs(5)).unwrap();
    assert!(shaders.poll(now).is_empty());
    assert_eq!(shaders.poll(now + std::time::Duration::from_secs(2)), ["live"]);
    assert!(shaders.get("live").unwrap().ok());
}

#[test]
fn the_palette_keeps_its_entries_through_a_file() {
    let library = Library::default();
    let shaders = Shaders::default();
    let mut paint = ScenePaint::default();
    let rust = library_material(&library, "Metal - Rust");
    let clay = library_material(&library, "Clay - Terracotta");
    let a = ensure_in_palette(&mut paint, &rust, 1.5, &shaders).unwrap();
    let b = ensure_in_palette(&mut paint, &clay, 1.5, &shaders).unwrap();
    assert_eq!((a, b), (1, 2));
    assert_eq!(ensure_in_palette(&mut paint, &rust, 9.0, &shaders).unwrap(), a, "a material joins once");
    assert_eq!(paint.palette[0].reference, 1.5, "its pattern frozen at the size it joined at");

    let mut details = crate::io::Details::default();
    paint.keep(&mut details);
    let back = ScenePaint::read(&details).unwrap();
    assert_eq!(back, paint);
    assert_eq!(material_of(&back.palette[0]), rust);
    // Packed the same after the round trip.
    assert_eq!(rows::upload(&back, 1.0, &shaders), rows::upload(&paint, 1.0, &shaders));

    // A material this version cannot read is drawn as its swatch's plain colour.
    let mut damaged = back.palette[1].clone();
    damaged.material = serde_json::json!({ "settings": { "finish": "Velvet" } });
    damaged.swatch = [0.1, 0.2, 0.3];
    let m = material_of(&damaged);
    assert_eq!(m.kind(), Kind::Plain);
    assert_eq!(m.swatch, [0.1, 0.2, 0.3]);
    // An entry of a later version's with fields this one does not know keeps them.
    let mut later = serde_json::to_value(&back).unwrap();
    later["palette"][0]["sheen_map"] = serde_json::json!("future");
    let mut details = crate::io::Details::default();
    details.set(crate::io::slot::PAINT, &later);
    let read = ScenePaint::read(&details).unwrap();
    assert_eq!(read.palette[0].extra.get("sheen_map"), Some(&serde_json::json!("future")));
}

#[test]
fn palette_entries_follow_their_library_material() {
    let mut library = Library::default();
    let shaders = Shaders::default();
    let mut paint = ScenePaint::default();
    let i = library.duplicate(library.named("Clay - Terracotta").unwrap()).unwrap();
    let mine = library.entries()[i].material.clone();
    let slot = ensure_in_palette(&mut paint, &mine, 1.0, &shaders).unwrap();

    // An entry from another file that only shares the name is left alone.
    let mut other = mine.clone();
    other.tint.amount = 0.3;
    let other_slot = ensure_in_palette(&mut paint, &other, 1.0, &shaders).unwrap();
    assert_ne!(other_slot, slot);
    assert_eq!(paint.palette[1].name, "Clay - Terracotta Copy 2");
    assert_eq!(ensure_in_palette(&mut paint, &other, 1.0, &shaders).unwrap(), other_slot, "and is found again");

    // Changing the library material changes what was painted with it.
    let mut changed = mine.clone();
    changed.swatch = [0.9, 0.1, 0.1];
    let before = library.set(i, changed).unwrap();
    let after = library.entries()[i].material.clone();
    assert_eq!(palette::follow_edit(&mut paint, &before, &after, &shaders), 1);
    assert_eq!(material_of(&paint.palette[0]).swatch, [0.9, 0.1, 0.1]);
    assert_eq!(ensure_in_palette(&mut paint, &after, 1.0, &shaders).unwrap(), slot);
    library.rename(i, "Brick").unwrap();
    palette::follow_rename(&mut paint, "Clay - Terracotta Copy", "Brick");
    let renamed = library.entries()[i].material.clone();
    assert_eq!(ensure_in_palette(&mut paint, &renamed, 1.0, &shaders).unwrap(), slot);

    // Replace keeps the slot; Re-measure the reference; Remove Unused the used.
    let gold = library_material(&library, "Metal - Antique Gold");
    palette::replace(&mut paint, slot, &gold, &shaders).unwrap();
    assert_eq!(material_of(&paint.palette[0]).kind(), Kind::Metal);
    palette::remeasure(&mut paint, slot, 3.5).unwrap();
    assert_eq!(paint.palette[0].reference, 3.5);
    assert_eq!(palette::remove_unused(&mut paint, &[slot]), 1);
    assert_eq!(paint.palette.len(), 1);
}

#[test]
fn the_material_to_paint_with_is_kept_with_the_scene() {
    let library = Library::default();
    let shaders = Shaders::default();
    let mut paint = ScenePaint::default();
    let slot = ensure_in_palette(&mut paint, &library_material(&library, "Wood - Mahogany"), 1.0, &shaders).unwrap();
    paint.active_material = Some(slot);
    let mut details = crate::io::Details::default();
    paint.keep(&mut details);
    assert_eq!(ScenePaint::read(&details).unwrap().active_material, Some(slot));
    // One the palette does not have is none; a file without it reads as none.
    paint.active_material = Some(77);
    let mut details = crate::io::Details::default();
    paint.keep(&mut details);
    assert_eq!(ScenePaint::read(&details).unwrap().active_material, None);
    let mut old = serde_json::to_value(&paint).unwrap();
    old.as_object_mut().unwrap().remove("active_material");
    let mut details = crate::io::Details::default();
    details.set(crate::io::slot::PAINT, &old);
    let read = ScenePaint::read(&details).unwrap();
    assert_eq!((read.active_material, read.palette.len()), (None, 1));
}

#[test]
fn a_shader_known_only_from_a_scene_is_carried() {
    let mut shaders = Shaders::default();
    let source = Source { wgsl: GOOD.replace("test_paint", "carried"), sidecar: r#"{ "name": "Carried" }"#.into() };
    shaders.carry("carried", &source);
    assert!(shaders.usable("carried").is_some());
    let shader: &Shader = shaders.get("carried").unwrap();
    assert_eq!(shader.name(), "Carried");
}
