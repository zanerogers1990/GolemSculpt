//! Recent files: the sculpts most recently saved or opened, newest first,
//! each with a small picture of the model as it looked when the file was last
//! saved. They show as tiles in the File card; a click opens one.
//!
//! The list rides in the settings file (`prefs.rs`). The pictures sit beside
//! it in a `thumbnails` folder, one PNG per file, named by a hash of the
//! file's path. They are not kept inside the `.sclp` files: the list is about
//! this computer, and a picture in the file would mean opening every file
//! (they can be hundreds of megabytes) to draw six tiles.
//!
//! As in the Unity app, eight are kept and the first six still on disk are
//! shown. A file that has gone is left out of the tiles but stays in the list
//! until newer ones push it off the end, since it may only be on a drive that
//! is unplugged.

use std::path::{Path, PathBuf};

use egui::{Align2, Color32, Rect, Sense, TextureHandle, TextureOptions, Ui, pos2, vec2};

use crate::doc::Action;
use crate::ui::theme;

/// How many files the list keeps.
pub const KEPT: usize = 8;
/// How many tiles are shown.
pub const SHOWN: usize = 6;
/// Size of a picture, in pixels.
pub const PICTURE: (u32, u32) = (320, 200);
/// Seconds between looks at the disk for files that have gone or come back.
const CHECK_EVERY: f64 = 2.0;

struct Tile {
    path: PathBuf,
    /// The file's name without `.sclp`.
    name: String,
    picture: Option<TextureHandle>,
}

#[derive(Default)]
pub struct Recent {
    /// Newest first.
    paths: Vec<PathBuf>,
    /// Where the pictures are kept; `None` keeps none.
    pictures: Option<PathBuf>,
    /// Goes up when the list or a picture changes, so the tiles reload.
    version: u64,
    tiles: Vec<Tile>,
    /// The `version` the tiles were built from, and when the disk was last
    /// looked at.
    built: Option<(u64, f64)>,
}

/// The same file, by the rules of the system it is on: Windows does not tell
/// `Lure.sclp` from `lure.sclp`.
fn same(a: &Path, b: &Path) -> bool {
    if cfg!(windows) { a.as_os_str().eq_ignore_ascii_case(b.as_os_str()) } else { a == b }
}

/// A path as the list keeps it: whole, from the drive down.
fn whole(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

/// File name of the picture for `path`: a hash of the path (FNV-1a), since a
/// path is not itself a usable file name.
fn picture_name(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = if cfg!(windows) { text.to_lowercase() } else { text.into_owned() };
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    format!("{hash:016x}.png")
}

impl Recent {
    /// The list as the settings file had it, and the folder for pictures.
    pub fn restore(&mut self, paths: Vec<PathBuf>, pictures: Option<PathBuf>) {
        self.paths = paths;
        self.paths.truncate(KEPT);
        self.pictures = pictures;
        self.version += 1;
    }

    /// The list, newest first, for the settings file.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Moves `path` to the front of the list, adding it if it is new. The
    /// oldest drops off the end once there are more than `KEPT`, and its
    /// picture goes with it.
    pub fn add(&mut self, path: &Path) {
        let path = whole(path);
        self.paths.retain(|p| !same(p, &path));
        self.paths.insert(0, path);
        while self.paths.len() > KEPT {
            if let Some(dropped) = self.paths.pop() {
                self.forget_picture(&dropped);
            }
        }
        self.version += 1;
    }

    /// Drops a file from the list: it turned out to be gone.
    pub fn remove(&mut self, path: &Path) {
        let before = self.paths.len();
        self.paths.retain(|p| !same(p, path));
        if self.paths.len() != before {
            self.forget_picture(path);
            self.version += 1;
        }
    }

    fn picture_file(&self, path: &Path) -> Option<PathBuf> {
        Some(self.pictures.as_ref()?.join(picture_name(&whole(path))))
    }

    fn forget_picture(&self, path: &Path) {
        if let Some(file) = self.picture_file(path) {
            let _ = std::fs::remove_file(file);
        }
    }

    /// Whether pictures are kept at all in this run.
    pub fn keeps_pictures(&self) -> bool {
        self.pictures.is_some()
    }

    /// Whether `path` has a picture yet. A file opened for the first time on
    /// this computer has none until one is taken.
    pub fn has_picture(&self, path: &Path) -> bool {
        self.picture_file(path).is_some_and(|f| f.is_file())
    }

    /// Keeps `image` as the picture of `path`, scaled down to `PICTURE`.
    /// Failing to is not worth a message: the tile just shows no picture.
    pub fn keep_picture(&mut self, path: &Path, image: &image::RgbaImage) {
        let Some(file) = self.picture_file(path) else { return };
        let small = image::imageops::resize(image, PICTURE.0, PICTURE.1, image::imageops::FilterType::Triangle);
        let saved = file.parent().map_or(Ok(()), std::fs::create_dir_all).map_err(|e| e.to_string()).and_then(|()| {
            image::DynamicImage::ImageRgba8(small).to_rgb8().save(&file).map_err(|e| e.to_string())
        });
        if let Err(e) = saved {
            log::warn!("could not keep a picture of {}: {e}", path.display());
        }
        self.version += 1;
    }

    /// The files to show: the first `SHOWN` of the list that are on disk.
    fn on_disk(&self) -> Vec<&PathBuf> {
        self.paths.iter().filter(|p| p.is_file()).take(SHOWN).collect()
    }

    /// Brings the tiles up to date with the list and the disk. Cheap when
    /// nothing changed: the disk is only looked at every couple of seconds.
    fn refresh(&mut self, ctx: &egui::Context, now: f64) {
        match self.built {
            Some((version, checked)) if version == self.version && now - checked < CHECK_EVERY => return,
            Some((version, _)) if version == self.version => {
                // Only a look at the disk: the same files mean the same tiles.
                let shown = self.on_disk();
                if shown.len() == self.tiles.len() && shown.iter().zip(&self.tiles).all(|(p, t)| **p == t.path) {
                    self.built = Some((version, now));
                    return;
                }
            }
            _ => {}
        }
        let shown: Vec<PathBuf> = self.on_disk().into_iter().cloned().collect();
        self.tiles = shown
            .into_iter()
            .map(|path| {
                let picture = self.picture_file(&path).and_then(|file| {
                    let image = image::open(file).ok()?.to_rgba8();
                    let size = [image.width() as usize, image.height() as usize];
                    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                    Some(ctx.load_texture(format!("recent/{}", picture_name(&path)), pixels, TextureOptions::LINEAR))
                });
                let name = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
                Tile { path, name, picture }
            })
            .collect();
        self.built = Some((self.version, now));
    }
}

/// The Recent block of the File card: a picture tile per file, two to a row.
/// Draws nothing while there is no recent file to show.
pub fn tiles(ui: &mut Ui, recent: &mut Recent, tips: bool, actions: &mut Vec<Action>) {
    let now = ui.input(|i| i.time);
    recent.refresh(ui.ctx(), now);
    if recent.tiles.is_empty() {
        return;
    }
    ui.add_space(2.0);
    theme::heading(ui, "Recent");
    let width = theme::row_width(ui, 2, 6.0);
    let picture_height = ((width - 8.0) * PICTURE.1 as f32 / PICTURE.0 as f32).round();
    let size = vec2(width, picture_height + 8.0 + 16.0);
    for row in recent.tiles.chunks(2) {
        // An odd one out keeps its half width rather than stretching.
        ui.horizontal(|ui| {
            for tile in row {
                let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                let p = ui.painter();
                p.rect_filled(rect, 6.0, theme::CONTROL);
                let frame = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(width - 8.0, picture_height));
                p.rect_filled(frame, 4.0, theme::FIELD);
                match &tile.picture {
                    Some(picture) => {
                        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                        p.image(picture.id(), frame, uv, Color32::WHITE);
                    }
                    None => {
                        p.text(frame.center(), Align2::CENTER_CENTER, "No picture yet", theme::font(10.0), theme::HINT);
                    }
                }
                let caption = Rect::from_min_max(pos2(rect.left() + 4.0, frame.bottom()), pos2(rect.right() - 4.0, rect.bottom()));
                p.with_clip_rect(caption.intersect(p.clip_rect())).text(
                    caption.center(),
                    Align2::CENTER_CENTER,
                    &tile.name,
                    theme::font(10.0),
                    theme::TEXT,
                );
                if response.hovered() {
                    p.rect_filled(rect, 6.0, Color32::from_white_alpha(20));
                }
                let help = format!("Open {}", tile.path.display());
                if theme::tip(response, tips, &help).clicked() {
                    actions.push(Action::OpenRecent(tile.path.clone()));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder under the temp folder, emptied; never the user's settings.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("sculpt_recent_tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn the_newest_is_first_and_a_file_is_listed_once() {
        let dir = folder("order");
        let (a, b, c) = (touch(&dir, "a.sclp"), touch(&dir, "b.sclp"), touch(&dir, "c.sclp"));
        let mut recent = Recent::default();
        for path in [&a, &b, &c, &a] {
            recent.add(path);
        }
        assert_eq!(recent.paths(), [a.clone(), c.clone(), b.clone()]);
        if cfg!(windows) {
            // The same file under another spelling is still that file.
            recent.add(&dir.join("B.SCLP"));
            assert_eq!(recent.paths().len(), 3);
            assert!(same(&recent.paths()[0], &b));
        }
    }

    #[test]
    fn eight_are_kept_six_shown_and_missing_files_left_out() {
        let dir = folder("pruned");
        let files: Vec<PathBuf> = (0..10).map(|i| touch(&dir, &format!("sculpt {i}.sclp"))).collect();
        let mut recent = Recent::default();
        recent.restore(Vec::new(), Some(dir.join("thumbnails")));
        let picture = image::RgbaImage::from_pixel(640, 400, image::Rgba([200, 120, 40, 255]));
        for file in &files {
            recent.add(file);
            recent.keep_picture(file, &picture);
        }
        assert_eq!(recent.paths().len(), KEPT);
        assert_eq!(recent.paths()[0], files[9], "the newest is first");
        assert!(!recent.paths().contains(&files[1]), "the two oldest fell off the end");
        // Their pictures went with them; the others' are there, at tile size.
        assert!(!recent.has_picture(&files[0]) && !recent.has_picture(&files[1]));
        assert!(recent.has_picture(&files[9]));
        let kept = image::open(recent.picture_file(&files[9]).unwrap()).unwrap();
        assert_eq!((kept.width(), kept.height()), PICTURE);
        assert_eq!(std::fs::read_dir(dir.join("thumbnails")).unwrap().count(), KEPT);

        assert_eq!(recent.on_disk().len(), SHOWN);
        // A file that has gone is not shown, and the next one takes its tile.
        std::fs::remove_file(&files[9]).unwrap();
        let shown = recent.on_disk();
        assert_eq!(shown.len(), SHOWN);
        assert_eq!(*shown[0], files[8]);
        assert_eq!(*shown[5], files[3]);
        // It stays in the list until it is found missing on a click.
        assert_eq!(recent.paths().len(), KEPT);
        recent.remove(&files[9]);
        assert_eq!(recent.paths().len(), KEPT - 1);
        assert!(!recent.has_picture(&files[9]));
    }

    #[test]
    fn no_picture_folder_means_no_pictures_and_no_trouble() {
        let dir = folder("nowhere");
        let file = touch(&dir, "a.sclp");
        let mut recent = Recent::default();
        recent.add(&file);
        recent.keep_picture(&file, &image::RgbaImage::new(8, 8));
        assert!(!recent.has_picture(&file));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "nothing was written beside the sculpt");
    }

    #[test]
    fn each_path_has_its_own_picture_name() {
        let a = picture_name(Path::new("C:\\sculpts\\lure.sclp"));
        let b = picture_name(Path::new("C:\\sculpts\\lure2.sclp"));
        assert_ne!(a, b);
        assert!(a.ends_with(".png") && a.len() == 20, "{a}");
        if cfg!(windows) {
            assert_eq!(a, picture_name(Path::new("c:\\Sculpts\\LURE.sclp")));
        }
    }
}
