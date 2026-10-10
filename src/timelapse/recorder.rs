//! Gets a timelapse's frames from the graphics card into the video file
//! without the sculpting ever waiting on either.
//!
//! Per recorded frame: the app draws the timelapse camera's view into a
//! texture and hands it to [`Recorder::capture`], which asks for a copy of it
//! and returns at once. A frame or two later the copy is ready; a thread of
//! its own (the "encoder") reads it, compresses it and adds it to the file.
//! Nothing on the main thread waits for the graphics card or for the
//! encoder, which is what keeps recording from costing the brush its speed.
//!
//! Frames in flight are whole uncompressed pictures (8 MB each at 1080p), so
//! there is a fixed pool of buffers for them, sized by memory. When every one
//! is in use the encoder is behind, and the new frame is dropped: losing a
//! frame of a timelapse shortens the video by a thirtieth of a second, where
//! waiting would freeze the brush under the artist's hand.
//!
//! The activity gate needs no trick here. A frame the app did not record is
//! simply never handed over, and the file stamps the frames it does get back
//! to back: idle time is removed, not frozen.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::mp4::{self, Mp4Writer};
use crate::render::Gpu;

/// How much memory frames waiting for the encoder may take.
const POOL_BYTES: u64 = 256 << 20;
/// Opening the file and loading the encoder is quick; this long without an
/// answer means it is not going to come.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// After Stop, how long to wait for frames still on their way back from the
/// graphics card before finishing without them.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// How many frames may be in flight at once for a picture of `frame_bytes`.
pub fn pool_size(frame_bytes: u64) -> usize {
    (POOL_BYTES / frame_bytes.max(1)).clamp(2, 32) as usize
}

enum Message {
    /// Frame `seq`, in `buffer`; `ok` is false if the copy failed.
    Frame { seq: u64, buffer: wgpu::Buffer, ok: bool },
    /// Nothing more is coming: finish the file.
    Finish,
}

struct Shared {
    /// Frames in the file so far.
    written: AtomicU32,
    /// Copies asked for and not yet back from the graphics card.
    pending: AtomicUsize,
    error: Mutex<Option<String>>,
}

/// What came of a recording.
#[derive(Debug)]
pub struct Outcome {
    pub path: PathBuf,
    pub frames: u32,
    pub dropped: u32,
    /// Length of the video.
    pub seconds: f32,
    /// Why there is no video, if there is none. The file is removed then.
    pub error: Option<String>,
}

pub struct Recorder {
    path: PathBuf,
    size: (u32, u32),
    frame_rate: u32,
    /// Bytes per row of a readback buffer; rows are padded as copies need.
    row: u32,
    device: wgpu::Device,
    frames: Sender<Message>,
    /// Buffers the encoder is done with; `None` for one that was lost.
    returned: Receiver<Option<wgpu::Buffer>>,
    free: Vec<wgpu::Buffer>,
    made: usize,
    capacity: usize,
    next: u64,
    dropped: u32,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// When Stop was pressed.
    stopping: Option<Instant>,
    finish_sent: bool,
}

impl Recorder {
    /// Opens `path` and starts the encoder. A failure is reported here, by
    /// the button press that caused it, not discovered a minute into a
    /// session that was never being recorded.
    pub fn start(gpu: &Gpu, path: PathBuf, size: (u32, u32), frame_rate: u32) -> Result<Recorder, String> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| format!("Could not make {}: {e}", folder.display()))?;
        }
        let row = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let shared = Arc::new(Shared { written: AtomicU32::new(0), pending: AtomicUsize::new(0), error: Mutex::new(None) });
        let (frames, inbox) = mpsc::channel();
        let (give_back, returned) = mpsc::channel();
        let (opened, open_result) = mpsc::channel();
        let job = Job {
            path: path.clone(),
            size,
            row,
            frame_rate,
            // Media Foundation takes blue first; a surface that is already
            // that way round needs no swap.
            swap: !matches!(gpu.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb),
            shared: shared.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("timelapse encoder".into())
            .spawn(move || job.run(inbox, give_back, opened))
            .map_err(|e| format!("Could not start the video encoder: {e}"))?;
        match open_result.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("The video encoder did not respond.".into()),
        }
        Ok(Recorder {
            path,
            size,
            frame_rate,
            row,
            device: gpu.device.clone(),
            frames,
            returned,
            free: Vec::new(),
            made: 0,
            capacity: pool_size(row as u64 * size.1 as u64),
            next: 0,
            dropped: 0,
            shared,
            thread: Some(thread),
            stopping: None,
            finish_sent: false,
        })
    }

    /// Frames handed over so far, less the ones dropped.
    pub fn frames_taken(&self) -> u32 {
        self.next as u32
    }

    pub fn frames_written(&self) -> u32 {
        self.shared.written.load(Ordering::Acquire)
    }

    /// The encoder gave up (the disk is full, most likely).
    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|e| e.clone())
    }

    /// Whether copies are still on their way back from the graphics card.
    pub fn pending(&self) -> bool {
        self.shared.pending.load(Ordering::Acquire) > 0
    }

    pub fn stopping(&self) -> bool {
        self.stopping.is_some()
    }

    /// A buffer to copy the next frame into, if one can be had.
    fn take_buffer(&mut self) -> Option<wgpu::Buffer> {
        while let Ok(back) = self.returned.try_recv() {
            match back {
                Some(buffer) => self.free.push(buffer),
                None => self.made = self.made.saturating_sub(1),
            }
        }
        if let Some(buffer) = self.free.pop() {
            return Some(buffer);
        }
        if self.made >= self.capacity {
            return None;
        }
        self.made += 1;
        Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("timelapse frame"),
            size: self.row as u64 * self.size.1 as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }))
    }

    /// Whether a frame drawn now could be taken. When not, the encoder is
    /// behind and the frame should not be drawn at all; it counts as dropped.
    pub fn has_room(&mut self) -> bool {
        if self.stopping.is_some() {
            return false;
        }
        match self.take_buffer() {
            Some(buffer) => {
                self.free.push(buffer);
                true
            }
            None => {
                self.dropped += 1;
                false
            }
        }
    }

    /// Takes the picture in `texture` (already drawn and submitted) as the
    /// next frame of the video. Returns at once; false if it was dropped.
    pub fn capture(&mut self, gpu: &Gpu, texture: &wgpu::Texture) -> bool {
        if self.stopping.is_some() {
            return false;
        }
        let Some(buffer) = self.take_buffer() else {
            self.dropped += 1;
            return false;
        };
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.row),
                    rows_per_image: Some(self.size.1),
                },
            },
            wgpu::Extent3d { width: self.size.0, height: self.size.1, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([encoder.finish()]);

        let seq = self.next;
        self.next += 1;
        self.shared.pending.fetch_add(1, Ordering::AcqRel);
        let (frames, shared, handle) = (self.frames.clone(), self.shared.clone(), buffer.clone());
        // Runs during a later `poll`, on this thread; all it does is pass
        // the buffer on.
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            shared.pending.fetch_sub(1, Ordering::AcqRel);
            let _ = frames.send(Message::Frame { seq, buffer: handle, ok: result.is_ok() });
        });
        true
    }

    /// Call once a frame while recording or saving: lets copies that are
    /// ready be passed to the encoder. Never waits.
    pub fn poll(&mut self) {
        let _ = self.device.poll(wgpu::PollType::Poll);
    }

    /// Stops taking frames and lets what is already in flight drain; keep
    /// calling [`Recorder::finished`] until it gives the outcome. Pressing
    /// Stop therefore never makes the app hitch.
    pub fn stop(&mut self) {
        self.stopping.get_or_insert_with(Instant::now);
    }

    /// The outcome, once the encoder has written the file out after
    /// [`Recorder::stop`]. Never waits.
    pub fn finished(&mut self) -> Option<Outcome> {
        let stopped = self.stopping?;
        self.poll();
        if !self.finish_sent {
            // Frames still on their way back belong in the file.
            if self.pending() && stopped.elapsed() < DRAIN_TIMEOUT {
                return None;
            }
            let _ = self.frames.send(Message::Finish);
            self.finish_sent = true;
        }
        if self.thread.as_ref().is_some_and(|t| !t.is_finished()) {
            return None;
        }
        Some(self.conclude())
    }

    /// Joins the encoder and says what came of the recording.
    fn conclude(&mut self) -> Outcome {
        let panicked = self.thread.take().is_some_and(|t| t.join().is_err());
        let frames = self.frames_written();
        let mut error = self.error();
        if panicked && error.is_none() {
            error = Some("The video encoder stopped unexpectedly.".into());
        }
        if error.is_none() && frames == 0 {
            error = Some("Nothing was recorded: no sculpting happened while recording.".into());
        }
        if error.is_some() {
            // A file with no frames, or one the encoder gave up on, is not a video.
            let _ = std::fs::remove_file(&self.path);
        }
        Outcome {
            path: self.path.clone(),
            frames,
            dropped: self.dropped,
            seconds: frames as f32 / self.frame_rate.max(1) as f32,
            error,
        }
    }
}

impl Drop for Recorder {
    /// Reached with a recording still open only when the app is closing
    /// underneath it. There is no later frame to finish on, so this one
    /// waits: an MP4 that was never finished has no index and does not play.
    fn drop(&mut self) {
        if self.thread.is_none() {
            return;
        }
        if !self.finish_sent {
            let wait = wgpu::PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(2)) };
            let _ = self.device.poll(wait);
            let _ = self.frames.send(Message::Finish);
        }
        self.conclude();
    }
}

/// The encoder thread's side.
struct Job {
    path: PathBuf,
    size: (u32, u32),
    row: u32,
    frame_rate: u32,
    /// The frames arrive red first and must be turned round.
    swap: bool,
    shared: Arc<Shared>,
}

impl Job {
    fn run(self, inbox: Receiver<Message>, give_back: Sender<Option<wgpu::Buffer>>, opened: Sender<Result<(), String>>) {
        let (width, height) = self.size;
        let rate = self.frame_rate as f32;
        let open = || -> Result<(mp4::Session, Mp4Writer), String> {
            let session = mp4::Session::start()?;
            let writer = Mp4Writer::create(&self.path, width, height, rate, mp4::bit_rate(width, height, rate))?;
            Ok((session, writer))
        };
        // The session outlives the writer: declared first, dropped last.
        let (_session, mut writer) = match open() {
            Ok(both) => {
                let _ = opened.send(Ok(()));
                both
            }
            Err(e) => {
                let _ = opened.send(Err(e));
                return;
            }
        };

        // Copies are answered in the order they were asked for in practice,
        // but nothing promises it, and two frames swapped in a video is a
        // visible stutter: frames go into the file strictly by number.
        let mut waiting: BTreeMap<u64, (wgpu::Buffer, bool)> = BTreeMap::new();
        let mut next = 0u64;
        let mut failed = false;
        let mut write = |buffer: wgpu::Buffer, ok: bool, failed: &mut bool| {
            if !ok {
                let _ = give_back.send(None);
                return;
            }
            if !*failed {
                let result = match buffer.get_mapped_range(..) {
                    Ok(view) => writer.write_frame(|out| copy_rows(&view, out, width, self.row, self.swap)),
                    // A copy that cannot be read is a dropped frame, no more.
                    Err(_) => Ok(()),
                };
                if let Err(e) = result {
                    *failed = true;
                    if let Ok(mut error) = self.shared.error.lock() {
                        *error = Some(e);
                    }
                }
                self.shared.written.store(writer.frames() as u32, Ordering::Release);
            }
            buffer.unmap();
            let _ = give_back.send(Some(buffer));
        };
        for message in inbox {
            match message {
                Message::Finish => break,
                Message::Frame { seq, buffer, ok } => {
                    waiting.insert(seq, (buffer, ok));
                }
            }
            while let Some((buffer, ok)) = waiting.remove(&next) {
                next += 1;
                write(buffer, ok, &mut failed);
            }
        }
        // Anything left was waiting on a frame that never came back.
        for (_, (buffer, ok)) in std::mem::take(&mut waiting) {
            write(buffer, ok, &mut failed);
        }
        drop(write);
        if !failed {
            if let Err(e) = writer.finish() {
                if let Ok(mut error) = self.shared.error.lock() {
                    *error = Some(e);
                }
            }
        }
    }
}

/// Copies a picture out of a readback buffer, whose rows are `row` bytes
/// apart, into `out`, whose rows are packed; `swap` exchanges red and blue.
fn copy_rows(from: &[u8], out: &mut [u8], width: u32, row: u32, swap: bool) {
    let line = width as usize * 4;
    for (src, dst) in from.chunks(row as usize).zip(out.chunks_exact_mut(line)) {
        let Some(src) = src.get(..line) else { break };
        if swap {
            for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
                d.copy_from_slice(&[s[2], s[1], s[0], 255]);
            }
        } else {
            dst.copy_from_slice(src);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_is_sized_by_memory() {
        // 1080p: 8 MB a frame, so 32 fit and that is the ceiling.
        assert_eq!(pool_size(1920 * 1080 * 4), 32);
        // 4K: 33 MB a frame, so eight.
        assert_eq!(pool_size(3840 * 2160 * 4), 8);
        // However large the picture, there is room for one being copied and
        // one being encoded.
        assert_eq!(pool_size(1 << 30), 2);
        assert_eq!(pool_size(0), 32);
    }

    #[test]
    fn rows_are_unpadded_and_turned_round() {
        // Two pixels a row, rows eight bytes of picture in sixteen of buffer.
        let from: Vec<u8> = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0, //
            11, 12, 13, 14, 15, 16, 17, 18, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut out = [0u8; 16];
        copy_rows(&from, &mut out, 2, 16, false);
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 13, 14, 15, 16, 17, 18]);
        copy_rows(&from, &mut out, 2, 16, true);
        assert_eq!(out, [3, 2, 1, 255, 7, 6, 5, 255, 13, 12, 11, 255, 17, 16, 15, 255]);
        // A last row cut short of its padding still copies.
        let mut out = [0u8; 16];
        copy_rows(&from[..24], &mut out, 2, 16, false);
        assert_eq!(out[8..], [11, 12, 13, 14, 15, 16, 17, 18]);
    }
}
