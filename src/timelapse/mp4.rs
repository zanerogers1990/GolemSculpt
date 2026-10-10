//! Writes an H.264 MP4 from raw frames.
//!
//! Windows ships a video encoder with every desktop install: Media
//! Foundation's sink writer and its H.264 encoder. This talks to it directly,
//! so the app needs nothing bundled beside it, as the Unity app's own writer
//! did (`Timelapse/Mp4Writer.cs`).
//!
//! The COM interfaces are called through their vtables by slot number, with
//! the handful of functions and identifiers needed declared here, instead of
//! pulling in the `windows` crate's bindings for the whole of Media
//! Foundation. The slot numbers are the declaration order in mfobjects.h and
//! mfreadwrite.h, which is fixed for good, and are the ones the Unity writer
//! has been using.
//!
//! Everything here belongs to ONE thread, the one that opened the
//! [`Session`]. Media Foundation would allow more, but `write_frame` blocks
//! on purpose when the encoder falls behind, and that must never be the
//! thread the artist is sculpting on (see recorder.rs).

use std::path::Path;

/// A frame rate as a fraction, so 29.97 and 23.976 survive; whole rates come
/// out as n/1.
pub fn rate_fraction(frame_rate: f32) -> (u32, u32) {
    let thousandths = ((frame_rate * 1000.0).round() as u32).max(1);
    if thousandths % 1000 == 0 { (thousandths / 1000, 1) } else { (thousandths, 1000) }
}

/// When frame `frame` starts, in Media Foundation's 100 ns units. Worked out
/// from the frame number each time, never added up, so a rate that does not
/// divide evenly cannot drift.
pub fn frame_time(frame: u64, rate: (u32, u32)) -> i64 {
    (frame as u128 * 10_000_000 * rate.1 as u128 / rate.0 as u128) as i64
}

/// The bit rate for a picture of this size and rate: about 0.2 bits per pixel
/// per frame. A smooth-shaded sculpt on a gradient background shows bands at
/// the rates that are fine for camera footage.
pub fn bit_rate(width: u32, height: u32, frame_rate: f32) -> u32 {
    let wanted = width as f64 * height as f64 * frame_rate as f64 * 0.2;
    wanted.clamp(2_000_000.0, 100_000_000.0) as u32
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::null_mut;

    use super::{frame_time, rate_fraction};

    type Hresult = i32;
    type Object = *mut c_void;

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);

    /// The identifiers that share Media Foundation's base value and differ
    /// only in their first field (a FOURCC or a Direct3D format number).
    const fn media(first: u32) -> Guid {
        Guid(first, 0x0000, 0x0010, [0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71])
    }

    const MF_MEDIA_TYPE_VIDEO: Guid = media(0x7364_6976);
    const MF_VIDEO_FORMAT_H264: Guid = media(0x3436_3248);
    const MF_VIDEO_FORMAT_RGB32: Guid = media(0x0000_0016);
    const MF_MT_MAJOR_TYPE: Guid = Guid(0x48eb_a18e, 0xf8c9, 0x4687, [0xbf, 0x11, 0x0a, 0x74, 0xc9, 0xf9, 0x6a, 0x8f]);
    const MF_MT_SUBTYPE: Guid = Guid(0xf7e3_4c9a, 0x42e8, 0x4714, [0xb7, 0x4b, 0xcb, 0x29, 0xd7, 0x2c, 0x35, 0xe5]);
    const MF_MT_AVG_BITRATE: Guid = Guid(0x2033_2624, 0xfb0d, 0x4d9e, [0xbd, 0x0d, 0xcb, 0xf6, 0x78, 0x6c, 0x10, 0x2e]);
    const MF_MT_INTERLACE_MODE: Guid = Guid(0xe272_4bb8, 0xe676, 0x4806, [0xb4, 0xb2, 0xa8, 0xd6, 0xef, 0xb4, 0x4c, 0xcd]);
    const MF_MT_FRAME_SIZE: Guid = Guid(0x1652_c33d, 0xd6b2, 0x4012, [0xb8, 0x34, 0x72, 0x03, 0x08, 0x49, 0xa3, 0x7d]);
    const MF_MT_FRAME_RATE: Guid = Guid(0xc459_a2e8, 0x3d2c, 0x4e44, [0xb1, 0x32, 0xfe, 0xe5, 0x15, 0x6c, 0x7b, 0xb0]);
    const MF_MT_PIXEL_ASPECT_RATIO: Guid = Guid(0xc637_6a1e, 0x8d0a, 0x4027, [0xbe, 0x45, 0x6d, 0x9a, 0x0a, 0xd3, 0x9b, 0xb6]);
    const MF_MT_DEFAULT_STRIDE: Guid = Guid(0x644b_4e48, 0x1e02, 0x4516, [0xb0, 0xeb, 0xc0, 0x1c, 0xa9, 0xd4, 0x9a, 0xc6]);
    const MF_MT_ALL_SAMPLES_INDEPENDENT: Guid = Guid(0xc917_3739, 0x5e56, 0x461c, [0xb7, 0x13, 0x46, 0xfb, 0x99, 0x5c, 0xb9, 0x5f]);
    const MF_MT_MPEG2_PROFILE: Guid = Guid(0xad76_a80b, 0x2d5c, 0x4e0b, [0xb3, 0x75, 0x64, 0xe5, 0x20, 0x13, 0x70, 0x36]);
    // Encoder settings (codecapi.h), handed to the sink writer as attributes.
    const AV_ENC_MPV_GOP_SIZE: Guid = Guid(0x95f3_1b26, 0x95a4, 0x41aa, [0x93, 0x03, 0x24, 0x6a, 0x7f, 0xc6, 0xee, 0xf1]);
    const AV_ENC_MPV_DEFAULT_B_PICTURE_COUNT: Guid =
        Guid(0x8d39_0aac, 0xdc5c, 0x4200, [0xb5, 0x7f, 0x81, 0x4d, 0x04, 0xba, 0xba, 0xb2]);

    const MF_VERSION: u32 = 0x0002_0070;
    const COINIT_MULTITHREADED: u32 = 0;
    const INTERLACE_PROGRESSIVE: u32 = 2;
    const H264_PROFILE_HIGH: u32 = 100;

    // No import libraries needed: the linker is told which DLL each comes from.
    #[link(name = "ole32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, model: u32) -> Hresult;
        fn CoUninitialize();
    }

    #[link(name = "mfplat", kind = "raw-dylib")]
    unsafe extern "system" {
        fn MFStartup(version: u32, flags: u32) -> Hresult;
        fn MFShutdown() -> Hresult;
        fn MFCreateMediaType(out: *mut Object) -> Hresult;
        fn MFCreateAttributes(out: *mut Object, initial_size: u32) -> Hresult;
        fn MFCreateSample(out: *mut Object) -> Hresult;
        fn MFCreateMemoryBuffer(max_length: u32, out: *mut Object) -> Hresult;
    }

    #[link(name = "mfreadwrite", kind = "raw-dylib")]
    unsafe extern "system" {
        fn MFCreateSinkWriterFromURL(url: *const u16, stream: Object, attributes: Object, out: *mut Object) -> Hresult;
    }

    // IUnknown
    type ReleaseFn = unsafe extern "system" fn(Object) -> u32;
    // IMFAttributes (IMFMediaType and IMFSample both extend it)
    type SetU32Fn = unsafe extern "system" fn(Object, *const Guid, u32) -> Hresult;
    type SetU64Fn = unsafe extern "system" fn(Object, *const Guid, u64) -> Hresult;
    type SetGuidFn = unsafe extern "system" fn(Object, *const Guid, *const Guid) -> Hresult;
    // IMFSinkWriter
    type AddStreamFn = unsafe extern "system" fn(Object, Object, *mut u32) -> Hresult;
    type SetInputFn = unsafe extern "system" fn(Object, u32, Object, Object) -> Hresult;
    type WriteSampleFn = unsafe extern "system" fn(Object, u32, Object) -> Hresult;
    // IMFSample
    type AddBufferFn = unsafe extern "system" fn(Object, Object) -> Hresult;
    type SetTimeFn = unsafe extern "system" fn(Object, i64) -> Hresult;
    // IMFMediaBuffer
    type LockFn = unsafe extern "system" fn(Object, *mut *mut u8, *mut u32, *mut u32) -> Hresult;
    type SetLengthFn = unsafe extern "system" fn(Object, u32) -> Hresult;
    // BeginWriting, Finalize, Unlock: no arguments.
    type PlainFn = unsafe extern "system" fn(Object) -> Hresult;

    const RELEASE: usize = 2;
    const ATTR_SET_U32: usize = 21;
    const ATTR_SET_U64: usize = 22;
    const ATTR_SET_GUID: usize = 24;
    const WRITER_ADD_STREAM: usize = 3;
    const WRITER_SET_INPUT: usize = 4;
    const WRITER_BEGIN: usize = 5;
    const WRITER_WRITE_SAMPLE: usize = 6;
    const WRITER_FINALIZE: usize = 11;
    const SAMPLE_SET_TIME: usize = 36;
    const SAMPLE_SET_DURATION: usize = 38;
    const SAMPLE_ADD_BUFFER: usize = 42;
    const BUFFER_LOCK: usize = 3;
    const BUFFER_UNLOCK: usize = 4;
    const BUFFER_SET_LENGTH: usize = 6;

    /// The function in slot `index` of a COM object's vtable.
    ///
    /// SAFETY: `object` must be a live COM object whose interface has that
    /// slot, and `F` must be the slot's real signature.
    unsafe fn method<F: Copy>(object: Object, index: usize) -> F {
        debug_assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<usize>());
        unsafe {
            let vtable = *(object as *const *const *const c_void);
            std::mem::transmute_copy(&*vtable.add(index))
        }
    }

    fn check(result: Hresult, doing: &str) -> Result<(), String> {
        if result < 0 { Err(format!("Could not {doing} (error 0x{:08X}).", result as u32)) } else { Ok(()) }
    }

    /// A COM object this code holds one reference to.
    struct Held(Object);

    impl Drop for Held {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a live COM object; every one has Release in slot 2.
                unsafe { method::<ReleaseFn>(self.0, RELEASE)(self.0) };
            }
        }
    }

    /// A new, empty media type with `fill` run over it.
    fn media_type(doing: &str, fill: impl Fn(&Held) -> Result<(), String>) -> Result<Held, String> {
        let mut out = null_mut();
        // SAFETY: `out` is a valid place for the new object's pointer.
        check(unsafe { MFCreateMediaType(&mut out) }, doing)?;
        let held = Held(out);
        fill(&held)?;
        Ok(held)
    }

    fn set_guid(attributes: &Held, key: &Guid, value: &Guid) -> Result<(), String> {
        // SAFETY: an IMFAttributes object; both identifiers outlive the call.
        check(unsafe { method::<SetGuidFn>(attributes.0, ATTR_SET_GUID)(attributes.0, key, value) }, "describe the video format")
    }

    fn set_u32(attributes: &Held, key: &Guid, value: u32) -> Result<(), String> {
        // SAFETY: as above.
        check(unsafe { method::<SetU32Fn>(attributes.0, ATTR_SET_U32)(attributes.0, key, value) }, "describe the video format")
    }

    /// Settings for the encoder itself, as the sink writer wants them: an
    /// attribute store of codec properties. A keyframe every `keyframe_every`
    /// frames and no B-frames, which are the two things a loop needs (see
    /// [`Mp4Writer::create_for_loop`]).
    fn loop_encoder_settings(keyframe_every: u32) -> Result<Held, String> {
        let mut out = null_mut();
        // SAFETY: `out` is a valid place for the new object's pointer.
        check(unsafe { MFCreateAttributes(&mut out, 2) }, "describe the video encoder")?;
        let held = Held(out);
        set_u32(&held, &AV_ENC_MPV_GOP_SIZE, keyframe_every)?;
        set_u32(&held, &AV_ENC_MPV_DEFAULT_B_PICTURE_COUNT, 0)?;
        Ok(held)
    }

    /// Sizes and rates are two 32-bit numbers packed into one 64-bit value.
    fn set_pair(attributes: &Held, key: &Guid, high: u32, low: u32) -> Result<(), String> {
        let packed = (high as u64) << 32 | low as u64;
        // SAFETY: as above.
        check(unsafe { method::<SetU64Fn>(attributes.0, ATTR_SET_U64)(attributes.0, key, packed) }, "describe the video format")
    }

    /// Media Foundation, started on this thread. Open one before the first
    /// [`Mp4Writer`] and keep it until the last is finished.
    pub struct Session {
        /// Whether this thread's COM start-up is ours to undo.
        com: bool,
    }

    impl Session {
        pub fn start() -> Result<Session, String> {
            // SAFETY: plain Win32 calls. A failure to initialise COM here
            // means another part of the program already chose this thread's
            // threading model, which leaves COM usable all the same.
            let com = unsafe { CoInitializeEx(null_mut(), COINIT_MULTITHREADED) } >= 0;
            let started = check(
                unsafe { MFStartup(MF_VERSION, 0) },
                "start the Windows video encoder (on Windows N, install the Media Feature Pack)",
            );
            if let Err(e) = started {
                if com {
                    // SAFETY: balances the successful CoInitializeEx above.
                    unsafe { CoUninitialize() };
                }
                return Err(e);
            }
            Ok(Session { com })
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: balances `start`.
            unsafe {
                MFShutdown();
                if self.com {
                    CoUninitialize();
                }
            }
        }
    }

    pub struct Mp4Writer {
        writer: Held,
        stream: u32,
        width: u32,
        height: u32,
        rate: (u32, u32),
        frames: u64,
    }

    impl Mp4Writer {
        pub fn create(path: &Path, width: u32, height: u32, frame_rate: f32, bit_rate: u32) -> Result<Mp4Writer, String> {
            Self::open(path, width, height, frame_rate, bit_rate, None)
        }

        /// Like [`Mp4Writer::create`], for a video that will be played round
        /// and round (a turntable loop).
        ///
        /// The encoder's own choices cost a loop its last frame. It uses
        /// B-frames, which the file records by playing every frame one frame
        /// late with no edit list to take that back out, so the picture starts
        /// a frame in and the header's length ends a frame before the last
        /// picture is done. Here there are none, so frame n is stamped and
        /// shown at n / rate exactly and the frames' own lengths add up to
        /// exactly frames / rate. (The header can still come out one tick
        /// short of that, a tick being 1 / (1000 x rate) second, which is how
        /// the sink writer rounds; the last frame starts well inside it.) And
        /// a keyframe comes every second (the first frame is always one), so
        /// scrubbing and jumping back to the start are quick.
        pub fn create_for_loop(
            path: &Path,
            width: u32,
            height: u32,
            frame_rate: f32,
            bit_rate: u32,
        ) -> Result<Mp4Writer, String> {
            let keyframe_every = (frame_rate.round() as u32).max(1);
            Self::open(path, width, height, frame_rate, bit_rate, Some(keyframe_every))
        }

        fn open(
            path: &Path,
            width: u32,
            height: u32,
            frame_rate: f32,
            bit_rate: u32,
            keyframe_every: Option<u32>,
        ) -> Result<Mp4Writer, String> {
            if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
                return Err("A video needs an even width and height.".into());
            }
            let rate = rate_fraction(frame_rate);
            let url: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            let mut out = null_mut();
            // SAFETY: `url` is NUL-terminated UTF-16 that outlives the call;
            // the two optional arguments are left out.
            check(unsafe { MFCreateSinkWriterFromURL(url.as_ptr(), null_mut(), null_mut(), &mut out) }, "create the video file")?;
            let writer = Held(out);

            let output = media_type("create the video format", |t| {
                set_guid(t, &MF_MT_MAJOR_TYPE, &MF_MEDIA_TYPE_VIDEO)?;
                set_guid(t, &MF_MT_SUBTYPE, &MF_VIDEO_FORMAT_H264)?;
                set_u32(t, &MF_MT_AVG_BITRATE, bit_rate)?;
                set_u32(t, &MF_MT_INTERLACE_MODE, INTERLACE_PROGRESSIVE)?;
                set_u32(t, &MF_MT_MPEG2_PROFILE, H264_PROFILE_HIGH)?;
                set_pair(t, &MF_MT_FRAME_SIZE, width, height)?;
                set_pair(t, &MF_MT_FRAME_RATE, rate.0, rate.1)?;
                set_pair(t, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)
            })?;
            let mut stream = 0;
            // SAFETY: an IMFSinkWriter and an IMFMediaType, both live.
            check(
                unsafe { method::<AddStreamFn>(writer.0, WRITER_ADD_STREAM)(writer.0, output.0, &mut stream) },
                "add the video stream",
            )?;

            let input = media_type("create the frame format", |t| {
                set_guid(t, &MF_MT_MAJOR_TYPE, &MF_MEDIA_TYPE_VIDEO)?;
                set_guid(t, &MF_MT_SUBTYPE, &MF_VIDEO_FORMAT_RGB32)?;
                set_u32(t, &MF_MT_INTERLACE_MODE, INTERLACE_PROGRESSIVE)?;
                // Stated, not left to the default: an RGB format's rows run
                // bottom-up unless a positive stride says the first row is
                // the top one, which is how the frames arrive here.
                set_u32(t, &MF_MT_DEFAULT_STRIDE, width * 4)?;
                set_u32(t, &MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
                set_pair(t, &MF_MT_FRAME_SIZE, width, height)?;
                set_pair(t, &MF_MT_FRAME_RATE, rate.0, rate.1)?;
                set_pair(t, &MF_MT_PIXEL_ASPECT_RATIO, 1, 1)
            })?;
            let tuning = keyframe_every.map(loop_encoder_settings).transpose()?;
            let tuning_object = tuning.as_ref().map_or(null_mut(), |held| held.0);
            // SAFETY: as above; the last argument is an attribute store that
            // `tuning` keeps alive past the call, or null.
            check(
                unsafe { method::<SetInputFn>(writer.0, WRITER_SET_INPUT)(writer.0, stream, input.0, tuning_object) },
                "set up the video encoder (the picture may be too large for it)",
            )?;
            // SAFETY: an IMFSinkWriter with its stream set up.
            check(unsafe { method::<PlainFn>(writer.0, WRITER_BEGIN)(writer.0) }, "start the video")?;
            Ok(Mp4Writer { writer, stream, width, height, rate, frames: 0 })
        }

        /// Appends one frame. `fill` is handed the frame's bytes to write:
        /// `width * height * 4` of them, in B, G, R, A order, top row first.
        ///
        /// Frames carry no times of their own: frame n is stamped n / rate,
        /// back to back. A frame that is never handed over is therefore not
        /// a gap in the video; it simply never existed.
        pub fn write_frame(&mut self, fill: impl FnOnce(&mut [u8])) -> Result<(), String> {
            let size = self.width * self.height * 4;
            let mut out = null_mut();
            // SAFETY: `out` is a valid place for the new object's pointer.
            check(unsafe { MFCreateMemoryBuffer(size, &mut out) }, "make room for a frame")?;
            let buffer = Held(out);
            let mut data = null_mut();
            // SAFETY: an IMFMediaBuffer; the two lengths are not wanted.
            check(
                unsafe { method::<LockFn>(buffer.0, BUFFER_LOCK)(buffer.0, &mut data, null_mut(), null_mut()) },
                "open a frame",
            )?;
            // SAFETY: Lock succeeded, so `data` points at the `size` bytes
            // asked for above, ours alone until Unlock.
            fill(unsafe { std::slice::from_raw_parts_mut(data, size as usize) });
            // SAFETY: an IMFMediaBuffer, locked above.
            unsafe {
                check(method::<PlainFn>(buffer.0, BUFFER_UNLOCK)(buffer.0), "close a frame")?;
                check(method::<SetLengthFn>(buffer.0, BUFFER_SET_LENGTH)(buffer.0, size), "size a frame")?;
            }

            let mut out = null_mut();
            // SAFETY: `out` is a valid place for the new object's pointer.
            check(unsafe { MFCreateSample(&mut out) }, "make a frame")?;
            let sample = Held(out);
            let start = frame_time(self.frames, self.rate);
            let length = frame_time(self.frames + 1, self.rate) - start;
            // SAFETY: an IMFSample, an IMFMediaBuffer and an IMFSinkWriter,
            // all live.
            unsafe {
                check(method::<AddBufferFn>(sample.0, SAMPLE_ADD_BUFFER)(sample.0, buffer.0), "fill a frame")?;
                check(method::<SetTimeFn>(sample.0, SAMPLE_SET_TIME)(sample.0, start), "time a frame")?;
                check(method::<SetTimeFn>(sample.0, SAMPLE_SET_DURATION)(sample.0, length), "time a frame")?;
                check(
                    method::<WriteSampleFn>(self.writer.0, WRITER_WRITE_SAMPLE)(self.writer.0, self.stream, sample.0),
                    "encode a frame (the disk may be full)",
                )?;
            }
            self.frames += 1;
            Ok(())
        }

        pub fn frames(&self) -> u64 {
            self.frames
        }

        /// Finishes the file. Until this returns the MP4 has no index and
        /// will not play.
        pub fn finish(self) -> Result<(), String> {
            // Finishing a video that never got a frame is an error, and
            // there is nothing to finish anyway.
            if self.frames == 0 {
                return Ok(());
            }
            // SAFETY: an IMFSinkWriter that has been written to.
            check(unsafe { method::<PlainFn>(self.writer.0, WRITER_FINALIZE)(self.writer.0) }, "finish the video file")
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    const NEEDS_WINDOWS: &str = "Recording a video needs Windows.";

    pub struct Session;

    impl Session {
        pub fn start() -> Result<Session, String> {
            Err(NEEDS_WINDOWS.into())
        }
    }

    pub struct Mp4Writer;

    impl Mp4Writer {
        pub fn create(_: &Path, _: u32, _: u32, _: f32, _: u32) -> Result<Mp4Writer, String> {
            Err(NEEDS_WINDOWS.into())
        }

        pub fn create_for_loop(_: &Path, _: u32, _: u32, _: f32, _: u32) -> Result<Mp4Writer, String> {
            Err(NEEDS_WINDOWS.into())
        }

        pub fn write_frame(&mut self, _: impl FnOnce(&mut [u8])) -> Result<(), String> {
            Err(NEEDS_WINDOWS.into())
        }

        pub fn frames(&self) -> u64 {
            0
        }

        pub fn finish(self) -> Result<(), String> {
            Err(NEEDS_WINDOWS.into())
        }
    }
}

pub use imp::{Mp4Writer, Session};

/// Whether this build can write a video at all.
pub const SUPPORTED: bool = cfg!(windows);

/// What an MP4 file says about itself, read straight from its boxes: used to
/// check a finished recording without a player.
#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    /// The file starts with a valid `ftyp` box.
    pub ftyp: bool,
    /// Frames in the video track, from its sample table.
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    /// Length in seconds, from the movie header.
    pub seconds: f64,
    /// What the frames' own times add up to, in seconds, from the track's
    /// sample table. The header's length should agree with it.
    pub frames_seconds: f64,
    /// Frames a second: `frames` over `frames_seconds`.
    pub frame_rate: f64,
    /// Frames a player can start from (key frames).
    pub keyframes: u32,
    /// Frames are stored in a different order from the one they are shown in
    /// (B-frames), which the file records as a delay before the first picture.
    pub reordered: bool,
}

/// Reads [`Summary`] out of an MP4 file.
pub fn summary(path: &Path) -> std::io::Result<Summary> {
    let data = std::fs::read(path)?;
    let mut out = Summary { ftyp: data.len() >= 8 && &data[4..8] == b"ftyp", ..Default::default() };
    let word = |at: usize| data.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    // The time scale (ticks a second) and the length in ticks that a `mvhd`
    // or `mdhd` box starting at `body` holds: version 0 keeps its times in 32
    // bits, version 1 in 64.
    let times = |body: usize| -> Option<(u32, u64)> {
        if data.get(body) == Some(&1) {
            let high = word(body + 24)? as u64;
            Some((word(body + 20)?, high << 32 | word(body + 28)? as u64))
        } else {
            Some((word(body + 12)?, word(body + 16)? as u64))
        }
    };
    // The track's own clock, and the ticks its frames add up to.
    let (mut track_scale, mut track_ticks) = (0u32, 0u64);
    // Boxes that only hold other boxes; the rest are read or stepped over.
    const PARENTS: [&[u8; 4]; 5] = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];
    let mut spans = vec![(0usize, data.len())];
    while let Some((mut at, end)) = spans.pop() {
        while at + 8 <= end {
            let Some(size) = word(at) else { break };
            let kind = &data[at + 4..at + 8];
            let (body, size) = match size {
                // The size is in the 64 bits after the name.
                1 => {
                    let (Some(high), Some(low)) = (word(at + 8), word(at + 12)) else { break };
                    (at + 16, ((high as u64) << 32 | low as u64) as usize)
                }
                // The box runs to the end of the file.
                0 => (at + 8, end - at),
                _ => (at + 8, size as usize),
            };
            if size < 8 || at + size > end {
                break;
            }
            if PARENTS.iter().any(|p| p.as_slice() == kind) {
                spans.push((body, at + size));
            } else if kind == b"mvhd" {
                if let Some((scale, length)) = times(body) {
                    out.seconds = length as f64 / scale.max(1) as f64;
                }
            } else if kind == b"mdhd" {
                track_scale = times(body).map_or(0, |(scale, _)| scale);
            } else if kind == b"stts" {
                // Runs of (frames, ticks each), after a version and a count.
                let runs = word(body + 4).unwrap_or(0) as usize;
                for run in 0..runs {
                    let (Some(count), Some(each)) = (word(body + 8 + run * 8), word(body + 12 + run * 8)) else { break };
                    track_ticks += count as u64 * each as u64;
                }
            } else if kind == b"stss" {
                out.keyframes = word(body + 4).unwrap_or(0);
            } else if kind == b"ctts" {
                // Present only when some frame is shown later than it is stored.
                out.reordered = true;
            } else if kind == b"stsz" {
                out.frames = out.frames.max(word(body + 8).unwrap_or(0));
            } else if kind == b"tkhd" {
                // Width and height close the box, as 16.16 fixed point.
                let (w, h) = (word(at + size - 8).unwrap_or(0) >> 16, word(at + size - 4).unwrap_or(0) >> 16);
                if w > 0 && h > 0 {
                    (out.width, out.height) = (w, h);
                }
            }
            at += size;
        }
    }
    if track_scale > 0 && track_ticks > 0 {
        out.frames_seconds = track_ticks as f64 / track_scale as f64;
        out.frame_rate = out.frames as f64 / out.frames_seconds;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_times_are_back_to_back_and_do_not_drift() {
        assert_eq!(rate_fraction(30.0), (30, 1));
        assert_eq!(rate_fraction(29.97), (29970, 1000));
        let rate = rate_fraction(30.0);
        assert_eq!(frame_time(0, rate), 0);
        assert_eq!(frame_time(30, rate), 10_000_000);
        // An hour in, frame n still starts exactly n thirtieths of a second in.
        assert_eq!(frame_time(108_000, rate), 36_000_000_000);
    }

    #[test]
    fn the_rates_a_loop_is_offered_at_are_stamped_without_gaps_or_overlaps() {
        for fps in [24u32, 30, 60] {
            let rate = rate_fraction(fps as f32);
            assert_eq!(rate, (fps, 1));
            // None of them divides ten million, so a frame is a whole number
            // of units or one more; whichever, the next frame starts where
            // this one ends and a whole second is a whole second.
            assert_eq!(frame_time(fps as u64, rate), 10_000_000);
            let nominal = 10_000_000.0 / fps as f64;
            for frame in 0..(fps as u64 * 12) {
                let length = frame_time(frame + 1, rate) - frame_time(frame, rate);
                assert!((length as f64 - nominal).abs() < 1.0, "{fps} fps: frame {frame} is {length} long");
            }
        }
    }

    #[test]
    fn bit_rate_follows_the_picture_and_stays_in_range() {
        assert_eq!(bit_rate(1920, 1080, 30.0), 12_441_600);
        assert_eq!(bit_rate(320, 240, 24.0), 2_000_000);
        assert_eq!(bit_rate(7680, 4320, 120.0), 100_000_000);
    }

    /// A moving gradient, so the encoder has something to compress.
    fn paint(frame: u32, width: u32, height: u32, bytes: &mut [u8]) {
        for (i, px) in bytes.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % width, i as u32 / width);
            px.copy_from_slice(&[(x + frame * 8) as u8, (y * 255 / height) as u8, (frame * 20) as u8, 255]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_few_frames_make_a_playable_file() {
        let path = std::env::temp_dir().join(format!("sculpt-mp4-test-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (width, height, frames) = (320, 240, 12);
        {
            let _session = Session::start().expect("Media Foundation starts");
            let mut writer = Mp4Writer::create(&path, width, height, 30.0, bit_rate(width, height, 30.0)).expect("the file opens");
            for frame in 0..frames {
                writer.write_frame(|bytes| paint(frame, width, height, bytes)).expect("a frame is written");
            }
            assert_eq!(writer.frames(), frames as u64);
            writer.finish().expect("the file is finished");
        }
        let size = std::fs::metadata(&path).expect("the file exists").len();
        let read = summary(&path).expect("the file reads back");
        let _ = std::fs::remove_file(&path);
        assert!(size > 1000, "only {size} bytes were written");
        assert!(read.ftyp, "the file does not start with an ftyp box");
        assert_eq!((read.width, read.height), (width, height));
        assert_eq!(read.frames, frames);
        assert!((read.seconds - frames as f64 / 30.0).abs() < 0.05, "{} s long", read.seconds);
    }

    #[test]
    fn odd_sizes_are_refused_before_anything_is_opened() {
        let path = std::env::temp_dir().join("sculpt-mp4-odd.mp4");
        assert!(Mp4Writer::create(&path, 321, 240, 30.0, 2_000_000).is_err());
        assert!(!path.exists());
    }
}
