//! Save Picture: the view as it is, drawn again at a size of its own and
//! handed to the browser as a PNG.
//!
//! The app asks with a [`Request`] (the size in pixels and whether the
//! backdrop is left out). The renderer draws it the next time it draws the
//! viewport, copies the picture into a buffer the processor can read, and
//! puts it in the request's `result` once the graphics card has finished,
//! which is a frame or two later. The app then makes the PNG and hands it
//! over as a download.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The longest side a picture is drawn at. Past this the four samples of
/// smoothing for every pixel take hundreds of megabytes of graphics memory
/// for a moment; WebGL2 allows only 2048 anyway.
pub const MAX_SIDE: u32 = 4096;

/// A picture asked for, and, once it is drawn, the picture.
pub struct Request {
    /// The size asked for, in pixels. It is drawn smaller, the same shape,
    /// when the graphics card cannot make a picture that big.
    pub size: [u32; 2],
    /// Leave the backdrop out: the model on a clear background.
    pub transparent: bool,
    /// The renderer has begun drawing it (it is drawn once).
    started: AtomicBool,
    /// The picture, or why there is none.
    pub result: Mutex<Option<Result<image::RgbaImage, String>>>,
}

impl Request {
    pub fn new(size: [u32; 2], transparent: bool) -> Request {
        Request { size, transparent, started: AtomicBool::new(false), result: Mutex::new(None) }
    }

    /// True the first time it is called: the renderer takes the request up.
    pub(crate) fn begin(&self) -> bool {
        !self.started.swap(true, Ordering::Relaxed)
    }

    pub(crate) fn finish(&self, result: Result<image::RgbaImage, String>) {
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(result);
        }
    }

    /// The picture, or why there is none, once the renderer has it.
    pub fn take(&self) -> Option<Result<image::RgbaImage, String>> {
        self.result.lock().ok()?.take()
    }
}

/// `size` made to fit within `max` pixels a side (and [`MAX_SIDE`]), the
/// same shape.
pub fn fit(size: [u32; 2], max: u32) -> [u32; 2] {
    let max = max.min(MAX_SIDE).max(1) as f32;
    let [w, h] = size.map(|v| v.max(1) as f32);
    let scale = (max / w.max(h)).min(1.0);
    [((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1)]
}

/// The picture as a PNG file. Fast compression: a big picture is made in a
/// moment, at a file somewhat bigger than the smallest possible.
pub fn png(image: &image::RgbaImage) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::Adaptive)
        .write_image(image.as_raw(), image.width(), image.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("The picture could not be made into a PNG: {e}"))?;
    Ok(bytes)
}
