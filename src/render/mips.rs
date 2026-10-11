//! Smaller copies of a picture, each half the size of the one before, for the
//! GPU to choose from when the picture is drawn small. The Unity app's
//! matcaps have them; without them a matcap seen through fine detail at a
//! distance sparkles as the view moves, because neighbouring pixels land on
//! unrelated parts of it.
//!
//! The same halving brings a picture too large for the graphics device down
//! to one it will take.

use std::borrow::Cow;

use image::{Rgba, RgbaImage};

/// The size a picture comes to when it is halved until neither side is
/// longer than `limit`: what [`fitted`] returns. Halving, as an HDRI is
/// brought down, because each step is an exact average of four pixels and
/// the shape is kept (to the pixel, where a side is odd). A side never goes
/// below one pixel.
pub fn fitted_size(width: u32, height: u32, limit: u32) -> (u32, u32) {
    let (mut width, mut height) = (width.max(1), height.max(1));
    while width.max(height) > limit.max(1) {
        (width, height) = ((width / 2).max(1), (height / 2).max(1));
    }
    (width, height)
}

/// `image` as the graphics device can take it: as it is when no side is
/// longer than `limit` (the device's own figure for the longest side of a
/// texture), else halved until none is. Asking the device for a larger
/// texture than it allows is an error that stops the app.
pub fn fitted(image: &RgbaImage, limit: u32) -> Cow<'_, RgbaImage> {
    let mut fitted = Cow::Borrowed(image);
    while fitted.width().max(fitted.height()) > limit.max(1) {
        fitted = Cow::Owned(half(&fitted));
    }
    fitted
}

/// Every size below `image`, halving down to a single pixel.
pub fn below(image: &RgbaImage) -> Vec<RgbaImage> {
    let mut levels: Vec<RgbaImage> = Vec::new();
    loop {
        let last = levels.last().unwrap_or(image);
        if last.width() <= 1 && last.height() <= 1 {
            return levels;
        }
        let next = half(last);
        levels.push(next);
    }
}

/// Half the size: each pixel the average of the four under it. The picture
/// is sRGB-encoded, so the average is taken of the light itself and encoded
/// again, which is what the GPU does when it blends between pixels.
fn half(image: &RgbaImage) -> RgbaImage {
    let (width, height) = (image.width(), image.height());
    let decode: Vec<f32> = (0..256).map(|v| to_linear(v as f32 / 255.0)).collect();
    RgbaImage::from_fn((width / 2).max(1), (height / 2).max(1), |x, y| {
        let mut sum = [0.0f32; 4];
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            // (An odd side's last pixel has no partner and stands for both.)
            let p = image.get_pixel((2 * x + dx).min(width - 1), (2 * y + dy).min(height - 1));
            for c in 0..3 {
                sum[c] += decode[p[c] as usize];
            }
            sum[3] += p[3] as f32 / 255.0;
        }
        let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        Rgba([byte(to_srgb(sum[0] / 4.0)), byte(to_srgb(sum[1] / 4.0)), byte(to_srgb(sum[2] / 4.0)), byte(sum[3] / 4.0)])
    })
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_down_to_one_pixel_and_keeps_a_flat_colour() {
        let flat = RgbaImage::from_pixel(40, 24, Rgba([200, 120, 40, 255]));
        let sizes: Vec<(u32, u32)> = below(&flat).iter().map(|l| l.dimensions()).collect();
        assert_eq!(sizes, [(20, 12), (10, 6), (5, 3), (2, 1), (1, 1)]);
        for level in below(&flat) {
            assert!(level.pixels().all(|p| *p == Rgba([200, 120, 40, 255])));
        }
        assert!(below(&RgbaImage::new(1, 1)).is_empty());
    }

    /// At the limit, one over it, far over it, and a picture one pixel wide.
    #[test]
    fn a_picture_is_halved_until_the_device_can_take_it() {
        const LIMIT: u32 = 8192;
        let cases = [
            ((512, 512), (512, 512)),
            ((8192, 8192), (8192, 8192)),
            ((8193, 8192), (4096, 4096)),
            ((8192, 8193), (4096, 4096)),
            ((16384, 16384), (8192, 8192)),
            ((16385, 9000), (8192, 4500)),
            ((1_000_000, 3), (7812, 1)),
            ((1, 20_000), (1, 5000)),
            ((1, 8192), (1, 8192)),
            ((1, 1), (1, 1)),
            ((0, 0), (1, 1)),
        ];
        for ((width, height), fits) in cases {
            assert_eq!(fitted_size(width, height, LIMIT), fits, "{width} x {height}");
            let (w, h) = fitted_size(width, height, LIMIT);
            assert!(w <= LIMIT && h <= LIMIT && w >= 1 && h >= 1);
        }
        // Whatever the device says, down to nothing at all, there is an answer.
        assert_eq!(fitted_size(40, 24, 10), (10, 6));
        assert_eq!(fitted_size(40, 24, 1), (1, 1));
        assert_eq!(fitted_size(40, 24, 0), (1, 1));
        assert_eq!(fitted_size(u32::MAX, u32::MAX, LIMIT), (8191, 8191));
    }

    /// The picture itself comes out the size worked out for it, by the same
    /// averaging the smaller copies are made with, and untouched when it fits.
    #[test]
    fn a_picture_too_large_is_brought_down_and_one_that_fits_is_left_alone() {
        let mut wide = RgbaImage::from_pixel(70, 9, Rgba([200, 120, 40, 255]));
        // Black beside white, in the corner: half the light where they are averaged.
        wide.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        wide.put_pixel(1, 0, Rgba([255, 255, 255, 255]));
        wide.put_pixel(0, 1, Rgba([0, 0, 0, 255]));
        wide.put_pixel(1, 1, Rgba([255, 255, 255, 255]));
        for limit in [70, 69, 35, 34, 16, 2, 1, 0] {
            let small = fitted(&wide, limit);
            assert_eq!(small.dimensions(), fitted_size(70, 9, limit), "limit {limit}");
            assert!(small.width().max(small.height()) <= limit.max(1));
            assert_eq!(matches!(small, Cow::Borrowed(_)), limit >= 70, "limit {limit}");
        }
        let halved = fitted(&wide, 64);
        assert_eq!(halved.dimensions(), (35, 4));
        assert_eq!(*halved.get_pixel(0, 0), Rgba([188, 188, 188, 255]));
        assert_eq!(*halved.get_pixel(20, 2), Rgba([200, 120, 40, 255]));
        // One pixel wide stays one pixel wide.
        let tall = RgbaImage::from_pixel(1, 300, Rgba([9, 9, 9, 255]));
        assert_eq!(fitted(&tall, 100).dimensions(), (1, 75));
    }

    #[test]
    fn averages_light_not_its_encoding() {
        // Black beside white is half the light, which sRGB writes as 188,
        // not the 128 that averaging the two numbers would give.
        let mut stripes = RgbaImage::new(2, 2);
        for (x, _, p) in stripes.enumerate_pixels_mut() {
            *p = if x == 0 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) };
        }
        assert_eq!(*below(&stripes)[0].get_pixel(0, 0), Rgba([188, 188, 188, 255]));
    }
}
