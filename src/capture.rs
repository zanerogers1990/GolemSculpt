//! Pictures at a size of their own: the offscreen target the viewport is
//! drawn into for a 360 loop or a timelapse, the lens that keeps a tall or
//! square picture from cropping what the window shows, and the writer of a
//! loop's numbered pictures.
//!
//! The window is never resized for any of it. A [`Target`] is a texture of
//! the wanted size plus the renderer's working textures at that size (see
//! `render::Side`), kept apart from the window's so drawing one does not
//! rebuild the other. The drawing itself is in app/recording.rs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::render::{Gpu, Side};

/// A picture size the encoders will take: H.264 refuses odd sizes, and
/// nothing sensible is smaller than 64.
pub fn even_size(width: u32, height: u32) -> (u32, u32) {
    (width.max(64) & !1, height.max(64) & !1)
}

/// The vertical field of view, in degrees, for a picture of `output_aspect`
/// showing what a window of `window_aspect` shows with `fov_y`.
///
/// A picture narrower than the window gets a wider lens, so a square or
/// upright picture never crops away what the viewport had at its sides. A
/// wider one keeps the framing top to bottom, and its extra width is simply
/// more of the background. The orthographic view follows the same number,
/// since its height is worked out from the field of view too.
pub fn widened_fov(fov_y: f32, window_aspect: f32, output_aspect: f32) -> f32 {
    let (window, output) = (window_aspect.max(0.01), output_aspect.max(0.01));
    let widen = if output < window { window / output } else { 1.0 };
    2.0 * ((fov_y.to_radians() * 0.5).tan() * widen).atan().to_degrees()
}

/// Somewhere to draw the viewport at `size`, whatever size the window is.
pub struct Target {
    pub size: (u32, u32),
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// The renderer's working textures at this size.
    pub side: Side,
}

impl Target {
    pub fn new(gpu: &Gpu, size: (u32, u32)) -> Target {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Target { size, texture, view, side: Side::new(gpu) }
    }
}

/// Where recordings go: the Sculpt folder in the user's Videos.
pub fn recordings_folder() -> PathBuf {
    match std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        Some(home) => PathBuf::from(home).join("Videos").join("Sculpt"),
        None => PathBuf::from("Recordings"),
    }
}

/// `prefix` with the date and time after it, as the export names have:
/// "Turntable_2026-10-10_153012".
pub fn stamped(prefix: &str) -> String {
    let name = crate::io::export_file_name(prefix, "x");
    name.strip_suffix(".x").unwrap_or(&name).to_owned()
}

/// `path`, or if something is already there, the same name with " (2)",
/// " (3)"... after it. Two recordings started within the same second would
/// otherwise share a name, and cancelling the second would remove the first.
pub fn unused(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2u32..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){extension}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(path)
}

/// Shows a folder in the file manager, making it first if need be.
pub fn open_folder(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("Could not make {}: {e}", folder.display()))?;
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    std::process::Command::new(program)
        .arg(folder)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open {}: {e}", folder.display()))
}

/// The name of picture `index` of a loop: "frame_0000.png".
pub fn frame_name(index: u32) -> String {
    format!("frame_{index:04}.png")
}

struct Shared {
    /// Pictures handed over and not yet on disk.
    in_flight: AtomicUsize,
    written: AtomicU32,
    cancelled: AtomicBool,
    error: Mutex<Option<String>>,
}

/// Writes a loop's pictures into a folder of their own, compressing them on
/// the worker threads so the window keeps drawing meanwhile.
///
/// Unlike a timelapse, a loop cannot drop a frame: with one missing it would
/// not close. So when the workers fall behind, [`LoopWriter::ready`] says no
/// and the recording waits a frame, with the window still answering.
pub struct LoopWriter {
    folder: PathBuf,
    total: u32,
    issued: u32,
    limit: usize,
    shared: Arc<Shared>,
}

impl LoopWriter {
    /// Makes `folder` and gets ready for `total` pictures of `size`.
    pub fn create(folder: PathBuf, total: u32, size: (u32, u32)) -> Result<LoopWriter, String> {
        std::fs::create_dir_all(&folder).map_err(|e| format!("Could not make {}: {e}", folder.display()))?;
        // Pictures waiting for a worker are whole and uncompressed (33 MB
        // each at 4K), so how many may wait is set by memory.
        let bytes = (size.0 as usize * size.1 as usize * 4).max(1);
        let limit = ((256usize << 20) / bytes).clamp(2, 16).min(rayon::current_num_threads().max(2));
        let shared = Shared {
            in_flight: AtomicUsize::new(0),
            written: AtomicU32::new(0),
            cancelled: AtomicBool::new(false),
            error: Mutex::new(None),
        };
        Ok(LoopWriter { folder, total, issued: 0, limit, shared: Arc::new(shared) })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn total(&self) -> u32 {
        self.total
    }

    /// Pictures handed over so far; the next one is this number.
    pub fn issued(&self) -> u32 {
        self.issued
    }

    /// Pictures safely on disk.
    pub fn written(&self) -> u32 {
        self.shared.written.load(Ordering::Acquire)
    }

    /// Whether the next picture can be taken now.
    pub fn ready(&self) -> bool {
        self.issued < self.total && self.shared.in_flight.load(Ordering::Acquire) < self.limit
    }

    /// Every picture has been handed over and written.
    pub fn complete(&self) -> bool {
        self.issued >= self.total && self.settled()
    }

    /// No worker is still writing.
    pub fn settled(&self) -> bool {
        self.shared.in_flight.load(Ordering::Acquire) == 0
    }

    /// What went wrong, if a picture could not be written.
    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|e| e.clone())
    }

    /// Hands the next picture to a worker.
    pub fn push(&mut self, picture: image::RgbaImage) {
        let path = self.folder.join(frame_name(self.issued));
        self.issued += 1;
        let shared = self.shared.clone();
        shared.in_flight.fetch_add(1, Ordering::AcqRel);
        rayon::spawn(move || {
            if !shared.cancelled.load(Ordering::Acquire) {
                // The picture is opaque; without the unused channel it is a
                // quarter less to compress and to store.
                match image::DynamicImage::ImageRgba8(picture).into_rgb8().save(&path) {
                    Ok(()) => {
                        shared.written.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(e) => {
                        if let Ok(mut error) = shared.error.lock() {
                            error.get_or_insert(format!("Could not write {}: {e}", path.display()));
                        }
                    }
                }
            }
            shared.in_flight.fetch_sub(1, Ordering::AcqRel);
        });
    }

    /// Stops the workers writing anything more. The recording is over; call
    /// [`LoopWriter::discard`] once they have [`settled`](LoopWriter::settled).
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
    }

    /// Removes what was written. Part of a loop is not a loop, and leaving
    /// it behind invites someone to use it as one.
    pub fn discard(self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn horizontal_half_angle(fov_y: f32, aspect: f32) -> f32 {
        ((fov_y.to_radians() * 0.5).tan() * aspect).atan().to_degrees()
    }

    #[test]
    fn an_upright_picture_gets_a_wider_lens() {
        let window = 16.0 / 9.0;
        // 9:16 from a 16:9 window: the lens opens until the picture's sides
        // are where the window's sides were.
        let fov = widened_fov(60.0, window, 9.0 / 16.0);
        assert!(fov > 120.0 && fov < 125.0, "widened to {fov}");
        let (was, now) = (horizontal_half_angle(60.0, window), horizontal_half_angle(fov, 9.0 / 16.0));
        assert!((was - now).abs() < 1e-3, "the sides moved: {was} to {now}");
        // Square: the same rule, less of it.
        let square = widened_fov(60.0, window, 1.0);
        assert!((horizontal_half_angle(square, 1.0) - was).abs() < 1e-3);
        assert!(square > 60.0 && square < fov);
    }

    #[test]
    fn a_wider_picture_keeps_the_lens() {
        assert!((widened_fov(60.0, 1.0, 16.0 / 9.0) - 60.0).abs() < 1e-4);
        assert!((widened_fov(45.0, 16.0 / 9.0, 16.0 / 9.0) - 45.0).abs() < 1e-4);
    }

    #[test]
    fn sizes_are_made_even() {
        assert_eq!(even_size(1921, 1081), (1920, 1080));
        assert_eq!(even_size(1080, 1920), (1080, 1920));
        assert_eq!(even_size(3, 0), (64, 64));
    }

    #[test]
    fn names_carry_the_frame_number_and_the_time() {
        assert_eq!(frame_name(0), "frame_0000.png");
        assert_eq!(frame_name(359), "frame_0359.png");
        let name = stamped("Turntable");
        assert!(name.starts_with("Turntable_") && !name.contains('.'), "{name}");
    }

    #[test]
    fn a_name_already_taken_gets_a_number() {
        let folder = std::env::temp_dir().join(format!("sculpt-unused-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("Turntable_x")).unwrap();
        std::fs::write(folder.join("Timelapse_x.mp4"), b"").unwrap();
        assert_eq!(unused(folder.join("Fresh")), folder.join("Fresh"));
        assert_eq!(unused(folder.join("Turntable_x")), folder.join("Turntable_x (2)"));
        std::fs::create_dir_all(folder.join("Turntable_x (2)")).unwrap();
        assert_eq!(unused(folder.join("Turntable_x")), folder.join("Turntable_x (3)"));
        assert_eq!(unused(folder.join("Timelapse_x.mp4")), folder.join("Timelapse_x (2).mp4"));
        let _ = std::fs::remove_dir_all(&folder);
    }

    fn wait_until(what: impl Fn() -> bool) {
        let start = std::time::Instant::now();
        while !what() {
            assert!(start.elapsed().as_secs() < 30, "the picture writers never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_loop_is_written_as_numbered_pictures() {
        let folder = std::env::temp_dir().join(format!("sculpt-loop-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let mut writer = LoopWriter::create(folder.clone(), 3, (64, 64)).expect("the folder is made");
        for shade in [10u8, 120, 240] {
            assert!(!writer.complete());
            wait_until(|| writer.ready());
            writer.push(image::RgbaImage::from_pixel(64, 64, image::Rgba([shade, shade, shade, 255])));
        }
        assert!(!writer.ready(), "a finished loop takes no more pictures");
        wait_until(|| writer.complete());
        assert_eq!((writer.issued(), writer.written(), writer.error()), (3, 3, None));
        let last = image::open(folder.join(frame_name(2))).expect("the last picture reads back").into_rgb8();
        assert_eq!((last.width(), last.height(), last.get_pixel(5, 5).0), (64, 64, [240, 240, 240]));

        // Cancelled, nothing of it is left behind.
        writer.cancel();
        writer.discard();
        assert!(!folder.exists());
    }
}
