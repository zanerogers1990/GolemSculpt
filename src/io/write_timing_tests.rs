//! How long the window's thread waits on a save now that the disk is written
//! by a thread of its own (`SceneBytes`, app/files/writing.rs), against the
//! whole save as it was. `cargo test --release write_timing -- --ignored --nocapture`.

use std::time::Instant;

use glam::{Quat, Vec3};

use super::{Details, SavedObject, SceneBytes, save_scene_with};
use crate::mesh::primitives::{self, Primitive};

#[test]
#[ignore]
fn write_timing() {
    // SCULPT_TIMING_DIR times another disk (the one the sculpts are kept on).
    let base = std::env::var_os("SCULPT_TIMING_DIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    let folder = base.join("sculpt_write_timing");
    let _ = std::fs::create_dir_all(&folder);
    for triangles in [1_000_000, 5_000_000, 10_000_000] {
        let mesh = primitives::build(Primitive::Sphere, triangles, 0.5);
        let object = SavedObject { name: "Ball".into(), visible: true, mesh: 0, position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        let details = Details::default();
        let path = folder.join(format!("ball {triangles}.sclp"));
        let whole = Instant::now();
        save_scene_with(&[&mesh], std::slice::from_ref(&object), 0, &details, &path).unwrap();
        let whole = whole.elapsed();
        let paused = Instant::now();
        let bytes = SceneBytes::of(&[&mesh], std::slice::from_ref(&object), 0, &details, &path).unwrap();
        let paused = paused.elapsed();
        let behind = Instant::now();
        bytes.write(&path).unwrap();
        let behind = behind.elapsed();
        println!(
            "{} triangles: save as it was {:.0} ms; now the window waits {:.0} ms and the thread writes for {:.0} ms",
            mesh.triangle_count(),
            whole.as_secs_f64() * 1000.0,
            paused.as_secs_f64() * 1000.0,
            behind.as_secs_f64() * 1000.0
        );
        let opened = Instant::now();
        let back = super::load_scene(&path).unwrap();
        println!("  reading and building it for Open: {:.0} ms, now off the window's thread", opened.elapsed().as_secs_f64() * 1000.0);
        drop(back);
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::remove_dir_all(&folder);
}
