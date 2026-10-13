//! The alpha under the cursor: the picture the brush in hand will print,
//! shown faintly inside the ring so a print can be aimed before it is made.
//!
//! It is drawn with the ring and only with it (`overlay::cursor`), so it is
//! away wherever the ring is: on the clean view, over a panel, in a picture
//! of the model alone. A brush with no alpha has nothing here and costs
//! nothing: the app hands over a [`Preview`] only when there is a picture.
//!
//! The picture lies flat on the screen, the size it will be against the
//! ring and turned as it will be turned. On a surface seen at a slant the
//! print itself is foreshortened and this is not: it shows where the
//! picture's middle and its top will go, which is what aiming needs.

use std::collections::VecDeque;

use egui::epaint::{Mesh, Vertex};
use egui::{Color32, ColorImage, Id, Painter, Pos2, Shape, TextureHandle, TextureId, TextureOptions, Vec2, pos2};

use crate::sculpt::alpha::AlphaImage;

/// What to show: handed over by the app each frame there is something to.
#[derive(Clone, Copy)]
pub struct Preview {
    pub image: &'static AlphaImage,
    /// The picture's size against the ring's: at 1 it fills the square the
    /// ring sits in.
    pub scale: f32,
    /// The way the picture's right edge and its top point on the screen (x
    /// across, y down), each one unit long.
    pub right: Vec2,
    pub up: Vec2,
    /// Black prints and white does not.
    pub invert: bool,
}

/// Pixels along a side of the picture as the graphics card holds it for the
/// preview: enough for a ring that fills a good part of the window.
const SIDE: usize = 128;
/// How many pictures are kept ready. One is in use at a time; the few
/// before it are kept so that going back and forth between brushes does
/// not make them again.
const KEPT: usize = 4;
/// Squares along a side of the patch the picture is drawn on. Each corner
/// has its own opacity, which is how the patch is faded out at the ring.
const CELLS: usize = 24;
/// How strongly the picture shows at its whitest: faint, so the surface
/// under it can still be read.
const STRENGTH: f32 = 0.3;
/// The share of the ring's radius from which the picture fades to nothing
/// at the ring itself, as the brush's own weight does.
const FADE_FROM: f32 = 0.8;

/// The pictures made so far, newest first: each with the picture it was made
/// from (by where it lives: the library never frees or moves one) and
/// whether it was made inverted.
#[derive(Clone, Default)]
struct Made(VecDeque<(usize, bool, TextureHandle)>);

/// The picture as a texture: white, as opaque as the picture is white.
fn texture(ctx: &egui::Context, image: &'static AlphaImage, invert: bool) -> TextureId {
    let id = Id::new("alpha_preview");
    let key = image as *const AlphaImage as usize;
    let made = ctx.data(|d| d.get_temp::<Made>(id)).unwrap_or_default();
    if let Some((_, _, handle)) = made.0.iter().find(|(of, inverted, _)| (*of, *inverted) == (key, invert)) {
        return handle.id();
    }
    let grey = image.thumbnail(SIDE);
    let shade = |g: u8| if invert { 255 - g } else { g };
    let pixels = grey.iter().map(|&g| Color32::from_white_alpha(shade(g))).collect();
    let handle = ctx.load_texture(format!("alpha_preview/{}", image.name), ColorImage::new([SIDE, SIDE], pixels), TextureOptions::LINEAR);
    let texture = handle.id();
    ctx.data_mut(|d| {
        let made = &mut d.get_temp_mut_or_default::<Made>(id).0;
        made.push_front((key, invert, handle));
        made.truncate(KEPT);
    });
    texture
}

/// How much of the picture shows `from_centre` ring radii from the middle
/// of the ring: all of it out to [`FADE_FROM`], none at the ring.
fn shown(from_centre: f32) -> f32 {
    let t = ((1.0 - from_centre) / (1.0 - FADE_FROM)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The patch the picture is drawn on: a grid over the picture's square,
/// each corner as opaque as [`shown`] says for where it is. `radius` is the
/// ring's, in points.
fn patch(texture: TextureId, centre: Pos2, radius: f32, tint: Color32, preview: &Preview) -> Mesh {
    let mut mesh = Mesh::with_texture(texture);
    let half = radius * preview.scale;
    let along = CELLS as u32 + 1;
    for row in 0..along {
        for column in 0..along {
            // Across the picture and down it, 0 to 1 each way.
            let (x, y) = (column as f32 / CELLS as f32, row as f32 / CELLS as f32);
            let off = (preview.right * (2.0 * x - 1.0) + preview.up * (1.0 - 2.0 * y)) * half;
            let color = tint.gamma_multiply(STRENGTH * shown(off.length() / radius));
            mesh.vertices.push(Vertex { pos: centre + off, uv: pos2(x, y), color });
        }
    }
    for row in 0..CELLS as u32 {
        for column in 0..CELLS as u32 {
            let corner = row * along + column;
            mesh.add_triangle(corner, corner + 1, corner + along);
            mesh.add_triangle(corner + 1, corner + along + 1, corner + along);
        }
    }
    mesh
}

/// Draws the preview inside a ring of `radius` points about `centre`, in
/// the ring's own colour.
pub fn draw(painter: &Painter, centre: Pos2, radius: f32, tint: Color32, preview: &Preview) {
    // Too small to make anything of: a few points across.
    if !(radius * preview.scale >= 3.0) {
        return;
    }
    let texture = texture(painter.ctx(), preview.image, preview.invert);
    painter.add(Shape::mesh(patch(texture, centre, radius, tint, preview)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::alpha;

    fn preview(scale: f32, right: Vec2, up: Vec2) -> Preview {
        Preview { image: alpha::find("Rake").expect("the Rake alpha ships"), scale, right, up, invert: false }
    }

    /// The picture's own corners go where the brush will print them: its
    /// top left up and to the left of the ring's middle, at the picture's
    /// size against the ring; and turned, they turn with it.
    #[test]
    fn the_patch_lies_where_the_print_will() {
        let (centre, radius) = (pos2(400.0, 300.0), 50.0);
        let upright = preview(1.0, Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0));
        let mesh = patch(TextureId::default(), centre, radius, Color32::WHITE, &upright);
        let corner = |mesh: &Mesh, u: f32, v: f32| mesh.vertices.iter().find(|vertex| vertex.uv == pos2(u, v)).unwrap().pos;
        assert_eq!(corner(&mesh, 0.0, 0.0), pos2(350.0, 250.0), "the picture's top left");
        assert_eq!(corner(&mesh, 1.0, 1.0), pos2(450.0, 350.0), "its bottom right");
        assert_eq!(corner(&mesh, 0.5, 0.5), centre);
        assert_eq!(mesh.indices.len(), CELLS * CELLS * 6);
        assert!(mesh.is_valid());

        // Half the size, and its top pointing right across the screen (a
        // stroke heading that way with Turn With Stroke on).
        let turned = preview(0.5, Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0));
        let mesh = patch(TextureId::default(), centre, radius, Color32::WHITE, &turned);
        let top_middle = corner(&mesh, 0.5, 0.0);
        assert!((top_middle - pos2(425.0, 300.0)).length() < 1e-3, "the top of the picture leads: {top_middle:?}");
        let top_left = corner(&mesh, 0.0, 0.0);
        assert!((top_left - pos2(425.0, 275.0)).length() < 1e-3, "{top_left:?}");
    }

    /// Faint everywhere, whole in the middle of the ring and gone at the
    /// ring itself, so the square's corners do not stick out past it.
    #[test]
    fn the_picture_fades_out_at_the_ring() {
        assert_eq!(shown(0.0), 1.0);
        assert_eq!(shown(FADE_FROM), 1.0);
        assert!(shown(0.9) > 0.0 && shown(0.9) < 1.0);
        assert_eq!(shown(1.0), 0.0);
        assert_eq!(shown(1.4), 0.0, "the corners of the square are outside the ring");
        let upright = preview(1.0, Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0));
        let mesh = patch(TextureId::default(), pos2(0.0, 0.0), 50.0, Color32::WHITE, &upright);
        let strongest = mesh.vertices.iter().map(|vertex| vertex.color.a()).max().unwrap();
        assert!((60..=90).contains(&strongest), "faint at its strongest: {strongest} of 255");
        for vertex in &mesh.vertices {
            let out = vertex.pos.to_vec2().length() >= 50.0;
            assert!(!out || vertex.color == Color32::TRANSPARENT, "{:?} shows outside the ring", vertex.pos);
        }
    }

    /// One texture per picture and way up, made once and found again.
    #[test]
    fn a_picture_is_made_ready_once() {
        let ctx = egui::Context::default();
        let (rake, square) = (alpha::find("Rake").unwrap(), alpha::find("Square").unwrap());
        let first = texture(&ctx, rake, false);
        assert_eq!(texture(&ctx, rake, false), first);
        assert_ne!(texture(&ctx, rake, true), first, "inverted is another picture");
        assert_ne!(texture(&ctx, square, false), first);
        assert_eq!(texture(&ctx, rake, false), first, "going back to a brush finds its picture still there");
        let kept = ctx.data(|d| d.get_temp::<Made>(Id::new("alpha_preview"))).unwrap().0.len();
        assert_eq!(kept, 3);
    }
}
