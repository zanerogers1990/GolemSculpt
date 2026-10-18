//! Material thumbnails: each material drawn on a small grooved ball, the way
//! material libraries in other sculpting apps show theirs. The drawing is the
//! renderer's (app/materials.rs draws one on a side picture, in a submission
//! of its own, after the window's frame); this is what it is drawn on and
//! where the pictures are kept.
//!
//! A picture is kept as a PNG in `materials/thumbnails`, named by a hash of
//! the material as a file keeps it, its shader's source if it has one, and
//! [`VERSION`], so a changed material or shader gets a new picture and the
//! old one is simply not asked for again. Bump [`VERSION`] when the ball,
//! the lighting or the shading changes, and every picture is drawn afresh.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use glam::Vec3;

use super::custom::Shaders;
use super::{Material, Settings};
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

/// Changes every picture's name: bump it when the way they are drawn changes.
pub const VERSION: u32 = 1;
/// The picture's side as kept and shown; it is drawn at twice this and
/// halved, which is its antialiasing.
pub const SIDE: u32 = 96;
pub const DRAW_SIDE: u32 = SIDE * 2;
/// The folder's name, inside the materials folder.
pub const FOLDER: &str = "thumbnails";

/// The ball: a sphere of about 20,000 triangles with a groove round its
/// middle and two dimples, so finishes that settle in recesses (metal's
/// wash, clay's darker hollows, wood's grime) have one to settle in.
pub fn ball() -> Mesh {
    let sphere = primitives::build(Primitive::Sphere, 20_000, 1.0);
    let dimples = [Vec3::new(0.45, 0.55, 0.7).normalize(), Vec3::new(-0.5, -0.45, 0.74).normalize()];
    let positions = sphere
        .positions
        .iter()
        .map(|&p| {
            let n = p.normalize_or_zero();
            let groove = 0.07 * (-(n.y / 0.07).powi(2)).exp();
            let dimple: f32 = dimples.iter().map(|d| 0.05 * (-(n.distance(*d) / 0.16).powi(2)).exp()).sum();
            p - n * (groove + dimple)
        })
        .collect();
    Mesh::new(positions, sphere.indices.clone(), None)
}

/// The name of `material`'s picture: a hash of everything that shows in it.
pub fn key(material: &Material, shaders: &Shaders) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    VERSION.hash(&mut h);
    let mut kept = material.clone();
    // The name is not in the picture: a renamed material keeps its picture.
    kept.name.clear();
    kept.source = None;
    serde_json::to_string(&kept).unwrap_or_default().hash(&mut h);
    if let Settings::Custom(c) = &material.settings {
        match shaders.get(&c.id) {
            Some(shader) => (shader.hash(), shader.ok()).hash(&mut h),
            None => "missing".hash(&mut h),
        }
    }
    h.finish()
}

/// Where the pictures are kept.
#[derive(Default)]
pub struct Cache {
    dir: Option<PathBuf>,
    writable: bool,
}

impl Cache {
    /// The pictures of the materials folder `materials`.
    pub fn new(materials: Option<&Path>, writable: bool) -> Cache {
        Cache { dir: materials.map(|m| m.join(FOLDER)), writable }
    }

    fn path(&self, key: u64) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(format!("{key:016x}.png")))
    }

    /// The picture kept for `key`, if there is one of the right size.
    pub fn load(&self, key: u64) -> Option<image::RgbaImage> {
        let image = image::open(self.path(key)?).ok()?.to_rgba8();
        (image.dimensions() == (SIDE, SIDE)).then_some(image)
    }

    /// Keeps `picture` for `key`. A picture that cannot be kept is drawn
    /// again next time; nothing else comes of it.
    pub fn store(&self, key: u64, picture: &image::RgbaImage) {
        let Some(path) = self.path(key).filter(|_| self.writable) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = picture.save(&path) {
            log::warn!("could not keep a material thumbnail: {e}");
        }
    }
}

/// `picture` (drawn at [`DRAW_SIDE`]) halved to [`SIDE`], each pixel the
/// mean of four, with the background made see-through: the ball is drawn
/// on black, and the thumbnail sits on the card's own colour.
pub fn finish(picture: &image::RgbaImage) -> image::RgbaImage {
    let (w, h) = picture.dimensions();
    let mut out = image::RgbaImage::new(SIDE, SIDE);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let mut sum = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                let p = picture.get_pixel(sx, sy).0;
                // Pure black is the backdrop; anything else is the ball.
                let solid = p[0] > 0 || p[1] > 0 || p[2] > 0;
                for c in 0..3 {
                    sum[c] += p[c] as u32;
                }
                sum[3] += if solid { 255 } else { 0 };
            }
            let a = sum[3] / 4;
            // Colour of the ball's own pixels only, so its rim is not darkened.
            let solid_count = (sum[3] / 255).max(1);
            let c = |k: usize| (sum[k] / solid_count).min(255) as u8;
            out.put_pixel(x, y, image::Rgba([c(0), c(1), c(2), a as u8]));
        }
    }
    out
}

/// The ball's size, which a material's patterns are measured against on it.
pub const BALL_SIZE: f32 = 2.0;

/// The ball with paint on it: one layer, all of it the material in palette
/// slot 1 (the thumbnail's own one-entry palette), put on through the paint
/// data's own commands.
pub fn painted_ball() -> Mesh {
    use crate::mesh::paint::{ChannelSet, FillArea, Mat8, PaintMode, PaintTarget, Texel};
    let mut ball = ball();
    let painted = ball.add_paint_layer(Some("Material")).and_then(|_| ball.add_paint_material(1)).and_then(|_| {
        let texel = Texel { material: Mat8([1, 255]), ..Texel::BARE };
        let target = PaintTarget { texel, channels: ChannelSet::MATERIAL, mode: PaintMode::Paint };
        ball.fill_paint(&target, FillArea::All)
    });
    if let Err(why) = painted {
        log::warn!("the thumbnail ball could not be painted: {why}");
    }
    ball
}

/// How a thumbnail is lit and shown: the Studio rig, on black, with nothing
/// over the picture (no vignette, bloom, depth of field or occlusion, which
/// read as dirt at this size), and both painted colour and materials showing.
pub fn options() -> crate::doc::Options {
    let mut opts = crate::doc::Options::default();
    opts.matcap_on = false;
    opts.cavity = false;
    opts.gradient_background = false;
    opts.background = [0.0; 3];
    let look = &mut opts.look;
    look.lighting = 0;
    look.ambient_occlusion = false;
    look.bloom = false;
    look.vignette = false;
    look.depth_of_field = false;
    look.follow_camera = true;
    opts.paint.show_polypaint = true;
    opts.paint.show_materials = true;
    opts
}

/// The frame a thumbnail is drawn at: the ball's bounds, a fixed clock.
pub fn frame(view: &crate::camera::View, bounds: crate::mesh::bvh::Aabb) -> crate::look::Frame {
    crate::look::Frame {
        cam_right: view.right,
        cam_up: view.up,
        cam_forward: view.forward,
        ortho_half_height: view.ortho_half_height,
        eye_offset: 0.0,
        scene_min: bounds.min,
        scene_max: bounds.max,
        model_size: BALL_SIZE,
        time: 1.25,
        gradient_background: false,
        smoothness: 0.4,
        shadow_size: crate::render::quality::Quality::default().shadow_size(),
    }
}
