//! The pen's samples at the rate the tablet sends them.
//!
//! A browser hands the page one pointer move per screen frame, however many
//! samples the pen sent in that time; the rest are folded into that event as
//! its "coalesced" events. egui reads only the one it is handed, so a quick
//! stroke drawn from egui's moves alone is a few straight pieces, and its
//! pressure is the frame's last. The page keeps every sample instead (the
//! pen block in index.html): its place in physical pixels on the canvas, its
//! pressure and its time. The app takes them here once a frame and gives
//! every one to the stroke, as the desktop gives it every touch event.
//!
//! The page clears the list when a button goes down and puts that press
//! first in it, so what is in the list after a press is that stroke's.

use glam::Vec2;

/// One sample from the page.
#[derive(Clone, Copy, Debug)]
pub struct PenSample {
    /// Physical pixels from the canvas's top-left corner, as the app's own
    /// pointer positions are.
    pub pos: Vec2,
    /// The pen's pressure, 0 to 1; `None` for a mouse or a finger.
    pub pressure: Option<f32>,
    /// Seconds, on the same clock as egui's frame time.
    pub time: f64,
    /// The press that began the list, not a move.
    pub down: bool,
}

/// The samples the page has kept since the last call, oldest first, or
/// `None` when the page keeps none (an older index.html, or not a browser):
/// then egui's own moves are all there is.
pub fn take() -> Option<Vec<PenSample>> {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        // Looked up each time rather than bound: a page without the pen
        // block must not stop the app from starting.
        let window = web_sys::window()?;
        let golem = js_sys::Reflect::get(&window, &"golem".into()).ok()?;
        let take = js_sys::Reflect::get(&golem, &"takePenSamples".into()).ok()?.dyn_into::<js_sys::Function>().ok()?;
        let flat = take.call0(&golem).ok()?.dyn_into::<js_sys::Float64Array>().ok()?.to_vec();
        Some(
            flat.chunks_exact(5)
                .map(|s| PenSample {
                    pos: Vec2::new(s[0] as f32, s[1] as f32),
                    pressure: (s[2] >= 0.0).then(|| s[2].clamp(0.0, 1.0) as f32),
                    time: s[3],
                    down: s[4] != 0.0,
                })
                .collect(),
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    None
}
