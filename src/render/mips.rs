//! Smaller copies of a picture, each half the size of the one before, for the
//! GPU to choose from when the picture is drawn small. The Unity app's
//! matcaps have them; without them a matcap seen through fine detail at a
//! distance sparkles as the view moves, because neighbouring pixels land on
//! unrelated parts of it.

use image::{Rgba, RgbaImage};

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
