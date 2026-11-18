//! What the Deform card, the pivot, Apply Scale, Project Detail, the booleans
//! and Quad Remesh keep between frames (deformers.rs, placement.rs,
//! project_detail.rs, combine.rs, quad_remesh.rs), in one place on the app,
//! and the work they do once a frame.

use std::collections::HashMap;

use eframe::egui::{self, TextureId};

use super::{GolemApp, Mode};

#[derive(Default)]
pub(crate) struct Shaping {
    pub deform: super::deformers::DeformState,
    pub pivot: super::placement::PivotState,
    pub project: super::project_detail::ProjectState,
    pub combine: super::combine::CombineState,
    pub quad: super::quad_remesh::QuadState,
    pub icons: Icons,
    /// What a boolean or Join's replacement said about levels or layers lost.
    pub note: String,
}

/// The icons these cards use that the panel's own list has not got, loaded
/// the first time they are asked for.
#[derive(Default)]
pub(crate) struct Icons {
    loaded: HashMap<&'static str, egui::TextureHandle>,
}

macro_rules! icon_files {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../../assets/icons/", $name, ".png")) as &[u8])),*]
    };
}

const FILES: &[(&str, &[u8])] = icon_files![
    "deform_bend", "deform_arc", "deform_twist", "deform_taper", "deform_flatten", "deform_inflate",
    "deform_stretch", "deform_lattice", "bool_subtract", "bool_union", "bool_intersect", "join",
];

impl Icons {
    pub fn get(&mut self, ctx: &egui::Context, name: &str) -> Option<TextureId> {
        if let Some(t) = self.loaded.get(name) {
            return Some(t.id());
        }
        let &(key, bytes) = FILES.iter().find(|(n, _)| *n == name)?;
        let image = image::load_from_memory(bytes).ok()?.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let texture =
            ctx.load_texture(key, egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()), egui::TextureOptions::LINEAR);
        let id = texture.id();
        self.loaded.insert(key, texture);
        Some(id)
    }
}

impl GolemApp {
    /// Once a frame, before the mesh is sent to be drawn.
    pub(crate) fn shaping_frame(&mut self, ctx: &egui::Context, now: f64) {
        if self.mode != Mode::Transform {
            self.shaping.pivot = Default::default();
        }
        self.deform_frame();
        self.quad_frame(ctx, now);
    }
}
