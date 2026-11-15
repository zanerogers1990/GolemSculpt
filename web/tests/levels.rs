//! Subdivision levels and sculpt layers kept in the browser's storage
//! (src/io.rs: the desktop's own file blocks) come back as they were.
//! Here rather than in the crate: its own tests would pull in the desktop's.

use glam::Vec2;
use golem_web::camera::Camera;
use golem_web::io::{build_mesh, level_blocks};
use golem_web::mesh::levels;
use golem_web::mesh::primitives::{self, Primitive};
use golem_web::mesh::Mesh;
use golem_web::sculpt::{PointerSample, Sculptor};
use sculpt_multires::Split;

/// A stroke across the middle of the view, as the pointer would draw it.
fn stroke(mesh: &mut Mesh) {
    let mut camera = Camera::default();
    let bounds = mesh.bvh.bounds();
    camera.frame(bounds, 1.0, false);
    let size = Vec2::new(800.0, 800.0);
    let view = camera.view(Vec2::ZERO, size, camera.distance, bounds.size().max_element());
    let mut sculptor = Sculptor::default();
    sculptor.library.select_named("Clay Strips", &mut sculptor.settings);
    let at = |k: f32, t: f64| PointerSample { pos: Vec2::new(300.0 + k * 200.0, 400.0), pressure: None, time: t };
    assert!(sculptor.begin(mesh, &view, at(0.0, 0.0), false, false), "the stroke starts on the ball");
    for k in 1..=20 {
        sculptor.sample(mesh, &view, at(k as f32 / 20.0, k as f64 * 0.016));
    }
    assert!(sculptor.end(mesh, &view).is_some(), "the stroke moved something");
}

fn kept(mesh: &Mesh) -> Mesh {
    let (lvls, layr) = level_blocks(mesh);
    let hidden = mesh.hidden_triangles().map(<[bool]>::to_vec).unwrap_or_default();
    let (back, lost) = build_mesh(mesh.positions.clone(), mesh.indices.clone(), mesh.mask.clone(), hidden, &lvls, &layr);
    assert!(!lost, "nothing was left off");
    back
}

#[test]
fn levels_and_layers_come_back() {
    let ball = primitives::build(Primitive::Sphere, 2_000, 1.0);
    let (once, _) = levels::subdivided(&ball, Split::Smooth).unwrap();
    let (mut twice, _) = levels::subdivided(&once, Split::Smooth).unwrap();
    let (_, step) = twice.add_layer(None);
    drop(step);
    stroke(&mut twice);
    assert!(twice.layers.as_ref().unwrap().offsets(1).unwrap().iter().any(|o| *o != glam::Vec3::ZERO), "the stroke went into the layer");

    // On the finest level.
    let back = kept(&twice);
    assert_eq!(back.level_count(), 3);
    assert_eq!(back.levels.as_ref().unwrap().current(), 2);
    assert_eq!(back.layers_print(), twice.layers_print(), "shape and layers the same");

    // One level down: the levels keep the layer's finest copy too.
    let (low, _) = levels::stepped(&twice, 1).unwrap();
    let back = kept(&low);
    assert_eq!(back.levels.as_ref().unwrap().current(), 1);
    assert_eq!(back.layers_print(), low.layers_print());
    // And going back up from what was kept gives what going up gives.
    let (up, _) = levels::stepped(&back, 2).unwrap();
    let (up_here, _) = levels::stepped(&low, 2).unwrap();
    assert_eq!(up.layers_print(), up_here.layers_print(), "the way back up is the same from the kept copy");
}

#[test]
fn a_plain_mesh_keeps_nothing_extra() {
    let ball = primitives::build(Primitive::Sphere, 500, 1.0);
    let (lvls, layr) = level_blocks(&ball);
    assert!(lvls.is_empty() && layr.is_empty());
    let back = kept(&ball);
    assert_eq!(back.level_count(), 1);
    assert_eq!(back.layer_count(), 0);
}
