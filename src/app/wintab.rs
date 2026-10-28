//! Pen pressure through WinTab, the tablet makers' own interface, for a pen
//! whose pressure never reaches the window.
//!
//! The window reports a pen as touch events carrying pressure (app/pen.rs),
//! which Windows only sends for a pen the tablet driver hands to Windows
//! Ink. With Ink off in the driver (some sculptors turn it off to be rid of
//! its press-and-hold circles and flicks), or with the pen in the driver's
//! mouse mode, the pen arrives as a mouse: moves and clicks, no pressure.
//! The driver still serves pressure the older way, through `Wintab32.dll`,
//! which Blender and Krita read. So does this module.
//!
//! The DLL is loaded by name when the app starts, if it is there at all:
//! nothing links to it, and a machine without a tablet driver starts as
//! before. A context is opened on the window and its packets are drained
//! whenever the pointer moves or presses, and between batches of events.
//! Only the pressure is read from them: the pen's position comes through
//! the window as it always did, so there is nothing to map.
//!
//! Which source wins is settled once per session: the moment a touch event
//! arrives with pressure, Windows Ink is reporting it and the packets are
//! ignored from then on, so a pen never counts twice. Until then, a stroke
//! begun by a mouse press takes the tip's pressure from the latest packet.

use super::Running;

/// The pressure a packet's tip is pressing with, 0 to 1, or `None` with the
/// tip up (the pen hovering, or no pen near the tablet at all). `buttons`
/// is the packet's button state, whose lowest bit is the tip; `raw` the
/// pressure in the device's own units, `range` what the device said its
/// pressure runs over (`AXIS`'s min and max). A tip reported down at a raw
/// pressure of nothing is the lightest touch, which opens a stroke at none:
/// not the blob a mouse's press would be.
pub(super) fn tip_pressure(buttons: u32, raw: u32, range: (i32, i32)) -> Option<f32> {
    let tip_down = buttons & 1 != 0 || raw > 0;
    if !tip_down {
        return None;
    }
    let (min, max) = range;
    let span = (max as f64 - min as f64).max(1.0);
    Some((((raw as f64) - min as f64) / span).clamp(0.0, 1.0) as f32)
}

/// Which source the stroke takes its pressure from: the window's own pen
/// events once any has carried pressure this session (`ink_seen`), and
/// WinTab's `reading` until then. The pen never counts twice.
pub(super) fn fallback(ink_seen: bool, reading: Option<f32>) -> Option<f32> {
    if ink_seen { None } else { reading }
}

/// A WinTab context open on the app's window, or the stub elsewhere.
pub(super) use platform::Tablet;

impl Running {
    /// The tip's pressure as of the latest WinTab packet, for the pointer
    /// event being handled, or `None` for a mouse: the tip is up, Windows
    /// Ink is reporting the pen already, or there is no WinTab. Keeps the
    /// pen readout up to date with it.
    pub(super) fn wintab_reading(&mut self) -> Option<f32> {
        let Some(tablet) = &mut self.wintab else { return None };
        let Some(latest) = tablet.drain() else {
            // No packet since last asked: what the last one said still holds.
            return fallback(self.input.ink_seen, self.input.wintab_tip);
        };
        let reading = fallback(self.input.ink_seen, latest);
        let before = self.input.wintab_tip;
        self.input.wintab_tip = reading;
        let meter = &mut self.doc.sculptor.pen_meter;
        match (before, reading) {
            (_, Some(pressure)) => {
                meter.moved(Some(pressure));
                meter.wintab = true;
                self.redraw = true;
            }
            (Some(_), None) => {
                meter.lifted();
                self.redraw = true;
            }
            (None, None) => {}
        }
        reading
    }

    /// Between batches of events: packets that arrived without a pointer
    /// event to carry them (a press without a move) still reach the readout.
    pub(super) fn wintab_poll(&mut self) {
        if self.wintab.is_some() {
            self.wintab_reading();
        }
    }

    /// The window has the focus again: the context goes back on top, as
    /// Krita's does, so the driver serves it over whatever else was open.
    pub(super) fn wintab_focused(&mut self) {
        if let Some(tablet) = &mut self.wintab {
            tablet.to_front();
        }
    }
}

#[cfg(windows)]
mod platform {
    //! The thin part that talks to `Wintab32.dll`: loads it, opens the
    //! context, pulls packets, closes it. Every call is behind a function
    //! pointer looked up by name, so a DLL that lacks one is as good as none.

    use std::ffi::c_void;
    use std::ptr;

    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use super::tip_pressure;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }

    type Hctx = *mut c_void;
    type WtInfoW = unsafe extern "system" fn(category: u32, index: u32, out: *mut c_void) -> u32;
    type WtOpenW = unsafe extern "system" fn(hwnd: isize, context: *mut LogContext, enable: i32) -> Hctx;
    type WtClose = unsafe extern "system" fn(ctx: Hctx) -> i32;
    type WtPacketsGet = unsafe extern "system" fn(ctx: Hctx, max: i32, out: *mut c_void) -> i32;
    type WtEnable = unsafe extern "system" fn(ctx: Hctx, enable: i32) -> i32;
    type WtOverlap = unsafe extern "system" fn(ctx: Hctx, to_top: i32) -> i32;

    /// The default system context: the one that already moves the cursor
    /// over the whole screen, which is what the app's context is cut from.
    const WTI_DEFSYSCTX: u32 = 4;
    /// The first device's category; its own index is added to it.
    const WTI_DEVICES: u32 = 100;
    const DVC_NAME: u32 = 1;
    const DVC_NPRESSURE: u32 = 15;
    /// The context moves the system cursor.
    const CXO_SYSTEM: u32 = 0x0001;
    /// The context posts a message per packet, which winit ignores; the
    /// packets queue all the same, and are read with `WTPacketsGet`.
    const CXO_MESSAGES: u32 = 0x0004;
    const PK_BUTTONS: u32 = 0x0040;
    const PK_X: u32 = 0x0080;
    const PK_Y: u32 = 0x0100;
    const PK_NORMAL_PRESSURE: u32 = 0x0400;
    /// Packets taken from the queue per call: at most a frame's worth at
    /// the fastest tablets' rates.
    const BATCH: usize = 64;

    /// `LOGCONTEXTW`, as `wintab.h` lays it out: 212 bytes of 32-bit fields
    /// after the 40-character name.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LogContext {
        name: [u16; 40],
        options: u32,
        status: u32,
        locks: u32,
        msg_base: u32,
        device: u32,
        pkt_rate: u32,
        pkt_data: u32,
        pkt_mode: u32,
        move_mask: u32,
        btn_dn_mask: u32,
        btn_up_mask: u32,
        in_org: [i32; 3],
        in_ext: [i32; 3],
        out_org: [i32; 3],
        out_ext: [i32; 3],
        sens: [u32; 3],
        sys_mode: i32,
        sys_org: [i32; 2],
        sys_ext: [i32; 2],
        sys_sens: [u32; 2],
    }

    /// `AXIS`: the range a reading runs over.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Axis {
        min: i32,
        max: i32,
        units: u32,
        resolution: u32,
    }

    /// One packet, with the fields asked for in `pkt_data`, in the order the
    /// header lists them: buttons, x, y, pressure.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Packet {
        buttons: u32,
        x: i32,
        y: i32,
        pressure: u32,
    }

    /// The functions used, looked up by name once the DLL is loaded.
    struct Api {
        info: WtInfoW,
        open: WtOpenW,
        close: WtClose,
        packets_get: WtPacketsGet,
        enable: WtEnable,
        overlap: WtOverlap,
    }

    pub(crate) struct Tablet {
        module: *mut c_void,
        api: Api,
        ctx: Hctx,
        /// The device's pressure range, from `DVC_NPRESSURE`.
        range: (i32, i32),
    }

    impl Tablet {
        /// Opens a context on `window`, if `Wintab32.dll` is there and
        /// reports a tablet. Says in the log either way.
        pub(crate) fn open(window: &Window) -> Option<Self> {
            let Ok(handle) = window.window_handle() else { return None };
            let RawWindowHandle::Win32(handle) = handle.as_raw() else { return None };
            let hwnd = handle.hwnd.get();

            let name: Vec<u16> = "Wintab32.dll\0".encode_utf16().collect();
            // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call.
            let module = unsafe { LoadLibraryW(name.as_ptr()) };
            if module.is_null() {
                log::info!("WinTab: Wintab32.dll not found; pen pressure comes through Windows Ink only");
                return None;
            }
            let Some(api) = Api::load(module) else {
                log::info!("WinTab: Wintab32.dll found, but it lacks the WinTab functions; not used");
                // SAFETY: `module` came from `LoadLibraryW` above and nothing else holds it.
                unsafe { FreeLibrary(module) };
                return None;
            };

            // SAFETY: a zeroed LOGCONTEXTW is a valid one to fill in.
            let mut lc: LogContext = unsafe { std::mem::zeroed() };
            // SAFETY: `lc` is the 212-byte LOGCONTEXTW the call writes.
            let got = unsafe { (api.info)(WTI_DEFSYSCTX, 0, ptr::addr_of_mut!(lc).cast()) };
            if got == 0 {
                log::info!("WinTab: Wintab32.dll found, but it reports no tablet; not used");
                unsafe { FreeLibrary(module) };
                return None;
            }
            let device = WTI_DEVICES + lc.device;
            let mut axis = Axis::default();
            // SAFETY: `axis` is the AXIS the call writes.
            let got = unsafe { (api.info)(device, DVC_NPRESSURE, ptr::addr_of_mut!(axis).cast()) };
            if got == 0 || axis.max <= axis.min {
                log::info!("WinTab: the tablet reports no pressure range; not used");
                unsafe { FreeLibrary(module) };
                return None;
            }
            let tablet_name = device_name(&api, device);

            for (i, c) in "GolemSculpt".encode_utf16().enumerate() {
                lc.name[i] = c;
            }
            lc.pkt_data = PK_BUTTONS | PK_X | PK_Y | PK_NORMAL_PRESSURE;
            // Absolute values for every field: buttons as a bitmask.
            lc.pkt_mode = 0;
            lc.move_mask = lc.pkt_data;
            lc.btn_up_mask = lc.btn_dn_mask;
            lc.options |= CXO_SYSTEM | CXO_MESSAGES;

            // SAFETY: `hwnd` is this live window's handle; `lc` is a filled-in
            // LOGCONTEXTW the driver copies.
            let ctx = unsafe { (api.open)(hwnd, &mut lc, 1) };
            if ctx.is_null() {
                log::info!("WinTab: tablet \"{tablet_name}\" found, but a context could not be opened; not used");
                unsafe { FreeLibrary(module) };
                return None;
            }
            log::info!(
                "WinTab: Wintab32.dll found; tablet \"{tablet_name}\", pressure {}..{}; read as a fallback to Windows Ink",
                axis.min,
                axis.max
            );
            Some(Self { module, api, ctx, range: (axis.min, axis.max) })
        }

        /// Takes every packet off the queue. The latest one's tip pressure
        /// (see [`tip_pressure`]), or `None` if no packet has come since
        /// last asked.
        pub(crate) fn drain(&mut self) -> Option<Option<f32>> {
            let mut latest = None;
            let mut packets = [Packet::default(); BATCH];
            loop {
                // SAFETY: `packets` holds `BATCH` packets of the layout the
                // context was opened with.
                let got = unsafe { (self.api.packets_get)(self.ctx, BATCH as i32, packets.as_mut_ptr().cast()) };
                let got = got.clamp(0, BATCH as i32) as usize;
                if got == 0 {
                    break;
                }
                let last = packets[got - 1];
                latest = Some(tip_pressure(last.buttons, last.pressure, self.range));
                if got < BATCH {
                    break;
                }
            }
            latest
        }

        /// Puts the context back on top and enabled, for when the window
        /// has the focus again.
        pub(crate) fn to_front(&mut self) {
            // SAFETY: `ctx` is the open context this struct owns.
            unsafe {
                (self.api.enable)(self.ctx, 1);
                (self.api.overlap)(self.ctx, 1);
            }
        }
    }

    impl Drop for Tablet {
        fn drop(&mut self) {
            // SAFETY: the context and the module are this struct's own, and
            // nothing is called on them after this.
            unsafe {
                (self.api.close)(self.ctx);
                FreeLibrary(self.module);
            }
        }
    }

    impl Api {
        fn load(module: *mut c_void) -> Option<Self> {
            // SAFETY: `module` is a loaded library; each name is NUL-terminated
            // and the signature is the one `wintab.h` gives the function.
            unsafe fn get<T: Copy>(module: *mut c_void, name: &[u8]) -> Option<T> {
                debug_assert_eq!(name.last(), Some(&0));
                debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
                // SAFETY: as above.
                let p = unsafe { GetProcAddress(module, name.as_ptr()) };
                if p.is_null() {
                    return None;
                }
                // SAFETY: a non-null export of the DLL, read as a function pointer of the size just checked.
                Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
            }
            // SAFETY: `module` is live for as long as the Api is used (see Drop).
            unsafe {
                Some(Self {
                    info: get(module, b"WTInfoW\0")?,
                    open: get(module, b"WTOpenW\0")?,
                    close: get(module, b"WTClose\0")?,
                    packets_get: get(module, b"WTPacketsGet\0")?,
                    enable: get(module, b"WTEnable\0")?,
                    overlap: get(module, b"WTOverlap\0")?,
                })
            }
        }
    }

    /// The tablet's name as the driver gives it, for the log.
    fn device_name(api: &Api, device: u32) -> String {
        // SAFETY: a null output asks only for the size.
        let bytes = unsafe { (api.info)(device, DVC_NAME, ptr::null_mut()) } as usize;
        if bytes == 0 {
            return "unnamed tablet".to_owned();
        }
        let mut buf = vec![0u16; bytes / 2 + 1];
        // SAFETY: `buf` has room for the `bytes` the call writes.
        unsafe { (api.info)(device, DVC_NAME, buf.as_mut_ptr().cast()) };
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end]).trim().to_owned()
    }
}

#[cfg(not(windows))]
mod platform {
    //! No WinTab off Windows: the pen's pressure comes through the window.
    use winit::window::Window;

    pub(crate) struct Tablet;

    impl Tablet {
        pub(crate) fn open(_: &Window) -> Option<Self> {
            None
        }

        pub(crate) fn drain(&mut self) -> Option<Option<f32>> {
            None
        }

        pub(crate) fn to_front(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Huion's range, as its driver reports it.
    const HUION: (i32, i32) = (0, 8191);

    #[test]
    fn the_tip_pressure_is_the_devices_reading_as_a_share_of_its_range() {
        assert_eq!(tip_pressure(1, 8191, HUION), Some(1.0));
        assert_eq!(tip_pressure(1, 0, HUION), Some(0.0), "the tip down at the lightest touch opens at none, not as a mouse");
        let half = tip_pressure(1, 4096, HUION).unwrap();
        assert!((half - 0.5).abs() < 0.001, "{half}");
        // A range that does not start at zero is measured from its start.
        assert_eq!(tip_pressure(1, 100, (100, 1123)), Some(0.0));
        assert_eq!(tip_pressure(1, 1123, (100, 1123)), Some(1.0));
    }

    #[test]
    fn a_hovering_pen_reads_as_no_pressure_at_all() {
        assert_eq!(tip_pressure(0, 0, HUION), None, "tip up, nothing pressed: a mouse's sample");
        assert_eq!(tip_pressure(2, 0, HUION), None, "the barrel button alone is not the tip");
        assert_eq!(tip_pressure(0, 300, HUION), Some(300.0 / 8191.0), "pressure with no tip bit is still a touch");
    }

    #[test]
    fn a_reading_outside_the_range_is_held_to_it() {
        assert_eq!(tip_pressure(1, 9000, HUION), Some(1.0));
        // A degenerate range cannot divide by nothing.
        assert_eq!(tip_pressure(1, 5, (5, 5)), Some(0.0));
    }

    #[test]
    fn wintab_gives_way_once_windows_ink_has_reported_pressure() {
        assert_eq!(fallback(false, Some(0.4)), Some(0.4), "no pen event with pressure yet: WinTab's reading is the stroke's");
        assert_eq!(fallback(false, None), None);
        assert_eq!(fallback(true, Some(0.4)), None, "Windows Ink reports the pen: the packets are ignored, so it never counts twice");
        assert_eq!(fallback(true, None), None);
    }
}
