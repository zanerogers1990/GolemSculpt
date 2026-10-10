//! Icons (embedded in the executable) and matcap images (scanned from the
//! `assets/matcaps` folder, with one embedded so the app always has a look).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{ColorImage, TextureHandle, TextureId, TextureOptions};
use rayon::prelude::*;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../../assets/icons/", $name, ".png")) as &[u8])),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "brush_move",
    "brush_clay",
    "brush_smooth",
    "brush_crease",
    "brush_inflate",
    "brush_flatten",
    "brush_pose",
    "brush_standard",
    "brush_layer",
    "brush_snakehook",
    "brush_mask",
    "polarity_add",
    "polarity_sub",
    "undo",
    "redo",
    "prim_sphere",
    "prim_cube",
    "prim_cylinder",
    "prim_capsule",
    "prim_cone",
    "prim_wedge",
    "eye",
    "eye_off",
    "trash",
    "clone",
    "instance",
    "tool_sculpt",
    "tool_sspheres",
    "mirror",
    // For the hosted tools (crates/toolkit): a tool names its icons, and they have to be in this list.
    "tool_transform",
    "tool_measure",
    "tool_tube",
    "tool_lathe",
    "tool_array",
    "tool_keys",
    "bool_union",
    "bool_subtract",
    "bool_intersect",
    "region_box_hide",
    "region_box_mask",
    "region_box_trim",
    "region_box_split",
    "region_lasso_hide",
    "region_lasso_mask",
    "region_lasso_trim",
    "region_lasso_split",
    "region_line_trim",
    "region_line_split",
    "region_invert",
    "region_show_all",
    "check",
    "close",
    "join",
];

const DEFAULT_MATCAP: &[u8] = include_bytes!("../../assets/matcaps/Ben Simonds/MatCap_BS1.jpg");
const DEFAULT_MATCAP_NAME: &str = "MatCap_BS1";

pub struct Matcap {
    pub name: String,
    pub category: String,
    path: Option<PathBuf>,
    pub thumb: TextureHandle,
}

pub struct Assets {
    icons: HashMap<&'static str, TextureHandle>,
    pub matcaps: Vec<Matcap>,
}

/// The matcap the app starts with, decoded from the embedded copy.
pub fn default_matcap() -> image::RgbaImage {
    image::load_from_memory(DEFAULT_MATCAP).expect("embedded matcap decodes").to_rgba8()
}

fn matcap_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(Path::parent);
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(dir) = exe_dir {
        roots.push(dir.to_owned());
        // `cargo run` puts the executable in target/<profile>/.
        roots.extend(dir.ancestors().skip(1).take(2).map(Path::to_owned));
    }
    roots.extend(std::env::current_dir().ok());
    roots.into_iter().map(|r| r.join("assets").join("matcaps")).find(|p| p.is_dir())
}

/// Where matcaps are kept: the folder that is there, else the place beside
/// the program where importing one will make it.
fn matcap_home() -> Option<PathBuf> {
    matcap_dir().or_else(|| Some(std::env::current_exe().ok()?.parent()?.join("assets").join("matcaps")))
}

/// Whether a file is a kind of picture the app can decode as a matcap.
fn is_matcap_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg")
}

/// Copies the picture at `source` into `folder` (made if it is not there),
/// under a name nothing there has yet. Returns where it went, or why it
/// could not be done.
fn copy_matcap(source: &Path, folder: &Path) -> Result<PathBuf, String> {
    if !source.is_file() {
        return Err("File not found.".into());
    }
    if !is_matcap_file(source) {
        return Err("Not a picture this app reads: a matcap must be a .png or .jpg.".into());
    }
    // Decode before copying, so a damaged picture is refused rather than
    // left in the folder as a matcap that can never be picked.
    if image::open(source).is_err() {
        return Err("Could not read that picture.".into());
    }
    let name = source.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = source.extension().unwrap_or_default().to_string_lossy().into_owned();
    // Two different pictures with the same file name must become two
    // matcaps, not one overwriting the other.
    let mut destination = folder.join(format!("{name}.{ext}"));
    let mut suffix = 2;
    while destination.exists() {
        destination = folder.join(format!("{name} {suffix}.{ext}"));
        suffix += 1;
    }
    std::fs::create_dir_all(folder)
        .and_then(|()| std::fs::copy(source, &destination))
        .map_err(|e| format!("Could not copy into the matcap folder: {e}"))?;
    Ok(destination)
}

/// Walks the matcap folder and makes a thumbnail for every picture in it.
/// Pictures that were already in `previous` keep the thumbnail they have.
fn scan_matcaps(ctx: &egui::Context, previous: Vec<Matcap>) -> Vec<Matcap> {
    let mut files: Vec<(String, String, PathBuf)> = Vec::new();
    if let Some(dir) = matcap_dir() {
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !is_matcap_file(&path) {
                    continue;
                }
                let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                let category = if d == dir {
                    "Matcaps".to_owned()
                } else {
                    d.file_name().unwrap_or_default().to_string_lossy().into_owned()
                };
                files.push((category, name, path));
            }
        }
    }
    files.sort();

    let mut kept: HashMap<PathBuf, TextureHandle> =
        previous.into_iter().filter_map(|m| Some((m.path?, m.thumb))).collect();
    // Decoding dozens of images is the slowest part of start-up; spread it
    // over all cores.
    let thumbs: Vec<Option<image::RgbaImage>> = files
        .par_iter()
        .map(|(_, _, path)| {
            if kept.contains_key(path) {
                return None;
            }
            let image = image::open(path).ok()?;
            Some(image.thumbnail(64, 64).to_rgba8())
        })
        .collect();
    let mut matcaps: Vec<Matcap> = files
        .into_iter()
        .zip(thumbs)
        .filter_map(|((category, name, path), thumb)| {
            let thumb = match kept.remove(&path) {
                Some(thumb) => thumb,
                None => texture(ctx, &format!("matcap/{name}"), &thumb?),
            };
            Some(Matcap { name, category, path: Some(path), thumb })
        })
        .collect();
    if !matcaps.iter().any(|m| m.name == DEFAULT_MATCAP_NAME) {
        let image = image::DynamicImage::ImageRgba8(default_matcap()).thumbnail(64, 64).to_rgba8();
        matcaps.insert(
            0,
            Matcap {
                name: DEFAULT_MATCAP_NAME.into(),
                category: "Matcaps".into(),
                path: None,
                thumb: texture(ctx, "matcap/default", &image),
            },
        );
    }
    matcaps
}

fn texture(ctx: &egui::Context, name: &str, image: &image::RgbaImage) -> TextureHandle {
    let size = [image.width() as usize, image.height() as usize];
    ctx.load_texture(name, ColorImage::from_rgba_unmultiplied(size, image.as_raw()), TextureOptions::LINEAR)
}

impl Assets {
    pub fn load(ctx: &egui::Context) -> Self {
        let icons = ICONS
            .iter()
            .filter_map(|&(name, bytes)| {
                let image = image::load_from_memory(bytes).ok()?.to_rgba8();
                Some((name, texture(ctx, name, &image)))
            })
            .collect();

        Assets { icons, matcaps: scan_matcaps(ctx, Vec::new()) }
    }

    /// Looks through the matcap folder again, so pictures dropped into it
    /// show up without a restart. `selected` follows its matcap to wherever
    /// it now is in the list, or goes back to the starting matcap if its
    /// file is gone. Returns a line for the status readout.
    pub fn rescan_matcaps(&mut self, ctx: &egui::Context, selected: &mut usize) -> String {
        let was = self.matcaps.get(*selected).map(|m| (m.path.clone(), m.name.clone()));
        let before = self.matcaps.len();
        self.matcaps = scan_matcaps(ctx, std::mem::take(&mut self.matcaps));
        let now = was.and_then(|(path, name)| self.matcaps.iter().position(|m| m.path == path && m.name == name));
        *selected = now.unwrap_or_else(|| self.default_matcap_index());
        let count = self.matcaps.len();
        let change = match count as i64 - before as i64 {
            0 => String::new(),
            n if n > 0 => format!(", {n} new"),
            n => format!(", {} gone", -n),
        };
        match matcap_dir() {
            Some(dir) => format!("{count} matcaps{change}. The folder is {}", dir.display()),
            None => format!("{count} matcaps{change}. There is no matcap folder yet; Import Matcap makes one."),
        }
    }

    /// Copies a picture from anywhere on disk into the matcap folder (under
    /// "Imported") and selects it. A copy, so the matcap is still there next
    /// time even if the original is moved or deleted. Returns a line for the
    /// status readout, or why it could not be done.
    pub fn import_matcap(&mut self, ctx: &egui::Context, source: &Path, selected: &mut usize) -> Result<String, String> {
        let folder = matcap_home().ok_or("Could not find where to keep matcaps.")?.join("Imported");
        let destination = copy_matcap(source, &folder)?;
        self.matcaps = scan_matcaps(ctx, std::mem::take(&mut self.matcaps));
        // (Windows does not tell "Imported" from "imported"; nor does this.)
        let index = self
            .matcaps
            .iter()
            .position(|m| m.path.as_ref().is_some_and(|p| p.as_os_str().eq_ignore_ascii_case(destination.as_os_str())))
            .ok_or("Copied the picture but could not find it again.")?;
        *selected = index;
        Ok(format!("Imported matcap {}", self.matcaps[index].name))
    }

    pub fn icon(&self, name: &str) -> Option<TextureId> {
        self.icons.get(name).map(TextureHandle::id)
    }

    pub fn default_matcap_index(&self) -> usize {
        self.matcaps.iter().position(|m| m.name == DEFAULT_MATCAP_NAME).unwrap_or(0)
    }

    /// Full-resolution image of a matcap, for the renderer.
    pub fn matcap_image(&self, index: usize) -> Option<image::RgbaImage> {
        let m = self.matcaps.get(index)?;
        match &m.path {
            Some(path) => Some(image::open(path).ok()?.to_rgba8()),
            None => Some(default_matcap()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importing_a_matcap_copies_it_under_a_free_name() {
        let dir = std::env::temp_dir().join(format!("sculpt-matcap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let picture = dir.join("glossy.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 120, 40, 255])).save(&picture).unwrap();
        let folder = dir.join("Imported");

        // The same file name twice gives two matcaps, not one over the other.
        assert_eq!(copy_matcap(&picture, &folder).unwrap(), folder.join("glossy.png"));
        assert_eq!(copy_matcap(&picture, &folder).unwrap(), folder.join("glossy 2.png"));
        assert_eq!(copy_matcap(&picture, &folder).unwrap(), folder.join("glossy 3.png"));

        // What is not a picture, or is one in name only, is refused and
        // nothing of it is left in the folder.
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "not a picture").unwrap();
        assert!(copy_matcap(&notes, &folder).is_err());
        let damaged = dir.join("damaged.jpg");
        std::fs::write(&damaged, "not a picture either").unwrap();
        assert!(copy_matcap(&damaged, &folder).is_err());
        assert!(copy_matcap(&dir.join("missing.png"), &folder).is_err());
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
