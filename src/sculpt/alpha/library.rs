//! The alphas there are to pick from: the ones that come with the app, then
//! every picture in the alpha folder (`assets/alphas`, with its subfolders),
//! by file name.
//!
//! One list for the whole program, behind a lock: the panel shows it, the
//! settings file names alphas out of it, and neither is in a hurry. A stroke
//! never comes here; it holds the picture it was given (see the module
//! above). Pictures are read from disk when first asked for, and the tiles
//! in the panel are made from a reading that is not kept, so a folder of
//! hundreds costs their tiles and nothing more.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rayon::prelude::*;

use super::{AlphaImage, patterns};

/// Samples along a side of the alphas that come with the app.
const BUILT_IN_SIZE: usize = 256;
/// The most samples along a side a picture from disk is kept at.
const MAX_SIZE: u32 = 512;
/// Pixels along a side of a tile's picture.
const THUMB_SIDE: usize = 48;

/// A tile's picture: `side` grey bytes a side, rows from the top.
pub struct Thumb {
    pub side: usize,
    pub pixels: Vec<u8>,
}

/// One alpha as the panel lists it.
#[derive(Clone)]
pub struct Listed {
    pub name: String,
    pub thumb: Arc<Thumb>,
}

struct Entry {
    name: String,
    /// The file it is read from, or `None` for one that comes with the app.
    path: Option<PathBuf>,
    image: Option<&'static AlphaImage>,
    thumb: Option<Arc<Thumb>>,
    /// Its file could not be read as a picture; it is left out of the list.
    unreadable: bool,
}

struct Library {
    entries: Vec<Entry>,
    scanned: bool,
}

static LIBRARY: Mutex<Library> = Mutex::new(Library { entries: Vec::new(), scanned: false });

fn library() -> MutexGuard<'static, Library> {
    // A panic elsewhere while the list was held leaves it as usable as ever.
    let mut library = LIBRARY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !library.scanned {
        library.scan();
    }
    library
}

/// Where alphas are kept: `SCULPT_ALPHA_DIR` if it is set (scripted runs),
/// else the `assets/alphas` folder beside the program or, for a `cargo run`,
/// up to two folders above it, or in the folder the program was started
/// from.
fn alpha_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SCULPT_ALPHA_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let exe = std::env::current_exe().ok();
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe.as_deref().and_then(Path::parent) {
        roots.extend(dir.ancestors().take(3).map(Path::to_owned));
    }
    roots.extend(std::env::current_dir().ok());
    roots.into_iter().map(|r| r.join("assets").join("alphas")).find(|p| p.is_dir())
}

/// The folder that is there, else the place beside the program where
/// importing an alpha will make it.
fn alpha_home() -> Option<PathBuf> {
    alpha_dir().or_else(|| Some(std::env::current_exe().ok()?.parent()?.join("assets").join("alphas")))
}

fn is_picture(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg")
}

/// Every picture under `dir`, in a fixed order.
pub(super) fn pictures_in(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if is_picture(&path) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Reads a picture as an alpha called `name`. Colour becomes brightness, and
/// see-through parts count as black. A picture that is not square is
/// centred in a square, black where it does not reach; a large one is
/// shrunk to [`MAX_SIZE`] a side.
pub(super) fn read_picture(path: &Path, name: &str) -> Result<AlphaImage, String> {
    let picture = image::open(path).map_err(|_| "Could not read that picture.".to_owned())?;
    let picture = if picture.width().max(picture.height()) > MAX_SIZE {
        picture.resize(MAX_SIZE, MAX_SIZE, image::imageops::FilterType::Triangle)
    } else {
        picture
    };
    // Sixteen bits: a height read from eight comes out in visible steps.
    let grey = picture.to_luma_alpha16();
    let (width, height) = (grey.width() as usize, grey.height() as usize);
    if width == 0 || height == 0 {
        return Err("That picture is empty.".into());
    }
    let size = width.max(height);
    let (left, top) = ((size - width) / 2, (size - height) / 2);
    let mut samples = vec![0.0f32; size * size];
    for (x, y, pixel) in grey.enumerate_pixels() {
        let [value, cover] = pixel.0;
        samples[(y as usize + top) * size + x as usize + left] = value as f32 / 65535.0 * (cover as f32 / 65535.0);
    }
    Ok(AlphaImage::new(name, size, samples))
}

/// Copies the picture at `source` into `folder` (made if it is not there),
/// under a name nothing there has yet. Returns where it went.
pub(super) fn copy_in(source: &Path, folder: &Path) -> Result<PathBuf, String> {
    if !source.is_file() {
        return Err("File not found.".into());
    }
    if !is_picture(source) {
        return Err("Not a picture this app reads: an alpha must be a .png or .jpg.".into());
    }
    // Read before copying, so a damaged picture is refused rather than left
    // in the folder as an alpha that can never be picked.
    read_picture(source, "")?;
    let name = source.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = source.extension().unwrap_or_default().to_string_lossy().into_owned();
    let mut destination = folder.join(format!("{name}.{ext}"));
    let mut suffix = 2;
    while destination.exists() {
        destination = folder.join(format!("{name} {suffix}.{ext}"));
        suffix += 1;
    }
    std::fs::create_dir_all(folder)
        .and_then(|()| std::fs::copy(source, &destination))
        .map_err(|e| format!("Could not copy into the alpha folder: {e}"))?;
    Ok(destination)
}

/// A name for `stem` that none of `taken` has: the stem itself, or with a
/// number after it. Names are what the settings file keeps, so two pictures
/// called the same in different folders must not share one.
fn free_name(stem: &str, taken: &[Entry]) -> String {
    let used = |name: &str| taken.iter().any(|e| e.name.eq_ignore_ascii_case(name));
    if !used(stem) {
        return stem.to_owned();
    }
    (2..).map(|n| format!("{stem} {n}")).find(|name| !used(name)).expect("some number is free")
}

impl Library {
    /// Lists the alphas afresh. The ones that come with the app keep what
    /// was made of them; pictures from disk are read again when next asked
    /// for, since the file may have changed.
    fn scan(&mut self) {
        let mut built_in: Vec<Entry> = self.entries.drain(..).filter(|e| e.path.is_none()).collect();
        if built_in.is_empty() {
            built_in = patterns::ALL
                .iter()
                .map(|&(name, _)| Entry { name: name.to_owned(), path: None, image: None, thumb: None, unreadable: false })
                .collect();
        }
        self.entries = built_in;
        for path in alpha_dir().map(|dir| pictures_in(&dir)).unwrap_or_default() {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let name = free_name(&stem, &self.entries);
            self.entries.push(Entry { name, path: Some(path), image: None, thumb: None, unreadable: false });
        }
        self.scanned = true;
    }

    /// The picture of entry `i`, read or drawn if it has not been yet.
    fn image(&mut self, i: usize) -> Option<&'static AlphaImage> {
        let entry = &mut self.entries[i];
        if entry.image.is_none() && !entry.unreadable {
            let image = match &entry.path {
                Some(path) => read_picture(path, &entry.name).ok(),
                None => built_in(&entry.name),
            };
            entry.unreadable = image.is_none();
            // Never freed: strokes and settings hold plain references.
            entry.image = image.map(|image| &*Box::leak(Box::new(image)));
        }
        entry.image
    }
}

/// One of the alphas that come with the app, drawn afresh.
pub(super) fn built_in(name: &str) -> Option<AlphaImage> {
    let &(name, shade) = patterns::ALL.iter().find(|(n, _)| *n == name)?;
    Some(AlphaImage::drawn(name, BUILT_IN_SIZE, shade))
}

fn thumb_of(image: &AlphaImage) -> Arc<Thumb> {
    Arc::new(Thumb { side: THUMB_SIDE, pixels: image.thumbnail(THUMB_SIDE) })
}

/// The alpha called `name`, whatever the case of its letters.
pub fn find(name: &str) -> Option<&'static AlphaImage> {
    let mut library = library();
    let i = library.entries.iter().position(|e| e.name.eq_ignore_ascii_case(name.trim()))?;
    library.image(i)
}

/// Every alpha there is to pick, with a tile's picture for each. The first
/// call after a scan reads the pictures on disk to make their tiles, on all
/// cores; pictures that cannot be read are left out.
pub fn listing() -> Vec<Listed> {
    let mut library = library();
    for i in 0..library.entries.len() {
        if library.entries[i].path.is_none() && library.entries[i].thumb.is_none() {
            library.entries[i].thumb = library.image(i).map(thumb_of);
        }
    }
    let made: Vec<(usize, Option<Arc<Thumb>>)> = library
        .entries
        .par_iter()
        .enumerate()
        .filter(|(_, e)| e.thumb.is_none() && !e.unreadable)
        .map(|(i, e)| {
            let thumb = match (e.image, &e.path) {
                (Some(image), _) => Some(thumb_of(image)),
                (None, Some(path)) => read_picture(path, &e.name).ok().map(|image| thumb_of(&image)),
                (None, None) => None,
            };
            (i, thumb)
        })
        .collect();
    for (i, thumb) in made {
        library.entries[i].unreadable = thumb.is_none();
        library.entries[i].thumb = thumb;
    }
    library
        .entries
        .iter()
        .filter_map(|e| Some(Listed { name: e.name.clone(), thumb: e.thumb.clone()? }))
        .collect()
}

/// Looks through the alpha folder again, so pictures dropped into it show
/// up without a restart. Returns a line for the status readout.
pub fn rescan() -> String {
    let mut library = library();
    library.scan();
    let count = library.entries.iter().filter(|e| e.path.is_some()).count();
    match alpha_dir() {
        Some(dir) => format!("{count} alphas of your own. The folder is {}", dir.display()),
        None => "There is no alpha folder yet; Import Alpha makes one.".to_owned(),
    }
}

/// Copies a picture from anywhere on disk into the alpha folder (under
/// "Imported") and returns the name it is listed under. A copy, so the
/// alpha is still there next time even if the original is moved or deleted.
pub fn import(source: &Path) -> Result<String, String> {
    let folder = alpha_home().ok_or("Could not find where to keep alphas.")?.join("Imported");
    let destination = copy_in(source, &folder)?;
    let mut library = library();
    library.scan();
    // (Windows does not tell "Imported" from "imported"; nor does this.)
    let same = |path: &Path| path.as_os_str().eq_ignore_ascii_case(destination.as_os_str());
    let found = library.entries.iter().find(|e| e.path.as_deref().is_some_and(same));
    found.map(|e| e.name.clone()).ok_or_else(|| "Copied the picture but could not find it again.".into())
}
