//! Reference pictures: photos or drawings of the subject, from the front, the
//! side and above, standing in the scene as flat pictures to sculpt against,
//! as ZBrush, Blender and Nomad have them. Matching the model's outline to a
//! photo's in the front and side views is how a likeness, or a design drawn
//! on paper, is carried into the clay.
//!
//! A picture is a view aid, not part of the model. It is not an object: it is
//! in none of the lists that brushes, picking, Frame, Remesh, Join, the
//! booleans, exports and Send to Blender read, and none of the pictures made
//! to keep (a render, a turntable loop, a timelapse) shows it
//! (render/references.rs). But it belongs to the scene: it is saved in the
//! `.sclp` with everything else, image and all (the image file's own bytes,
//! so the scene opens with its pictures on any computer: io/references.rs),
//! and adding, moving or deleting one is a change Save is asked about,
//! although none of it is an undo step, as a symmetry setting is not
//! (app/unsaved.rs).
//!
//! Where a picture stands is said the way a sculptor sets one up: which view
//! it faces (Front faces the front view, which sees it straight on and the
//! right way round), how tall it is, how far left or right and up or down its
//! middle is, and how far behind the middle of the scene (where the axes
//! cross) it stands, each along the picture's own axes. Those numbers are
//! what is kept, not a point in the world, so a picture turned to face
//! another view stays as far off-centre and as far back as it was.

use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::Vec3;
use image::{ImageDecoder, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::io::{self, Details};
use crate::mesh::bvh::Aabb;

/// The longest side a picture is kept at, in pixels. A photo from a camera
/// is often twice this; halved it is still finer than the screen it is seen
/// on, and a quarter of the graphics card's memory.
pub const LARGEST_SIDE: u32 = 4096;

/// The largest image file taken, in bytes: far past any photo, and short of
/// what would make every save of the scene slow.
const LARGEST_FILE: u64 = 200 << 20;

/// The kinds of file a picture can come from.
pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// How far behind the model a new picture stands, as a share of the scene's
/// longest side: clear of it, and close enough to read as its backdrop.
const BEHIND: f32 = 0.05;

/// The size of the scene a new picture is fitted to when the scene shows
/// nothing to fit it to.
const NOTHING_SHOWN: f32 = 2.0;

/// What the Reference Images card asks of the app (ui/references.rs). The
/// card changes a picture's settings itself; these need the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// The Add Picture... button: choose an image file and stand it in the
    /// scene.
    Add,
    /// Take this picture out of the scene (the card has asked first).
    Delete(u64),
}

/// Which view a picture faces: the one that sees it straight on, the right
/// way round, as the view gizmo turns to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Facing {
    #[default]
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

impl Facing {
    pub const ALL: [Facing; 6] = [Facing::Front, Facing::Back, Facing::Left, Facing::Right, Facing::Top, Facing::Bottom];

    pub fn name(self) -> &'static str {
        match self {
            Facing::Front => "Front",
            Facing::Back => "Back",
            Facing::Left => "Left",
            Facing::Right => "Right",
            Facing::Top => "Top",
            Facing::Bottom => "Bottom",
        }
    }

    /// The picture's right, its up, and the way it faces (toward whoever
    /// looks at it from its view), in the world: exactly the right, up and
    /// back of the camera in that view. The three always make a right-handed
    /// set (right × up is the way it faces), so its corners go round the same
    /// way, seen from the front, whichever view it faces.
    pub fn axes(self) -> (Vec3, Vec3, Vec3) {
        match self {
            Facing::Front => (Vec3::X, Vec3::Y, Vec3::Z),
            Facing::Back => (Vec3::NEG_X, Vec3::Y, Vec3::NEG_Z),
            Facing::Right => (Vec3::NEG_Z, Vec3::Y, Vec3::X),
            Facing::Left => (Vec3::Z, Vec3::Y, Vec3::NEG_X),
            // The front of the model toward the bottom of the screen, as the
            // view gizmo brings the top and bottom views up.
            Facing::Top => (Vec3::X, Vec3::NEG_Z, Vec3::Y),
            Facing::Bottom => (Vec3::X, Vec3::Z, Vec3::NEG_Y),
        }
    }

    /// The view that looks straight at it, as yaw and pitch in degrees: the
    /// one the view gizmo turns to (ui/overlay.rs).
    pub fn view(self) -> (f32, f32) {
        match self {
            Facing::Front => (0.0, 0.0),
            Facing::Back => (180.0, 0.0),
            Facing::Right => (-90.0, 0.0),
            Facing::Left => (90.0, 0.0),
            Facing::Top => (0.0, 90.0),
            Facing::Bottom => (0.0, -90.0),
        }
    }

    /// The facing whose view a camera at `yaw` and `pitch` (degrees) is in,
    /// if it is in one of them: what a new picture faces.
    pub fn of_view(yaw: f32, pitch: f32) -> Option<Facing> {
        const CLOSE: f32 = 0.5;
        let apart = |a: f32, b: f32| ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs();
        if (pitch - 90.0).abs() < CLOSE {
            return Some(Facing::Top);
        }
        if (pitch + 90.0).abs() < CLOSE {
            return Some(Facing::Bottom);
        }
        if pitch.abs() >= CLOSE {
            return None;
        }
        Facing::ALL.into_iter().filter(|f| f.view().1 == 0.0).find(|f| apart(yaw, f.view().0) < CLOSE)
    }
}

/// Where a picture stands and how it shows: what the Reference Images card
/// sets, and what a file keeps of it. Lengths are in the scene's units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    pub facing: Facing,
    /// How tall it is. How wide follows from its image.
    pub size: f32,
    /// How far its middle is to the right (Left/Right) and up (Up/Down) of
    /// the middle of the scene, along its own right and up.
    pub across: f32,
    pub up: f32,
    /// How far behind the middle of the scene it stands, along the way it
    /// faces; negative is in front of it.
    pub depth: f32,
    /// 0 (not seen) to 1 (solid).
    pub opacity: f32,
    /// Mirrored left to right: one photo of a side used for both sides.
    pub flip: bool,
    /// Drawn over the model, see-through by its opacity, to trace an
    /// outline. Otherwise the model hides it wherever the model is in front.
    pub in_front: bool,
    pub shown: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Placement {
            facing: Facing::Front,
            size: NOTHING_SHOWN,
            across: 0.0,
            up: 0.0,
            depth: NOTHING_SHOWN * (0.5 + BEHIND),
            opacity: 1.0,
            flip: false,
            in_front: false,
            shown: true,
        }
    }
}

impl Placement {
    /// A new picture facing `facing`, fitted to what the scene shows
    /// (`bounds`): as tall as the scene is along the picture's up, its middle
    /// on the scene's middle, and standing just behind the scene, so the
    /// model is seen against it. A scene that shows nothing fits it to a
    /// space the size of a new shape round the middle.
    pub fn fitted(facing: Facing, bounds: Aabb) -> Placement {
        let (lo, hi) = if bounds.min.is_finite() && bounds.max.is_finite() && bounds.min.cmple(bounds.max).all() {
            (bounds.min, bounds.max)
        } else {
            (Vec3::splat(-NOTHING_SHOWN * 0.5), Vec3::splat(NOTHING_SHOWN * 0.5))
        };
        let (right, up, toward) = facing.axes();
        let (centre, half) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
        let longest = (hi - lo).max_element();
        let longest = if longest > 1e-6 { longest } else { NOTHING_SHOWN };
        let tall = (hi - lo).dot(up).abs();
        // The back of the scene as the picture's view sees it, along the way it faces.
        let back = centre.dot(toward) - half.dot(toward.abs());
        Placement {
            facing,
            size: if tall > 1e-6 { tall } else { longest },
            across: centre.dot(right),
            up: centre.dot(up),
            depth: longest * BEHIND - back,
            ..Placement::default()
        }
    }

    /// Where its middle is in the world.
    pub fn centre(&self) -> Vec3 {
        let (right, up, toward) = self.facing.axes();
        right * self.across + up * self.up - toward * self.depth
    }

    /// Its four corners in the world for an image `aspect` times as wide as
    /// it is tall: bottom left, bottom right, top right, top left, as seen
    /// from the view it faces (so they go round anticlockwise from there).
    pub fn corners(&self, aspect: f32) -> [Vec3; 4] {
        let (right, up, _) = self.facing.axes();
        let centre = self.centre();
        let half_tall = self.size * 0.5;
        let half_wide = half_tall * aspect;
        let (x, y) = (right * half_wide, up * half_tall);
        [centre - x - y, centre + x - y, centre + x + y, centre - x + y]
    }

    /// Where in its image each corner of [`Self::corners`] is (0 to 1, from
    /// the image's top left), mirrored left to right when it is flipped.
    pub fn image_corners(&self) -> [[f32; 2]; 4] {
        let (left, right) = if self.flip { (1.0, 0.0) } else { (0.0, 1.0) };
        [[left, 1.0], [right, 1.0], [right, 0.0], [left, 0.0]]
    }
}

/// A new number for a picture, never given to another in this run: the
/// renderer keeps each picture's image on the graphics card by it, so a
/// picture that comes from a file opened later never borrows the image of
/// one that went.
fn fresh_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A fingerprint of an image file's bytes, to know the same image again
/// without comparing every byte.
fn print_of(file: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut hasher);
    hasher.finish()
}

/// One reference picture of the scene.
pub struct Picture {
    /// Its number in this run (see [`fresh_id`]).
    pub id: u64,
    /// The name of the file it came from, for the list.
    pub name: String,
    /// The image file's own bytes, as they were: what a save keeps.
    pub file: Arc<[u8]>,
    /// The image as it is drawn: read from `file`, turned the way the
    /// camera that took it was held, and no longer than [`LARGEST_SIDE`].
    pub image: Arc<RgbaImage>,
    /// [`print_of`] its file.
    pub print: u64,
    pub place: Placement,
}

impl Picture {
    pub fn new(name: String, file: Arc<[u8]>, image: Arc<RgbaImage>, place: Placement) -> Picture {
        let print = print_of(&file);
        Picture { id: fresh_id(), name, file, image, print, place }
    }

    /// How many times as wide as tall its image is.
    pub fn aspect(&self) -> f32 {
        self.image.width().max(1) as f32 / self.image.height().max(1) as f32
    }

    /// How wide it stands in the scene.
    pub fn width(&self) -> f32 {
        self.place.size * self.aspect()
    }

    /// Its corners in the world (see [`Placement::corners`]).
    pub fn corners(&self) -> [Vec3; 4] {
        self.place.corners(self.aspect())
    }
}

/// What a file keeps of one picture: its name, which of the file's images it
/// shows (by number: io/references.rs), and its placement.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub name: String,
    pub image: usize,
    pub place: Placement,
}

/// The scene's reference pictures, in the order the card lists them.
#[derive(Default)]
pub struct References {
    pub pictures: Vec<Picture>,
    /// The picture whose settings the card shows.
    pub selected: Option<u64>,
}

impl References {
    /// Adds `picture` at the end of the list, selected. Gives back its number.
    pub fn add(&mut self, picture: Picture) -> u64 {
        let id = picture.id;
        self.pictures.push(picture);
        self.selected = Some(id);
        id
    }

    pub fn get(&self, id: u64) -> Option<&Picture> {
        self.pictures.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Picture> {
        self.pictures.iter_mut().find(|p| p.id == id)
    }

    /// The picture the card shows the settings of: the one selected, else
    /// the first.
    pub fn selected(&self) -> Option<&Picture> {
        self.selected.and_then(|id| self.get(id)).or(self.pictures.first())
    }

    /// Takes a picture out of the scene. The selection goes to the one that
    /// took its place in the list, else the one before.
    pub fn remove(&mut self, id: u64) -> Option<Picture> {
        let at = self.pictures.iter().position(|p| p.id == id)?;
        let gone = self.pictures.remove(at);
        let next = self.pictures.get(at).or(at.checked_sub(1).and_then(|i| self.pictures.get(i)));
        self.selected = next.map(|p| p.id);
        Some(gone)
    }

    /// The image already in the scene that is the very file `file`, decoded,
    /// with the bytes it is kept as: a picture added twice (one photo for
    /// both sides) is held in memory and in the file once.
    pub fn same_image(&self, file: &[u8]) -> Option<(Arc<[u8]>, Arc<RgbaImage>)> {
        let print = print_of(file);
        self.pictures
            .iter()
            .find(|p| p.print == print && *p.file == *file)
            .map(|p| (p.file.clone(), p.image.clone()))
    }

    /// Feeds everything a file keeps of the pictures to `hasher`, for the
    /// fingerprint that tells whether the scene has changed since it was
    /// saved (app/unsaved.rs): each image by its fingerprint, each name and
    /// placement, and their order. Taken apart by name, so that a field added
    /// to `Placement` (which is saved) does not compile until it is hashed
    /// here too.
    pub fn hash_kept(&self, hasher: &mut impl Hasher) {
        self.pictures.len().hash(hasher);
        for picture in &self.pictures {
            let Placement { facing, size, across, up, depth, opacity, flip, in_front, shown } = picture.place;
            (picture.print, &picture.name, facing, flip, in_front, shown).hash(hasher);
            for value in [size, across, up, depth, opacity] {
                value.to_bits().hash(hasher);
            }
        }
    }

    /// Puts the pictures into what a file keeps (`io::Details`): their
    /// images, each once however many pictures show it, and the list of
    /// names and placements naming them. Nothing at all when there are none.
    pub fn keep(&self, details: &mut Details) {
        if self.pictures.is_empty() {
            return;
        }
        let mut images: Vec<Arc<[u8]>> = Vec::new();
        let mut saved = Vec::new();
        for picture in &self.pictures {
            let image = images.iter().position(|kept| Arc::ptr_eq(kept, &picture.file) || **kept == *picture.file);
            let image = image.unwrap_or_else(|| {
                images.push(picture.file.clone());
                images.len() - 1
            });
            saved.push(Saved { name: picture.name.clone(), image, place: picture.place });
        }
        details.set(io::slot::REFERENCES, &saved);
        details.set_pictures(images);
    }
}

/// The pictures a file kept, each with the bytes of its image: those whose
/// image is in the file. None for a file from before pictures were kept.
pub fn saved(details: &Details) -> Vec<(Saved, Arc<[u8]>)> {
    let Some(list) = details.get::<Vec<serde_json::Value>>(io::slot::REFERENCES) else { return Vec::new() };
    let images = details.pictures();
    list.iter()
        .filter_map(|entry| io::read_over(&Saved::default(), entry))
        .filter_map(|saved| {
            let bytes = images.get(saved.image)?.clone();
            Some((saved, bytes))
        })
        .collect()
}

/// The pictures a file kept (`details`), ready to stand in a scene, and how
/// many of them could not be read. An image that one of `known` already
/// shows is not read again: crossing an Open with Undo and Redo brings back
/// pictures whose images are decoded already.
pub fn restored(details: &Details, known: &References) -> (Vec<Picture>, usize) {
    let mut pictures: Vec<Picture> = Vec::new();
    let mut unread = 0;
    for (saved, file) in saved(details) {
        // A file keeps an image once however many pictures show it.
        let shared = pictures.iter().find(|p| Arc::ptr_eq(&p.file, &file)).map(|p| (p.file.clone(), p.image.clone()));
        let image = shared.or_else(|| known.same_image(&file)).map(|(_, image)| Ok(image));
        match image.unwrap_or_else(|| decode(&file).map(Arc::new)) {
            Ok(image) => pictures.push(Picture::new(saved.name, file, image, sound(saved.place))),
            Err(_) => unread += 1,
        }
    }
    (pictures, unread)
}

/// A placement read from a file held to numbers that can be drawn: a length
/// that is not a number, or a size of nothing, goes back to the default.
fn sound(mut place: Placement) -> Placement {
    let fallback = Placement::default();
    if !(place.size.is_finite() && place.size > 0.0) {
        place.size = fallback.size;
    }
    for (value, default) in [(&mut place.across, 0.0), (&mut place.up, 0.0), (&mut place.depth, fallback.depth)] {
        if !value.is_finite() {
            *value = default;
        }
    }
    place.opacity = if place.opacity.is_finite() { place.opacity.clamp(0.0, 1.0) } else { 1.0 };
    place
}

/// Reads an image file's bytes into a picture to draw: turned the way the
/// camera was held when it was taken (a phone's photo says so in its EXIF
/// and is stored on its side), and halved, keeping its shape, until no side
/// is longer than [`LARGEST_SIDE`].
pub fn decode(file: &[u8]) -> Result<RgbaImage, String> {
    let unreadable = |e: image::ImageError| format!("The picture could not be read ({e}).");
    let reader = image::ImageReader::new(Cursor::new(file)).with_guessed_format().map_err(|e| format!("The picture could not be read ({e})."))?;
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Err("The picture has no pixels.".into());
    }
    let longest = width.max(height);
    if longest > LARGEST_SIDE {
        let scale = LARGEST_SIDE as f64 / longest as f64;
        let w = ((width as f64 * scale).round() as u32).clamp(1, LARGEST_SIDE);
        let h = ((height as f64 * scale).round() as u32).clamp(1, LARGEST_SIDE);
        image = image.resize_exact(w, h, image::imageops::FilterType::Triangle);
    }
    Ok(image.into_rgba8())
}

/// Reads the image file at `path`: its name for the list, and its bytes to
/// keep (and to [`decode`], unless the scene has the image already).
pub fn read_file(path: &Path) -> Result<(String, Vec<u8>), String> {
    let name = path.file_name().map_or_else(|| "Picture".to_owned(), |n| n.to_string_lossy().into_owned());
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    if !EXTENSIONS.contains(&extension.as_str()) {
        return Err(format!("{name} is not a picture the app reads: a reference picture is a .png or a .jpg."));
    }
    let size = std::fs::metadata(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?.len();
    if size > LARGEST_FILE {
        return Err(format!("{name} is {} MB, too large to keep in a scene. Save a smaller copy of it.", size >> 20));
    }
    let file = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    Ok((name, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::mesh::primitives::{self, Primitive};
    use crate::scene::Scene;
    use glam::Vec2;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.abs_diff_eq(b, 1e-5)
    }

    /// A picture `width` by `height` pixels of one colour, as a PNG file.
    fn png(width: u32, height: u32, colour: [u8; 4]) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, image::Rgba(colour));
        let mut bytes = Vec::new();
        image.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        bytes
    }

    fn picture(width: u32, height: u32, place: Placement) -> Picture {
        let file = png(width, height, [200, 100, 50, 255]);
        let image = decode(&file).unwrap();
        Picture::new("photo.png".into(), file.into(), Arc::new(image), place)
    }

    #[test]
    fn each_facing_has_the_axes_of_the_camera_in_its_view() {
        for facing in Facing::ALL {
            let mut camera = Camera::default();
            (camera.yaw, camera.pitch) = facing.view();
            let view = camera.view(Vec2::ZERO, Vec2::new(800.0, 600.0), camera.distance, 2.0);
            let (right, up, toward) = facing.axes();
            assert!(close(view.right, right), "{facing:?}: right {} {}", view.right, right);
            assert!(close(view.up, up), "{facing:?}: up {} {}", view.up, up);
            assert!(close(-view.forward, toward), "{facing:?}: faces {} {}", -view.forward, toward);
            assert!(close(right.cross(up), toward), "{facing:?}: a right-handed set");
            assert_eq!(Facing::of_view(facing.view().0, facing.view().1), Some(facing));
        }
    }

    #[test]
    fn a_new_picture_faces_the_axis_view_the_camera_is_in_and_no_other() {
        assert_eq!(Facing::of_view(0.2, -0.3), Some(Facing::Front));
        assert_eq!(Facing::of_view(359.8, 0.0), Some(Facing::Front), "a turn all the way round");
        assert_eq!(Facing::of_view(270.0, 0.0), Some(Facing::Right), "-90 by another name");
        assert_eq!(Facing::of_view(-180.0, 0.0), Some(Facing::Back));
        assert_eq!(Facing::of_view(35.0, 90.0), Some(Facing::Top), "seen from above, however it is turned");
        assert_eq!(Facing::of_view(0.0, -90.0), Some(Facing::Bottom));
        assert_eq!(Facing::of_view(30.0, -18.0), None, "a view of the model at an angle");
        assert_eq!(Facing::of_view(0.0, 10.0), None);
    }

    #[test]
    fn a_new_picture_is_as_tall_as_the_scene_centred_on_it_and_just_behind_it() {
        let bounds = Aabb { min: Vec3::new(-0.5, 0.0, -0.3), max: Vec3::new(0.5, 2.0, 0.3) };
        let front = Placement::fitted(Facing::Front, bounds);
        assert!((front.size - 2.0).abs() < 1e-5, "as tall as the figure");
        assert!((front.up - 1.0).abs() < 1e-5 && front.across.abs() < 1e-5, "its middle on the figure's");
        let centre = front.centre();
        assert!(centre.z < -0.3 && centre.z > -0.3 - 0.2, "just behind the figure's back: {centre}");
        assert!((centre.z - (-0.3 - 2.0 * BEHIND)).abs() < 1e-5);
        // From the side it is as tall again, and behind the figure seen from there.
        let right = Placement::fitted(Facing::Right, bounds);
        assert!(right.centre().x < -0.5, "behind, seen from the right: {}", right.centre());
        let left = Placement::fitted(Facing::Left, bounds);
        assert!(left.centre().x > 0.5, "behind, seen from the left: {}", left.centre());
        // From above it is as tall as the figure is deep, and below it.
        let top = Placement::fitted(Facing::Top, bounds);
        assert!((top.size - 0.6).abs() < 1e-5 && top.centre().y < 0.0, "{top:?}");
        assert!(close(Vec3::new(top.centre().x, 0.0, top.centre().z), Vec3::ZERO));
        // An empty scene fits it to a new shape's space round the middle.
        let empty = Placement::fitted(Facing::Front, Aabb::EMPTY);
        assert!(empty.size == NOTHING_SHOWN && empty.centre().z < -1.0, "{empty:?}");
    }

    #[test]
    fn the_corners_follow_the_images_shape_and_go_round_toward_the_view() {
        let place = Placement { size: 2.0, across: 1.0, up: 0.5, depth: 3.0, ..Placement::default() };
        let [bl, br, tr, tl] = place.corners(1.5);
        assert!(close(place.centre(), Vec3::new(1.0, 0.5, -3.0)));
        assert!(close(bl, Vec3::new(-0.5, -0.5, -3.0)) && close(tr, Vec3::new(2.5, 1.5, -3.0)), "{bl} {tr}");
        assert!(((br - bl).length() - 3.0).abs() < 1e-5, "three wide for two tall");
        assert!(((tl - bl).length() - 2.0).abs() < 1e-5);
        // Anticlockwise seen from the view it faces, in every facing.
        for facing in Facing::ALL {
            let [a, b, c, _] = Placement { facing, ..place }.corners(1.5);
            assert!((b - a).cross(c - a).normalize().dot(facing.axes().2) > 0.99, "{facing:?}");
        }
        // Turned to face another view it stays as far off-centre and as far back.
        let side = Placement { facing: Facing::Right, ..place };
        assert!(close(side.centre(), Vec3::new(-3.0, 0.5, -1.0)), "{}", side.centre());
    }

    #[test]
    fn a_flipped_picture_shows_its_image_mirrored_left_to_right() {
        let place = Placement::default();
        assert_eq!(place.image_corners(), [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        let flipped = Placement { flip: true, ..place };
        assert_eq!(flipped.image_corners(), [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]]);
        // The corners themselves stay where they were.
        assert_eq!(flipped.corners(1.0), place.corners(1.0));
    }

    #[test]
    fn a_picture_is_drawn_no_longer_than_the_largest_side_and_keeps_its_shape() {
        let image = decode(&png(LARGEST_SIDE + 904, 2500, [9, 9, 9, 255])).unwrap();
        assert_eq!(image.width(), LARGEST_SIDE);
        assert_eq!(image.height(), (2500.0 * LARGEST_SIDE as f64 / (LARGEST_SIDE + 904) as f64).round() as u32);
        let small = decode(&png(300, 200, [9, 9, 9, 255])).unwrap();
        assert_eq!(small.dimensions(), (300, 200), "a small one as it is");
        assert!(decode(b"not a picture").is_err());
        let picture = picture(300, 200, Placement { size: 2.0, ..Placement::default() });
        assert!((picture.width() - 3.0).abs() < 1e-5);
    }

    #[test]
    fn pictures_are_kept_with_each_image_once_and_come_back_as_they_were() {
        let mut references = References::default();
        let place = Placement { facing: Facing::Left, size: 1.5, depth: 0.7, opacity: 0.4, flip: true, ..Placement::default() };
        let first = picture(30, 20, place);
        let (file, image) = (first.file.clone(), first.image.clone());
        references.add(first);
        // The same photo again, for the other side.
        let again = Picture::new("photo.png".into(), file, image, Placement { facing: Facing::Right, ..place });
        references.add(again);
        references.add(picture(10, 40, Placement { in_front: true, shown: false, ..Placement::default() }));
        let mut details = Details::default();
        references.keep(&mut details);
        assert_eq!(details.pictures().len(), 2, "the photo shown twice is kept once");

        let (back, unread) = restored(&details, &References::default());
        assert_eq!(unread, 0);
        assert_eq!(back.len(), 3);
        for (was, now) in references.pictures.iter().zip(&back) {
            assert_eq!((was.place, &was.name, was.print), (now.place, &now.name, now.print));
            assert_eq!(was.image.dimensions(), now.image.dimensions());
            assert_ne!(was.id, now.id, "a picture from a file has a number of its own");
        }
        assert!(Arc::ptr_eq(&back[0].image, &back[1].image), "and is decoded once");
        // Brought back while the same pictures are up (Undo across an Open), nothing is decoded again.
        let (reused, _) = restored(&details, &references);
        assert!(Arc::ptr_eq(&reused[2].image, &references.pictures[2].image));
        // A scene with none keeps nothing at all.
        let mut empty = Details::default();
        References::default().keep(&mut empty);
        assert!(empty.get::<serde_json::Value>(io::slot::REFERENCES).is_none() && empty.pictures().is_empty());
        assert!(saved(&empty).is_empty());
    }

    #[test]
    fn a_kept_picture_whose_image_is_missing_or_unreadable_is_left_out_and_counted() {
        let mut details = Details::default();
        let entry = |image: usize| Saved { name: format!("{image}.png"), image, place: Placement::default() };
        details.set(io::slot::REFERENCES, &vec![entry(0), entry(1), entry(5)]);
        details.set_pictures(vec![png(4, 4, [1, 2, 3, 255]).into(), Arc::from(&b"damaged"[..])]);
        let (back, unread) = restored(&details, &References::default());
        assert_eq!(back.len(), 1, "the one whose image reads");
        assert_eq!(back[0].name, "0.png");
        assert_eq!(unread, 1, "the damaged image; the one that names no image in the file is not in it at all");
        // Numbers that cannot be drawn are put back to ones that can.
        let bad = Placement { size: f32::NAN, depth: f32::INFINITY, opacity: 7.0, ..Placement::default() };
        let mended = sound(bad);
        assert!(mended.size > 0.0 && mended.depth.is_finite() && mended.opacity == 1.0);
    }

    #[test]
    fn the_fingerprint_moves_with_every_setting_a_file_keeps_and_comes_back_with_it() {
        let print = |r: &References| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            r.hash_kept(&mut hasher);
            hasher.finish()
        };
        let mut references = References::default();
        let none = print(&references);
        let id = references.add(picture(8, 8, Placement::default()));
        let one = print(&references);
        assert_ne!(none, one);
        let edits: [fn(&mut Placement); 9] = [
            |p| p.facing = Facing::Top,
            |p| p.size *= 2.0,
            |p| p.across += 0.1,
            |p| p.up += 0.1,
            |p| p.depth += 0.1,
            |p| p.opacity = 0.5,
            |p| p.flip = true,
            |p| p.in_front = true,
            |p| p.shown = false,
        ];
        for edit in edits {
            let was = references.get(id).unwrap().place;
            edit(&mut references.get_mut(id).unwrap().place);
            assert_ne!(print(&references), one);
            references.get_mut(id).unwrap().place = was;
            assert_eq!(print(&references), one, "put back as it was, it is as it was");
        }
        // The selection is not kept, so it does not count.
        references.selected = None;
        assert_eq!(print(&references), one);
        references.remove(id);
        assert_eq!(print(&references), none);
    }

    #[test]
    fn deleting_a_picture_selects_the_one_that_took_its_place() {
        let mut references = References::default();
        let ids: Vec<u64> = (0..3).map(|_| references.add(picture(2, 2, Placement::default()))).collect();
        assert_eq!(references.selected, Some(ids[2]), "a new picture is selected");
        references.remove(ids[1]);
        assert_eq!(references.selected, Some(ids[2]));
        references.remove(ids[2]);
        assert_eq!(references.selected, Some(ids[0]), "the last one gone: the one before");
        references.remove(ids[0]);
        assert_eq!(references.selected, None);
        assert!(references.selected().is_none());
    }

    #[test]
    fn pictures_are_not_in_the_scenes_bounds_and_brushes_and_picking_go_through_them() {
        let slot = primitives::build(Primitive::Sphere, 2000, 1.0);
        let mut scene = Scene::new("Ball");
        let before = scene.bounds(&slot);
        let ray = (Vec3::new(0.0, 0.0, 10.0), Vec3::NEG_Z);
        let hit = scene.pick(ray.0, ray.1, &slot).expect("the ball is in front of the camera").pos;
        // A big solid picture standing in front of the ball, between it and the eye.
        let place = Placement { size: 6.0, depth: -3.0, ..Placement::default() };
        scene.references.add(picture(16, 16, place));
        assert_eq!(scene.bounds(&slot).min, before.min);
        assert_eq!(scene.bounds(&slot).max, before.max);
        let through = scene.pick(ray.0, ray.1, &slot).expect("the ball is still what the ray meets").pos;
        assert!(close(through, hit), "{through} {hit}");
        assert_eq!(scene.objects.len(), 1, "a picture is not an object");
    }
}
