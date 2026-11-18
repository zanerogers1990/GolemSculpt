//! GolemSculpt in the browser.
//!
//! The sculpting engine is the desktop app's own: its files are compiled here
//! as they are (the `#[path]` modules below), so a brush fixed on the desktop
//! is fixed here too. Around it this crate adds only what a web page needs:
//! a small renderer, a small panel, files in and out through the browser.
//! See docs/web-plan.md.

#[path = "../../src/util.rs"]
pub mod util;
#[path = "../../src/mesh/mod.rs"]
pub mod mesh;
#[path = "../../src/sculpt/mod.rs"]
pub mod sculpt;
#[path = "../../src/camera.rs"]
pub mod camera;
#[path = "../../src/remesh.rs"]
pub mod remesh;
#[path = "../../src/printable.rs"]
pub mod printable;
/// Desktop files with sub-files of their own (`decimate.rs` has
/// `decimate/mirror.rs`) are mounted from the desktop's `src` folder as it
/// is, so their own `mod` lines find what they name.
#[path = "../../src"]
mod desktop {
    pub mod decimate;
    // The keyboard shortcuts a person can change (keymap.rs, with keymap/tests.rs).
    pub mod keymap;
    // Project Detail's maths (project.rs, with project/tests.rs).
    pub mod project;
}
pub use desktop::decimate;
pub use desktop::keymap;
pub use desktop::project;
/// The gizmo's deformers (Bend, Twist, Lattice ...) and Quad Remesh, the
/// desktop's maths as it is (web/src/deformers.rs, web/src/quad_remesh.rs).
#[path = "../../src/deform/mod.rs"]
pub mod deform;
#[path = "../../src/quadremesh/mod.rs"]
pub mod quadremesh;
/// The desktop's transform gizmo (the maths: handles, picking, what a drag
/// does) and its masked drag (moving the unmasked part of a surface), both
/// as they are; `scene` is the web version's object list, which the gizmo
/// moves.
#[path = "../../src/gizmo.rs"]
#[allow(dead_code)]
pub mod gizmo;
#[path = "../../src/app/masked.rs"]
#[allow(dead_code)]
pub mod masked;
pub mod scene;
pub mod io;
/// The desktop's SSpheres tool (rig, skin, gestures, armature drawing), as it
/// is; it finds the web's objects at `crate::scene` and the gizmo above.
#[path = "../../src/ssphere/mod.rs"]
#[allow(dead_code)]
pub mod ssphere;

mod app;
mod browser;
pub mod files;
mod render;
mod tools;
mod mask_fit;
// FBX in and out (the desktop's own reader and writer), and the settings of
// the desktop's `doc` that desktop files compiled here reach for.
pub mod fbx;
// Reference pictures: where one stands, reading its image (the desktop's own).
#[path = "../../src/references.rs"]
pub mod references;
mod doc;

pub use app::GolemApp;

/// The many-threads build's thread pool: index.html calls `initThreadPool`
/// with the number of workers before it calls `start`, so the pool is up
/// before the first brush stroke asks rayon for help.
#[cfg(all(target_arch = "wasm32", feature = "threads"))]
pub use wasm_bindgen_rayon::init_thread_pool;

/// Where the page starts the app: on the canvas `golem_canvas`.
///
/// index.html calls this itself once the module is loaded (and, in the
/// many-threads build, once the thread pool is up). It is not the module's
/// automatic start function because in that build every worker thread loads
/// the module too, and a worker has no page to start on.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start() {
    use wasm_bindgen::JsCast;

    console_error_panic_hook::set_once();
    eframe::WebLogger::init(log::LevelFilter::Warn).ok();
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().and_then(|w| w.document()).expect("a page");
        let canvas = document
            .get_element_by_id("golem_canvas")
            .and_then(|c| c.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("the page has a canvas called golem_canvas");
        let started = eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(GolemApp::new(cc)))))
            .await;
        if let Err(e) = started {
            let text = format!("GolemSculpt could not start in this browser: {e:?}");
            // The loading screen's message line (index.html), shown as a failure.
            if let Some(note) = document.get_element_by_id("golem_status") {
                note.set_text_content(Some(&text));
            }
            if let Some(screen) = document.get_element_by_id("golem_loading") {
                screen.set_class_name("failed");
            }
            log::error!("{text}");
        }
    });
}
