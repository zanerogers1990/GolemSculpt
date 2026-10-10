//! Pen pressure and Lazy Mouse: how raw pointer samples become the screen
//! positions and pressures a stroke works from.

use glam::Vec2;

/// How fast the smoothed pressure chases the pen's, per second.
const PRESSURE_SMOOTHING: f32 = 20.0;

/// The longest step one pressure sample may take. Pen samples arrive every
/// few milliseconds while the tip is down, so this binds mainly on the first
/// sample of a touch, whose gap since the last one is however long the pen
/// was lifted: capped at one 60 Hz frame, the touch eases on from the held
/// value instead of jumping straight to its own pressure.
const MAX_PRESSURE_DT: f32 = 1.0 / 60.0;

/// The pen-pressure filter. A tablet's sensor is noisy enough that raw
/// pressure gives a jittery, stair-stepped stroke, so the value the brushes
/// use chases it instead of tracking it 1:1.
///
/// It is kept between strokes rather than reset: resetting to full strength
/// made every touch ease down from a spike. It starts at 1 for the same
/// reason turned round: starting at 0 made the first stroke of a session
/// open weak.
#[derive(Clone, Copy, Debug)]
pub(super) struct PenPressure {
    pub smoothed: f32,
    /// Time of the last sample, or negative before the first.
    pub last_time: f64,
}

impl Default for PenPressure {
    fn default() -> Self {
        Self { smoothed: 1.0, last_time: -1.0 }
    }
}

impl PenPressure {
    /// One pointer sample's step: the smoothed value eases toward the pen's
    /// pressure over the time since the last sample. A sample with no
    /// pressure (a mouse) leaves the held value alone and only moves the
    /// clock.
    pub fn step(&mut self, pressure: Option<f32>, time: f64) {
        if let Some(raw) = pressure {
            let dt = if self.last_time < 0.0 {
                MAX_PRESSURE_DT
            } else {
                ((time - self.last_time) as f32).clamp(0.0, MAX_PRESSURE_DT)
            };
            self.smoothed += (raw - self.smoothed) * (dt * PRESSURE_SMOOTHING).clamp(0.0, 1.0);
        }
        self.last_time = time;
    }
}

/// Lazy Mouse, the stroke stabiliser: while a stroke is drawn the brush
/// trails the pointer on a taut rope of a set length in screen pixels,
/// moving only once the pointer has pulled the rope taut, and then only
/// straight toward it. Hand tremor inside the rope's length never reaches
/// the brush.
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct LazyRope {
    /// True while a stroke is pulling the rope.
    pub active: bool,
    /// The rope's near end, where the brush is really working.
    pub pos: Vec2,
    /// Its far end: the raw pointer, as of the same step.
    pub raw: Vec2,
}

impl LazyRope {
    /// `strength` is the share of the rope's slack taken up per frame at
    /// this rate. Applied as a decay over elapsed time it means the same at
    /// any frame or input rate.
    const REFERENCE_RATE: f32 = 60.0;

    /// One step of the rope toward `raw`, `dt` seconds after the previous
    /// step. Returns where the stroke should work from: `raw` itself while
    /// the stabiliser is off or nothing is pressed, and on the first step of
    /// every stroke, so a fresh press starts exactly under the pointer and
    /// not wherever the last stroke left the rope.
    pub fn advance(
        &mut self,
        raw: Vec2,
        dt: f32,
        pressed: bool,
        enabled: bool,
        radius: f32,
        strength: f32,
    ) -> Vec2 {
        self.raw = raw;
        if !enabled || !pressed {
            self.active = false;
            self.pos = raw;
            return raw;
        }
        if !self.active {
            self.active = true;
            self.pos = raw;
            return self.pos;
        }
        let delta = raw - self.pos;
        let dist = delta.length();
        if dist > radius {
            self.pos += delta / dist * ((dist - radius) * Self::follow(strength, dt));
        }
        self.pos
    }

    /// The share of the rope's slack taken up over `dt` seconds. Full
    /// strength stays exactly taut at any `dt`, a zero-length step included:
    /// two pointer samples can share a timestamp, and `1 - 0^0` would leave
    /// the brush behind.
    pub fn follow(strength: f32, dt: f32) -> f32 {
        if strength >= 1.0 {
            return 1.0;
        }
        1.0 - (1.0 - strength).powf(dt.max(0.0) * Self::REFERENCE_RATE)
    }
}
