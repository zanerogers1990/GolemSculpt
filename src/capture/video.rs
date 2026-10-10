//! A 360 loop written straight into an MP4, picture by picture, with none of
//! them ever dropped.
//!
//! The loop is drawn offline, at a size and a frame rate chosen beforehand,
//! so unlike the timelapse (timelapse/recorder.rs) it has nothing to gain by
//! dropping a frame when the encoder falls behind: a loop with one missing
//! does not close. So the pictures go to the encoder through a queue the
//! recording itself keeps short: [`VideoWriter::ready`] says no while the
//! encoder has too many waiting, and the recording waits a frame, the window
//! still answering, until it says yes.
//!
//! Everything to do with Media Foundation happens on one thread of its own
//! (see mp4.rs), and the file is written under a name of its own
//! (`Turntable_x.recording.mp4`), which becomes `Turntable_x.mp4` only when it
//! is finished. A recording that is cancelled, fails, or is cut short by the
//! app closing therefore leaves nothing behind that could be taken for a
//! loop, and even a crash leaves a file whose name says what it is.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use image::RgbaImage;

use crate::timelapse::mp4::{self, Mp4Writer, Session};

/// How much memory pictures waiting for the encoder may take. The encoder is
/// the slow end, so a deeper queue only holds more pictures that are waiting.
const QUEUE_BYTES: usize = 128 << 20;
/// Opening the file and loading the encoder is quick; this long without an
/// answer means it is not going to come.
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// A virus scanner may hold a file it has only just been shown for a moment;
/// renaming or removing it is tried this many times, a tenth of a second apart.
const FILE_TRIES: u32 = 20;

const RUNNING: u8 = 0;
const SAVED: u8 = 1;
const ABANDONED: u8 = 2;

/// What the encoder thread is told.
enum Message {
    Picture(RgbaImage),
    /// Give up, whether or not another picture is coming: without this a
    /// thread waiting for the next one would never hear it was cancelled.
    Stop,
}

/// How many pictures of `size` may wait for the encoder at once.
fn queue_limit(size: (u32, u32)) -> usize {
    let bytes = (size.0 as usize * size.1 as usize * 4).max(1);
    (QUEUE_BYTES / bytes).clamp(2, 8)
}

/// The name a video has while it is being written.
pub fn partial_name(path: &Path) -> PathBuf {
    path.with_extension("recording.mp4")
}

struct Shared {
    /// Pictures handed over and not yet in the file.
    in_flight: AtomicUsize,
    written: AtomicU32,
    cancelled: AtomicBool,
    /// [`RUNNING`] until the encoder thread has either published the file or
    /// removed it.
    outcome: AtomicU8,
    error: Mutex<Option<String>>,
}

/// Writes a loop's pictures into one MP4 file, encoding them on a thread of
/// its own so the window keeps drawing meanwhile.
pub struct VideoWriter {
    path: PathBuf,
    partial: PathBuf,
    total: u32,
    issued: u32,
    limit: usize,
    shared: Arc<Shared>,
    messages: Option<Sender<Message>>,
    thread: Option<JoinHandle<()>>,
}

impl VideoWriter {
    /// Opens the video for `total` pictures of `size` (even, both) at
    /// `frame_rate`, to be called `path` once it is finished. A failure to
    /// start the encoder is reported here, by the button press that caused
    /// it, and not discovered a minute into a recording.
    pub fn create(path: PathBuf, total: u32, size: (u32, u32), frame_rate: u32) -> Result<VideoWriter, String> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|e| format!("Could not make {}: {e}", folder.display()))?;
        }
        let partial = partial_name(&path);
        // Left by a run that was killed; the name is ours alone.
        let _ = std::fs::remove_file(&partial);
        let shared = Arc::new(Shared {
            in_flight: AtomicUsize::new(0),
            written: AtomicU32::new(0),
            cancelled: AtomicBool::new(false),
            outcome: AtomicU8::new(RUNNING),
            error: Mutex::new(None),
        });
        let (messages, inbox) = mpsc::channel();
        let (opened, open_result) = mpsc::channel();
        let job = Job { partial: partial.clone(), path: path.clone(), size, frame_rate, total, shared: shared.clone() };
        let thread = std::thread::Builder::new()
            .name("loop encoder".into())
            .spawn(move || job.run(inbox, opened))
            .map_err(|e| format!("Could not start the video encoder: {e}"))?;
        match open_result.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            // `messages` is dropped on the way out, which makes the thread
            // give up the moment it does get going.
            Err(_) => return Err("The video encoder did not respond.".into()),
        }
        Ok(VideoWriter {
            path,
            partial,
            total,
            issued: 0,
            limit: queue_limit(size),
            shared,
            messages: Some(messages),
            thread: Some(thread),
        })
    }

    /// Where the finished video will be.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn total(&self) -> u32 {
        self.total
    }

    /// Pictures handed over so far; the next one is this number.
    pub fn issued(&self) -> u32 {
        self.issued
    }

    /// Pictures the encoder has taken into the file.
    pub fn written(&self) -> u32 {
        self.shared.written.load(Ordering::Acquire)
    }

    /// Pictures handed over that the encoder has not got to yet.
    pub fn waiting(&self) -> usize {
        self.shared.in_flight.load(Ordering::Acquire)
    }

    /// Whether the next picture can be taken now. It is the only thing that
    /// holds a recording back: a picture is never skipped.
    pub fn ready(&self) -> bool {
        self.issued < self.total
            && self.waiting() < self.limit
            && self.shared.outcome.load(Ordering::Acquire) == RUNNING
    }

    /// Hands the next picture to the encoder. Call it only when
    /// [`VideoWriter::ready`] said so.
    pub fn push(&mut self, picture: RgbaImage) {
        self.issued += 1;
        self.shared.in_flight.fetch_add(1, Ordering::AcqRel);
        let sent = self.messages.as_ref().is_some_and(|messages| messages.send(Message::Picture(picture)).is_ok());
        if !sent {
            // The encoder has stopped; `error` says why.
            self.shared.in_flight.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Every picture is in the video and the file is in place.
    pub fn complete(&self) -> bool {
        self.shared.outcome.load(Ordering::Acquire) == SAVED
    }

    /// The encoder has finished with the file, one way or the other, and
    /// holds nothing open.
    pub fn settled(&self) -> bool {
        self.shared.outcome.load(Ordering::Acquire) != RUNNING || self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// What went wrong, if the video could not be made.
    pub fn error(&self) -> Option<String> {
        let reported = self.shared.error.lock().ok().and_then(|e| e.clone());
        reported.or_else(|| {
            // A thread that is gone without saying so panicked.
            let gone = self.thread.as_ref().is_some_and(|t| t.is_finished());
            (gone && self.shared.outcome.load(Ordering::Acquire) == RUNNING)
                .then(|| "The video encoder stopped unexpectedly.".to_owned())
        })
    }

    /// Tells the encoder to stop and take the file away. The recording is
    /// over; call [`VideoWriter::discard`] once it has
    /// [`settled`](VideoWriter::settled).
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
        if let Some(messages) = &self.messages {
            let _ = messages.send(Message::Stop);
        }
    }

    /// Removes the video, finished or not. Part of a loop is not a loop, and
    /// leaving it behind invites someone to use it as one.
    pub fn discard(mut self) {
        self.stop();
        let _ = std::fs::remove_file(&self.partial);
        let _ = std::fs::remove_file(&self.path);
    }

    /// Stops the encoder if the video is not finished, and lets go of it.
    fn stop(&mut self) {
        if self.shared.outcome.load(Ordering::Acquire) == RUNNING {
            self.cancel();
        }
        // Without its queue the thread stops waiting for pictures.
        self.messages = None;
        // A thread still busy with its last picture takes the file away
        // itself; waiting for it here would hold up closing the app.
        if let Some(thread) = self.thread.take_if(|t| t.is_finished()) {
            let _ = thread.join();
        }
    }
}

impl Drop for VideoWriter {
    /// Reached with the video unfinished only when the app closes underneath
    /// the recording (or it is discarded), and then it is to be removed.
    fn drop(&mut self) {
        self.stop();
    }
}

/// The encoder thread's side.
struct Job {
    partial: PathBuf,
    path: PathBuf,
    size: (u32, u32),
    frame_rate: u32,
    total: u32,
    shared: Arc<Shared>,
}

impl Job {
    fn run(self, inbox: Receiver<Message>, opened: Sender<Result<(), String>>) {
        let Some((session, mut writer)) = self.open(&opened) else { return self.abandon() };
        // Whole means every picture of the loop and not one more: the closing
        // picture would be the opening one again.
        if !self.fill(&mut writer, &inbox) {
            // The session outlives the writer: the writer goes first.
            drop(writer);
            drop(session);
            return self.abandon();
        }
        let finished = writer.finish();
        drop(session);
        match finished.and_then(|()| self.publish()) {
            Ok(()) => self.shared.outcome.store(SAVED, Ordering::Release),
            Err(e) => {
                self.fail(e);
                self.abandon();
            }
        }
    }

    /// Starts the encoder and opens the file, and tells `VideoWriter::create`
    /// how it went. `None` if it did not.
    fn open(&self, opened: &Sender<Result<(), String>>) -> Option<(Session, Mp4Writer)> {
        let (width, height) = self.size;
        let rate = self.frame_rate as f32;
        let both = Session::start().and_then(|session| {
            let bit_rate = mp4::bit_rate(width, height, rate);
            Mp4Writer::create_for_loop(&self.partial, width, height, rate, bit_rate).map(|writer| (session, writer))
        });
        match both {
            Ok(both) => {
                let _ = opened.send(Ok(()));
                Some(both)
            }
            Err(e) => {
                let _ = opened.send(Err(e));
                None
            }
        }
    }

    /// Moves pictures from the queue into the video until the loop is whole.
    /// False if it was cancelled, cut short or failed first.
    fn fill(&self, writer: &mut Mp4Writer, inbox: &Receiver<Message>) -> bool {
        let mut taken = 0u32;
        for message in inbox {
            let Message::Picture(picture) = message else { return false };
            if self.shared.cancelled.load(Ordering::Acquire) {
                return false;
            }
            if let Err(e) = writer.write_frame(|out| bgra_from_rgba(picture.as_raw(), out)) {
                self.fail(e);
                return false;
            }
            self.shared.written.fetch_add(1, Ordering::AcqRel);
            self.shared.in_flight.fetch_sub(1, Ordering::AcqRel);
            taken += 1;
            if taken == self.total {
                return true;
            }
        }
        // The queue was closed: the writer was dropped.
        false
    }

    /// Gives the finished file its real name.
    fn publish(&self) -> Result<(), String> {
        if self.shared.cancelled.load(Ordering::Acquire) {
            return Err("Recording cancelled.".into());
        }
        let mut result = std::fs::rename(&self.partial, &self.path);
        for _ in 1..FILE_TRIES {
            if result.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            result = std::fs::rename(&self.partial, &self.path);
        }
        result.map_err(|e| format!("Could not save {}: {e}", self.path.display()))
    }

    fn fail(&self, why: String) {
        if let Ok(mut error) = self.shared.error.lock() {
            error.get_or_insert(why);
        }
    }

    /// There is no video: takes the unfinished file away.
    fn abandon(&self) {
        for _ in 0..FILE_TRIES {
            match std::fs::remove_file(&self.partial) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => std::thread::sleep(Duration::from_millis(100)),
                _ => break,
            }
        }
        self.shared.outcome.store(ABANDONED, Ordering::Release);
    }
}

/// Media Foundation takes blue first. The pictures are opaque, so alpha is
/// set to full whatever they hold.
fn bgra_from_rgba(from: &[u8], out: &mut [u8]) {
    for (src, dst) in from.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        dst.copy_from_slice(&[src[2], src[1], src[0], 255]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timelapse::mp4::summary;
    use crate::turntable::{FRAME_RATES, LOOP_SIZES};

    #[test]
    fn pictures_are_turned_round_for_the_encoder() {
        let mut out = [0u8; 8];
        bgra_from_rgba(&[1, 2, 3, 0, 10, 20, 30, 7], &mut out);
        assert_eq!(out, [3, 2, 1, 255, 30, 20, 10, 255]);
    }

    #[test]
    fn the_queue_is_sized_by_memory() {
        // 1080p is 8 MB a picture: more than eight waiting is only memory.
        assert_eq!(queue_limit((1920, 1080)), 8);
        // 4K is 33 MB: four.
        assert_eq!(queue_limit((3840, 2160)), 4);
        // However large the picture, one being encoded and one being drawn.
        assert_eq!(queue_limit((16384, 16384)), 2);
        assert_eq!(queue_limit((0, 0)), 8);
    }

    #[test]
    fn a_video_is_written_under_a_name_of_its_own() {
        assert_eq!(partial_name(Path::new("a/Turntable_x.mp4")), Path::new("a/Turntable_x.recording.mp4"));
    }

    /// A folder to write into, empty.
    #[cfg(windows)]
    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("sculpt-loopvideo-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// Picture `index`: a gradient that moves, so the encoder has work to do
    /// and no two pictures are alike.
    #[cfg(windows)]
    fn picture(index: u32, (width, height): (u32, u32)) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x * 255 / width) as u8, (y * 255 / height) as u8, (index * 9) as u8, 255])
        })
    }

    /// Hands `count` pictures over, waiting whenever the writer says to.
    #[cfg(windows)]
    fn push_pictures(writer: &mut VideoWriter, count: u32, size: (u32, u32)) {
        for index in 0..count {
            wait_until(|| writer.ready());
            assert!(writer.waiting() <= writer.limit, "{} waiting", writer.waiting());
            writer.push(picture(index, size));
        }
    }

    #[cfg(windows)]
    fn wait_until(what: impl Fn() -> bool) {
        let start = std::time::Instant::now();
        while !what() {
            assert!(start.elapsed().as_secs() < 60, "the video writer never got there");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[cfg(windows)]
    fn files_in(folder: &Path) -> Vec<String> {
        let names = std::fs::read_dir(folder).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned());
        names.collect()
    }

    /// Whether the movie header's length is `seconds`, to within a tick. The
    /// sink writer keeps time in ticks of 1 / (1000 x fps) second and, for
    /// most frame counts, leaves the header one tick under what the frames
    /// come to (33 microseconds at 30 fps); the frames' own times are exact.
    #[cfg(windows)]
    fn header_agrees(read: &mp4::Summary, seconds: f64, fps: u32) -> bool {
        (read.seconds - seconds).abs() <= 1.0 / (1000.0 * fps as f64) + 1e-9
    }

    /// Records `frames` pictures at `fps` and checks what the file says.
    #[cfg(windows)]
    fn record_and_read(folder: &Path, size: (u32, u32), fps: u32, frames: u32) -> mp4::Summary {
        let path = folder.join("loop.mp4");
        let mut writer = VideoWriter::create(path.clone(), frames, size, fps).expect("the encoder starts");
        push_pictures(&mut writer, frames, size);
        assert!(!writer.ready(), "a finished loop takes no more pictures");
        wait_until(|| writer.complete());
        assert_eq!((writer.issued(), writer.written(), writer.error()), (frames, frames, None));
        assert_eq!(files_in(folder), ["loop.mp4"], "only the finished video is left");
        let read = summary(&path).expect("the video reads back");
        let _ = std::fs::remove_file(&path);
        read
    }

    #[cfg(windows)]
    #[test]
    fn a_loop_becomes_one_video_of_exactly_its_pictures() {
        let folder = scratch("exact");
        let read = record_and_read(&folder, (320, 180), 30, 61);
        assert!(read.ftyp);
        assert_eq!((read.width, read.height, read.frames), (320, 180, 61));
        // 61 frames at 30 a second: the frames come to exactly that, so no
        // frame is missing from the end and nothing is added to it.
        assert!((read.frames_seconds - 61.0 / 30.0).abs() < 1e-9, "frames add up to {}", read.frames_seconds);
        assert!(header_agrees(&read, 61.0 / 30.0, 30), "the header says {}", read.seconds);
        assert!((read.frame_rate - 30.0).abs() < 1e-9, "{} frames a second", read.frame_rate);
        assert!(!read.reordered, "the first picture would start a frame late");
        // The first frame, and one a second after.
        assert_eq!(read.keyframes, 3, "{read:?}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(windows)]
    #[test]
    fn every_shape_and_rate_the_card_offers_makes_a_true_video() {
        let folder = scratch("offered");
        for (label, width, height) in LOOP_SIZES {
            for fps in FRAME_RATES {
                let size = crate::capture::even_size(width, height);
                assert_eq!(size, (width, height), "{label} is already even");
                let frames = 6;
                let read = record_and_read(&folder, size, fps, frames);
                let what = format!("{label} at {fps} fps: {read:?}");
                assert_eq!((read.width, read.height, read.frames), (width, height, frames), "{what}");
                assert!((read.frame_rate - fps as f64).abs() < 1e-9, "{what}");
                let length = frames as f64 / fps as f64;
                assert!((read.frames_seconds - length).abs() < 1e-9, "{what}");
                assert!(header_agrees(&read, length, fps), "{what}");
                assert!(!read.reordered && read.keyframes >= 1, "{what}");
            }
        }
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(windows)]
    #[test]
    fn the_file_is_not_there_until_it_is_whole() {
        let folder = scratch("whole");
        let path = folder.join("loop.mp4");
        let mut writer = VideoWriter::create(path.clone(), 8, (320, 180), 30).expect("the encoder starts");
        push_pictures(&mut writer, 5, (320, 180));
        assert!(!path.exists() && !writer.complete());
        assert_eq!(files_in(&folder), ["loop.recording.mp4"]);
        push_pictures(&mut writer, 3, (320, 180));
        wait_until(|| writer.complete());
        assert_eq!(files_in(&folder), ["loop.mp4"]);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(windows)]
    #[test]
    fn a_cancelled_loop_leaves_nothing() {
        let folder = scratch("cancel");
        let mut writer = VideoWriter::create(folder.join("loop.mp4"), 40, (640, 360), 30).expect("the encoder starts");
        push_pictures(&mut writer, 6, (640, 360));
        writer.cancel();
        wait_until(|| writer.settled());
        assert!(!writer.complete());
        writer.discard();
        assert_eq!(files_in(&folder), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(windows)]
    #[test]
    fn a_loop_cancelled_while_the_encoder_is_idle_leaves_nothing() {
        // The encoder has everything it was given and is waiting for the next
        // picture, which is what happens when the window is slower than it.
        let folder = scratch("idle");
        let mut writer = VideoWriter::create(folder.join("loop.mp4"), 40, (320, 180), 30).expect("the encoder starts");
        push_pictures(&mut writer, 3, (320, 180));
        wait_until(|| writer.written() == 3);
        std::thread::sleep(Duration::from_millis(100));
        writer.cancel();
        wait_until(|| writer.settled());
        writer.discard();
        assert_eq!(files_in(&folder), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(windows)]
    #[test]
    fn closing_the_app_part_way_leaves_nothing() {
        let folder = scratch("close");
        let mut writer = VideoWriter::create(folder.join("loop.mp4"), 40, (640, 360), 30).expect("the encoder starts");
        push_pictures(&mut writer, 6, (640, 360));
        // The app going away takes the writer with it, with no cancel first.
        drop(writer);
        wait_until(|| files_in(&folder).is_empty());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_that_cannot_be_written_to_is_reported_at_the_start() {
        let folder = scratch("blocked");
        // A file where the folder should be.
        std::fs::write(folder.join("sub"), b"").unwrap();
        let blocked = VideoWriter::create(folder.join("sub").join("loop.mp4"), 4, (320, 180), 30);
        assert!(blocked.is_err());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(not(windows))]
    #[test]
    fn without_windows_there_is_no_video() {
        let path = std::env::temp_dir().join("sculpt-loopvideo-none.mp4");
        assert!(VideoWriter::create(path.clone(), 4, (320, 180), 30).is_err());
        assert!(!path.exists());
    }
}
