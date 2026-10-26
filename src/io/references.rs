//! The scene's reference pictures in a `.sclp` file (see src/references.rs
//! for what one is): the `PICT` blocks, and the images they carry through
//! `Details`.
//!
//! Each image is a block of its own, before the `INFO`: the image's number
//! (u32, counting from 0), then the bytes of the file the picture was made
//! from, exactly as they were (a PNG or a JPEG). The file's own bytes rather
//! than the pixels: a photo is several times smaller as its JPEG, the
//! picture stays as sharp as the photo was, and the scene opens with its
//! pictures on a computer that has never seen the photo. Where each picture
//! stands and how it shows is a list in the JSON (`slot::REFERENCES`) that
//! names its image by that number; an image two pictures show (one photo
//! for both sides) is kept once. The number is written, not left to the
//! order of the blocks, so that a block that cannot be read costs its own
//! picture and does not hand the next picture's image to it.
//!
//! A reader that knows nothing of pictures steps over the blocks, as it does
//! any block it does not know, and opens the scene without them.

use std::io::{Read, Write};
use std::sync::Arc;

use super::{Details, read_array, read_u32};

/// The images a file keeps for its reference pictures, by number. Shared,
/// not copied: a save, a recovery copy and the step of an Open that keeps
/// the scene before it all hold the same bytes.
#[derive(Clone, Default)]
pub struct Pictures(Vec<Arc<[u8]>>);

/// Not the bytes themselves, which would fill a log or a failed test with
/// millions of numbers.
impl std::fmt::Debug for Pictures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes: usize = self.0.iter().map(|image| image.len()).sum();
        write!(f, "Pictures({} images, {bytes} bytes)", self.0.len())
    }
}

impl Details {
    /// Keeps these image files for the scene's reference pictures, which
    /// name them by their place in the list.
    pub fn set_pictures(&mut self, images: Vec<Arc<[u8]>>) {
        self.pictures = Pictures(images);
    }

    /// The image files a file kept for its reference pictures, by number.
    pub fn pictures(&self) -> &[Arc<[u8]>] {
        &self.pictures.0
    }
}

/// How long the body of an image's `PICT` block is.
pub(super) fn length(image: &[u8]) -> usize {
    4 + image.len()
}

/// Writes the body of the `PICT` block of image `number`.
pub(super) fn write(w: &mut impl Write, number: u32, image: &[u8]) -> std::io::Result<()> {
    w.write_all(&number.to_le_bytes())?;
    w.write_all(image)
}

/// The body of a `PICT` block, `length` bytes long: the image's number and
/// its file's bytes. The caller has checked that `length` bytes are really
/// in the file, so a damaged length never asks for more memory than the file
/// is big.
pub(super) fn read(r: &mut impl Read, length: u64) -> Option<(u32, Arc<[u8]>)> {
    let number = read_u32(r)?;
    let image = read_array::<u8>(r, length.checked_sub(4)? as usize).ok()?;
    Some((number, image.into()))
}

/// The images of a file's `PICT` blocks, as [`read`] gave them (`None` for
/// a block that did not read), each in its place by number. A writer
/// numbers them from 0 in turn, so a number as high as the count of blocks
/// is damage, and that image is left out; a place no block filled holds an
/// empty image, which the picture naming it cannot show (it is counted as
/// unread, not given another's image).
pub(super) fn placed(read: Vec<Option<(u32, Arc<[u8]>)>>) -> Pictures {
    let count = read.len();
    let mut images: Vec<Arc<[u8]>> = vec![Arc::from(&[][..]); count];
    for (number, image) in read.into_iter().flatten() {
        if (number as usize) < count {
            images[number as usize] = image;
        }
    }
    Pictures(images)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use glam::{Quat, Vec3};

    use super::*;
    use crate::io::{SavedObject, load_scene, save_scene, save_scene_with, slot};
    use crate::mesh::Mesh;
    use crate::mesh::primitives::{self, Primitive};
    use crate::references::{self, Facing, Placement, References};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("sculpt_io_references_tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn object() -> SavedObject {
        SavedObject { name: "Ball".into(), visible: true, mesh: 0, position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }

    fn ball() -> Mesh {
        primitives::build(Primitive::Sphere, 2_000, 0.5)
    }

    /// An image file of one colour, `width` by `height`, as a PNG or a JPEG.
    fn image_file(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([180, 120, 60]));
        let mut bytes = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut bytes), format).unwrap();
        bytes
    }

    /// Two pictures, a JPEG facing the front and a PNG facing the right,
    /// flipped and in front, kept as a save would keep them.
    fn kept() -> (References, Details) {
        let mut scene = References::default();
        for (name, file, place) in [
            ("front.jpg", image_file(40, 60, image::ImageFormat::Jpeg), Placement { size: 1.8, depth: 0.6, ..Placement::default() }),
            (
                "side.png",
                image_file(30, 60, image::ImageFormat::Png),
                Placement { facing: Facing::Right, opacity: 0.5, flip: true, in_front: true, ..Placement::default() },
            ),
        ] {
            let image = references::decode(&file).unwrap();
            scene.add(references::Picture::new(name.into(), file.into(), image.into(), place));
        }
        let mut details = Details::default();
        scene.keep(&mut details);
        (scene, details)
    }

    /// The tags of a scene file's blocks, walked the way every version of
    /// the reader walks them: tag, length, body, to the very end.
    fn tags(path: &Path) -> Vec<[u8; 4]> {
        let bytes = std::fs::read(path).unwrap();
        let mut at = 8;
        let mut tags = Vec::new();
        while at < bytes.len() {
            tags.push(bytes[at..at + 4].try_into().unwrap());
            let length = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
            at += 12 + length;
        }
        assert_eq!(at, bytes.len(), "the blocks add up to the file");
        tags
    }

    #[test]
    fn pictures_go_through_a_saved_file_image_bytes_and_settings_alike() {
        let (scene, details) = kept();
        let path = temp("pictures.sclp");
        save_scene_with(&[&ball()], &[object()], 0, &details, &path).unwrap();
        let loaded = load_scene(&path).unwrap();
        assert_eq!(loaded.details.pictures().len(), 2);
        for (was, now) in scene.pictures.iter().zip(loaded.details.pictures()) {
            assert_eq!(*was.file, **now, "the image file's own bytes, exactly");
        }
        let (back, unread) = references::restored(&loaded.details, &References::default());
        assert_eq!(unread, 0);
        let settings = |pictures: &[references::Picture]| -> Vec<(String, Placement, (u32, u32))> {
            pictures.iter().map(|p| (p.name.clone(), p.place, p.image.dimensions())).collect()
        };
        assert_eq!(settings(&back), settings(&scene.pictures));
        assert_eq!(loaded.objects.len(), 1, "and the rest of the scene with them");
        assert_eq!(loaded.meshes[0].triangle_count(), ball().triangle_count());
    }

    #[test]
    fn the_images_are_blocks_of_their_own_before_the_info_that_any_reader_can_step_over() {
        let (_, details) = kept();
        let path = temp("blocks.sclp");
        save_scene_with(&[&ball()], &[object()], 0, &details, &path).unwrap();
        let tags = tags(&path);
        assert_eq!(&tags[..], &[*b"MESH", *b"PICT", *b"PICT", *b"INFO"]);
        // An older reader knows nothing of PICT: to it the blocks are ones
        // of a later version. The same file with a tag no version knows in
        // their place opens whole, without its pictures.
        let mut bytes = std::fs::read(&path).unwrap();
        let mut at = 8;
        while at < bytes.len() {
            let length = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().unwrap()) as usize;
            if &bytes[at..at + 4] == b"PICT" {
                bytes[at..at + 4].copy_from_slice(b"ZZZZ");
            }
            at += 12 + length;
        }
        let older = temp("older.sclp");
        std::fs::write(&older, &bytes).unwrap();
        let loaded = load_scene(&older).expect("the file opens");
        assert_eq!(loaded.objects[0].name, "Ball");
        assert_eq!(loaded.meshes[0].triangle_count(), ball().triangle_count());
        assert!(loaded.details.pictures().is_empty());
        assert!(references::saved(&loaded.details).is_empty(), "the list names images the file no longer has");
    }

    #[test]
    fn a_file_without_pictures_opens_with_none_and_writes_no_blocks_for_them() {
        let path = temp("none.sclp");
        save_scene(&[&ball()], &[object()], 0, &path).unwrap();
        assert_eq!(tags(&path), [*b"MESH", *b"INFO"]);
        let loaded = load_scene(&path).unwrap();
        assert!(loaded.details.pictures().is_empty());
        assert!(loaded.details.get::<serde_json::Value>(slot::REFERENCES).is_none());
        let (back, unread) = references::restored(&loaded.details, &References::default());
        assert!(back.is_empty() && unread == 0);
    }

    #[test]
    fn a_picture_block_that_does_not_read_costs_that_picture_and_nothing_else() {
        let (_, details) = kept();
        let path = temp("damaged.sclp");
        save_scene_with(&[&ball()], &[object()], 0, &details, &path).unwrap();
        // The first image's number spoilt: it is not taken for an image it
        // is not, and the second picture still has its own.
        let mut bytes = std::fs::read(&path).unwrap();
        let first = (8..bytes.len()).find(|&at| &bytes[at..at + 4] == b"PICT").unwrap();
        bytes[first + 12..first + 16].copy_from_slice(&7u32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let loaded = load_scene(&path).expect("the file opens");
        assert_eq!(loaded.objects.len(), 1);
        let (back, unread) = references::restored(&loaded.details, &References::default());
        assert_eq!(unread, 1, "the front picture, whose image is gone");
        assert_eq!(back.len(), 1);
        assert_eq!((back[0].name.as_str(), back[0].image.dimensions()), ("side.png", (30, 60)));
    }

    #[test]
    fn the_images_say_how_many_there_are_and_not_what_is_in_them() {
        let (_, details) = kept();
        let said = format!("{details:?}");
        assert!(said.contains("Pictures(2 images"), "{said}");
        assert!(said.len() < 2_000, "no bytes in it: {} characters", said.len());
    }
}
