//! `--demo-paint <what>`: the startup sphere painted with four materials of
//! the library in soft-edged wedges and a band of colour, through the paint
//! data's own layer commands (`materials::demo`), set up for a picture with
//! `--screenshot`:
//!
//! - `lit`: the lit look; `matcap`: under the matcap;
//! - `no-materials`, `no-polypaint`: with Show Materials or Show Polypaint off;
//! - `card`: the Materials card open on the Look tab, at 75% interface size;
//! - `custom`: the fourth material the shipped example custom material;
//! - `broken`: the fourth one a custom material that does not compile, which
//!   shows as its swatch while the status line says which line is wrong.
//!
//! It prints the status line, so a script can see what the app said.

use super::Running;
use crate::materials::{Ask, custom, demo};

/// A custom material with a mistake on its second line (no `;`).
const BROKEN_WGSL: &str = "fn finish_demo_broken(s: Surface, params: FinishParams) -> MaterialSample {\n    \
                           var m = material(s, vec3<f32>(1.0, 0.0, 1.0))\n    return m;\n}\n";
const BROKEN_SIDECAR: &str = r#"{ "name": "Demo Broken", "swatch": [0.85, 0.25, 0.75] }"#;

impl Running {
    pub(super) fn paint_demo(&mut self, what: &str) {
        let mut names = ["Metal - Antique Gold", "Clay - Grey Plasteline", "Wood - Golden Oak", "Lure - Blue Flake"];
        if what == "custom" || what == "broken" {
            // A run for a picture writes nothing beside its settings by
            // itself, so the files the demo is about are put in place here.
            if let Some(dir) = self.doc.opts.materials.shaders.folder().map(std::path::Path::to_path_buf) {
                let _ = std::fs::create_dir_all(&dir);
                let files = if what == "broken" {
                    [("demo_broken.wgsl", BROKEN_WGSL), ("demo_broken.json", BROKEN_SIDECAR)]
                } else {
                    [("hammered_copper.wgsl", custom::EXAMPLE_WGSL), ("hammered_copper.json", custom::EXAMPLE_SIDECAR)]
                };
                for (name, text) in files {
                    let _ = std::fs::write(dir.join(name), text);
                }
            }
            let said = self.material_edit(Ask::ReloadShaders);
            println!("reload: {said:?}");
            names[3] = if what == "custom" { "Hammered Copper" } else { "Demo Broken" };
        }
        let mut slots = Vec::new();
        for name in names {
            let Some(i) = self.doc.opts.materials.library.named(name) else {
                eprintln!("demo-paint: no material called {name}");
                continue;
            };
            let _ = self.material_edit(Ask::Select(i));
            match self.doc.paint_slot() {
                Ok(Some(slot)) => slots.push(slot),
                other => eprintln!("demo-paint: {name} did not join the palette: {other:?}"),
            }
        }
        if let Err(why) = demo::paint_bands(&mut self.doc.mesh, &slots, Some([205, 70, 45]), true) {
            eprintln!("demo-paint: {why}");
        }
        self.doc.opts.matcap_on = what == "matcap";
        match what {
            "no-materials" => self.doc.opts.paint.show_materials = false,
            "no-polypaint" => self.doc.opts.paint.show_polypaint = false,
            "card" => {
                self.doc.opts.tabs.scene = crate::ui::tabs::Scene::Look;
                self.doc.opts.ui_scale = 0.75;
                let cards = [("Materials", true), ("Material", false), ("Lighting", false), ("Backdrop & Effects", false)];
                let cards = cards.into_iter().map(|(card, open)| (card.to_owned(), open)).collect();
                crate::prefs::cards::restore(&self.egui_ctx, &cards);
            }
            _ => {}
        }
        // Materials (the picture's and the palette's) are worked out in the frame.
        self.materials_frame();
        println!("palette: {:?}", self.doc.opts.paint.palette.iter().map(|e| (e.slot, e.name.clone())).collect::<Vec<_>>());
        println!("{}", self.ui.status.0);
    }
}
