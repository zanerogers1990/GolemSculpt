//! Writing baked maps to files that other programs read.
//!
//! - `<base>_normal.png`: 8-bit RGB, as every engine takes a normal map.
//! - `<base>_displacement.exr`: the distances themselves, 32-bit floating
//!   point, in scene units; the file renderers and Blender want for
//!   displacement, since it needs no range to be read back.
//! - `<base>_displacement.png`: the same squeezed into 16-bit grey for
//!   programs that take no EXR, with `<base>_displacement.txt` beside it
//!   saying what black and white stand for, and the Midlevel and Scale that
//!   undo the squeeze in Blender's Displacement node.
//! - `<base>_ao.png`: 8-bit grey; `<base>_colour.png`: 8-bit RGBA.
//!
//! The EXR is written here rather than by the `image` crate, whose writer
//! stores pictures uncompressed (a single 8192 square would be most of a
//! gigabyte). It is the plain scanline kind with ZIP compression, sixteen
//! rows to a block as OpenEXR has it, deflated with `miniz_oxide`. It holds
//! the distance in R, G and B alike, because a grey-only ("Y") file is not
//! read by every program; the three equal rows deflate to little more than one.

use std::io::Write;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::Baked;

/// Writes every map in `baked` into `folder` (made if it is missing), named
/// from `base_name`, and gives back the files written.
pub fn save(baked: &Baked, folder: &Path, base_name: &str) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("Could not make the folder {}: {e}", folder.display()))?;
    let base = file_safe(base_name);
    let path = |end: &str| folder.join(format!("{base}_{end}"));
    let failed = |path: &Path, e: &dyn std::fmt::Display| format!("Could not write {}: {e}", path.display());
    let mut written = Vec::new();
    let png = |picture: &dyn Fn(&Path) -> image::ImageResult<()>, path: PathBuf, written: &mut Vec<PathBuf>| {
        picture(&path).map_err(|e| failed(&path, &e))?;
        written.push(path);
        Ok::<(), String>(())
    };
    if let Some(normal) = &baked.normal {
        png(&|p| normal.save_with_format(p, image::ImageFormat::Png), path("normal.png"), &mut written)?;
    }
    if let Some((picture, lo, hi)) = &baked.displacement {
        let exr = path("displacement.exr");
        let bytes = exr_bytes(picture.width(), picture.height(), picture.as_raw());
        std::fs::write(&exr, bytes).map_err(|e| failed(&exr, &e))?;
        written.push(exr);

        let (lo, hi) = (*lo, *hi);
        let height = hi - lo;
        let grey: image::ImageBuffer<image::Luma<u16>, Vec<u16>> =
            image::ImageBuffer::from_fn(picture.width(), picture.height(), |x, y| {
                let d = picture.get_pixel(x, y).0[0];
                let share = if height > 0.0 { (d - lo) / height } else { 0.5 };
                image::Luma([(share * 65535.0).round().clamp(0.0, 65535.0) as u16])
            });
        png(&|p| grey.save_with_format(p, image::ImageFormat::Png), path("displacement.png"), &mut written)?;

        let txt = path("displacement.txt");
        std::fs::write(&txt, range_note(&base, lo, hi)).map_err(|e| failed(&txt, &e))?;
        written.push(txt);
    }
    if let Some(ao) = &baked.ao {
        png(&|p| ao.save_with_format(p, image::ImageFormat::Png), path("ao.png"), &mut written)?;
    }
    if let Some(colour) = &baked.colour {
        png(&|p| colour.save_with_format(p, image::ImageFormat::Png), path("colour.png"), &mut written)?;
    }
    Ok(written)
}

/// `name` with what a file name cannot hold put right; "baked" for nothing.
fn file_safe(name: &str) -> String {
    let safe: String =
        name.trim().chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let safe = safe.trim_end_matches(['.', ' ']).to_string();
    if safe.is_empty() { "baked".to_string() } else { safe }
}

/// What the 16-bit displacement picture's greys stand for, in words.
fn range_note(base: &str, lo: f32, hi: f32) -> String {
    let height = hi - lo;
    let mid = if height > 0.0 { -lo / height } else { 0.5 };
    let nl = "\r\n";
    [
        format!("Displacement baked by GolemSculpt for {base}"),
        String::new(),
        format!("{base}_displacement.exr holds the distances themselves, in scene units, along the low model's"),
        "normal: 0 is on its surface, more than 0 stands out from it, less sinks in.".into(),
        "In Blender's Displacement node: Midlevel 0, Scale 1.".into(),
        String::new(),
        format!("{base}_displacement.png holds the same distances squeezed into 16-bit grey:"),
        format!("  black (0)     = {lo:.6} units"),
        format!("  white (65535) = {hi:.6} units"),
        format!("  the low surface itself is {mid:.6} of the way from black to white."),
        format!("In Blender's Displacement node: Midlevel {mid:.6}, Scale {height:.6}."),
        String::new(),
    ]
    .join(nl)
}

/// A whole OpenEXR file of one value a texel (written to R, G and B alike),
/// ZIP-compressed scanlines, rows from the top down.
fn exr_bytes(width: u32, height: u32, values: &[f32]) -> Vec<u8> {
    const ROWS: usize = 16;
    let (w, h) = (width as usize, height as usize);
    let mut out = Vec::new();
    out.extend_from_slice(&[0x76, 0x2f, 0x31, 0x01]); // the magic number
    out.extend_from_slice(&2u32.to_le_bytes()); // version 2: one part, scanlines
    let attribute = |out: &mut Vec<u8>, name: &str, kind: &str, value: &[u8]| {
        for text in [name, kind] {
            out.extend_from_slice(text.as_bytes());
            out.push(0);
        }
        out.extend_from_slice(&(value.len() as i32).to_le_bytes());
        out.extend_from_slice(value);
    };
    let mut channels = Vec::new();
    for name in ["B", "G", "R"] {
        channels.extend_from_slice(name.as_bytes());
        channels.push(0);
        channels.extend_from_slice(&2i32.to_le_bytes()); // FLOAT
        channels.extend_from_slice(&[0, 0, 0, 0]); // linear, and three spare
        channels.extend_from_slice(&1i32.to_le_bytes()); // x sampling
        channels.extend_from_slice(&1i32.to_le_bytes()); // y sampling
    }
    channels.push(0);
    let window: Vec<u8> = [0, 0, width as i32 - 1, height as i32 - 1].iter().flat_map(|v| v.to_le_bytes()).collect();
    attribute(&mut out, "channels", "chlist", &channels);
    attribute(&mut out, "compression", "compression", &[3]); // ZIP, 16 rows a block
    attribute(&mut out, "dataWindow", "box2i", &window);
    attribute(&mut out, "displayWindow", "box2i", &window);
    attribute(&mut out, "lineOrder", "lineOrder", &[0]); // top row first
    attribute(&mut out, "pixelAspectRatio", "float", &1.0f32.to_le_bytes());
    let centre: Vec<u8> = [0.0f32, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
    attribute(&mut out, "screenWindowCenter", "v2f", &centre);
    attribute(&mut out, "screenWindowWidth", "float", &1.0f32.to_le_bytes());
    out.push(0); // end of the header

    let blocks: Vec<Vec<u8>> = (0..h.div_ceil(ROWS))
        .into_par_iter()
        .map(|block| {
            let mut raw = Vec::with_capacity(ROWS * w * 12);
            for y in block * ROWS..((block + 1) * ROWS).min(h) {
                let row = &values[y * w..(y + 1) * w];
                for _channel in 0..3 {
                    for v in row {
                        raw.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
            zip(&raw)
        })
        .collect();
    let table = out.len();
    out.resize(table + blocks.len() * 8, 0);
    for (block, data) in blocks.iter().enumerate() {
        let at = out.len() as u64;
        out[table + block * 8..table + block * 8 + 8].copy_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&((block * ROWS) as i32).to_le_bytes());
        out.extend_from_slice(&(data.len() as i32).to_le_bytes());
        out.write_all(data).expect("writing to memory");
    }
    out
}

/// One block packed as OpenEXR's ZIP compression has it: the bytes split
/// into two halves (every other byte), each byte stored as its difference
/// from the one before, then deflated. A block that would not shrink is
/// stored as it is, which readers know by its length.
fn zip(raw: &[u8]) -> Vec<u8> {
    let half = raw.len().div_ceil(2);
    let mut split = vec![0u8; raw.len()];
    for (i, &b) in raw.iter().enumerate() {
        split[if i % 2 == 0 { i / 2 } else { half + i / 2 }] = b;
    }
    for i in (1..split.len()).rev() {
        split[i] = split[i].wrapping_sub(split[i - 1]).wrapping_add(128);
    }
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&split, 4);
    if packed.len() < raw.len() { packed } else { raw.to_vec() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_loses_what_windows_will_not_take() {
        assert_eq!(file_safe("Head: v2/final?"), "Head_ v2_final_");
        assert_eq!(file_safe("  "), "baked");
        assert_eq!(file_safe("rock."), "rock");
    }

    #[test]
    fn an_exr_written_here_reads_back_with_the_same_numbers() {
        // Uneven size, values that compress and values that do not.
        let (w, h) = (37u32, 41u32);
        let values: Vec<f32> =
            (0..w * h).map(|i| if i % 7 == 0 { (i as f32 * 0.618).sin() * 3.0 } else { -0.25 }).collect();
        let bytes = exr_bytes(w, h, &values);
        let back = image::load_from_memory_with_format(&bytes, image::ImageFormat::OpenExr).expect("reads").to_rgb32f();
        assert_eq!((back.width(), back.height()), (w, h));
        for (i, p) in back.pixels().enumerate() {
            assert_eq!(p.0, [values[i]; 3], "texel {i}");
        }
    }
}
