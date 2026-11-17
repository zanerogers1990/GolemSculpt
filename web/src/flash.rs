//! The selection flash, as the desktop's (`src/render/flash.rs`): an object
//! pulses blue for 0.35 s when it is picked, framed or selected, so the user
//! can see which one they are now working on. The colour starts at its full
//! strength and fades linearly to nothing.
//!
//! This is only the clock. The renderer takes each object's colour from
//! [`Flash::color`] and lays it over that object's picture (shader.wgsl).

use web_time::Instant;

/// How long a pulse lasts, in seconds.
const SECONDS: f32 = 0.35;
/// The colour as shown on screen, with the strength it starts at (the
/// desktop's `Kind::Select`).
const COLOR: [f32; 4] = [0.3, 0.7, 1.0, 0.6];

/// The pulses running now, one per object at most.
#[derive(Default)]
pub struct Flash {
    pulses: Vec<(u64, Instant)>,
}

impl Flash {
    /// Starts a pulse on object `id`, or starts its pulse again.
    pub fn start(&mut self, id: u64) {
        self.pulses.retain(|&(o, _)| o != id);
        self.pulses.push((id, Instant::now()));
    }

    /// Drops the pulses that have finished; true while any is still running
    /// (another frame is wanted).
    pub fn tick(&mut self) -> bool {
        self.pulses.retain(|&(_, t)| t.elapsed().as_secs_f32() < SECONDS);
        !self.pulses.is_empty()
    }

    /// The colour to lay over object `id` this frame; alpha 0 when it is not
    /// pulsing.
    pub fn color(&self, id: u64) -> [f32; 4] {
        let Some(&(_, t)) = self.pulses.iter().find(|&&(o, _)| o == id) else { return [0.0; 4] };
        let left = (1.0 - t.elapsed().as_secs_f32() / SECONDS).clamp(0.0, 1.0);
        [COLOR[0], COLOR[1], COLOR[2], COLOR[3] * left]
    }
}
