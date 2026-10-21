//! Pen pressure and Lazy Mouse: how raw pointer samples become the screen
//! positions and pressures a stroke works from.

use glam::Vec2;

/// How fast the smoothed pressure chases the pen's, per second: it closes
/// all but a third of the gap in about 17 milliseconds. Enough to take the
/// grain out of a pen's sensor; at 20 a second (50 milliseconds) a stroke's
/// width and strength trailed the hand visibly, most of all where a stroke
/// tapers at its ends.
const PRESSURE_SMOOTHING: f32 = 57.0;

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
/// A stroke drawn with the pen starts from the pen's own first reading
/// ([`Self::touch`]), not from where the last stroke left the filter: eased
/// in from there, a light touch after a firm stroke opened with a blob, and
/// a firm one after a light stroke with a gap. Between strokes it holds its
/// value, which only matters to a stroke whose first sample carries no
/// pressure; it starts at 1, as a mouse is.
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
    /// A stroke begins with the pen reading `raw`: the filter starts there.
    pub fn touch(&mut self, raw: f32, time: f64) {
        self.smoothed = raw.clamp(0.0, 1.0);
        self.last_time = time;
    }

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

/// The smallest a brush gets under Pressure -> Size, as a share of the ring.
/// Smaller than this a dab falls between the vertices of most meshes and
/// does nothing, while the stroke still pays for it: spacing follows the
/// dab's size, so they would also come ten times as thick.
pub(super) const MIN_PEN_SIZE: f32 = 0.1;

/// The pen's pressure as the brushes read it, from the filter's smoothed
/// value: 0 at no pressure and 1 at full. `curve` is the global Curve
/// setting. An exponent above 1 keeps the response gentle near zero, which
/// is the noisiest part of a pen's sensor.
pub(super) fn pen_curve(smoothed: f32, curve: f32) -> f32 {
    smoothed.clamp(0.0, 1.0).powf(curve)
}

/// The share of its Strength a brush works at under pen pressure `pen` (0 to
/// 1, from [`pen_curve`]; a mouse is always 1). `amount` is the brush's
/// Pressure -> Strength: how much of the strength the lightest touch takes
/// away, all of it at 100% (where the brushes start). Written so that full
/// pressure, and a mouse, give exactly 1.
pub(super) fn pen_strength(amount: f32, pen: f32) -> f32 {
    1.0 - amount * (1.0 - pen)
}

/// The share of the ring's size a brush works at under pen pressure `pen`.
/// `amount` is the brush's Pressure -> Size: how much smaller the lightest
/// touch makes it. Exactly 1 with the setting off, at full pressure and for a
/// mouse, and never under [`MIN_PEN_SIZE`].
pub(super) fn pen_size(amount: f32, pen: f32) -> f32 {
    (1.0 - amount * (1.0 - pen)).max(MIN_PEN_SIZE)
}

/// What the pen is doing right now, for the panel's readout: pen pressure
/// has to be seen arriving before any of the pressure settings can be
/// trusted, and when it is not arriving the fix is in the tablet driver, not
/// in the app.
///
/// Fed by the window's pen events, whether or not they become a stroke (a
/// press on a panel shows as well). The brushes do not read it: they have
/// the filter above.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PenMeter {
    /// A pen has reported pressure at some time in this session.
    pub seen: bool,
    /// A pen reported pressure in an earlier session on this machine: the
    /// settings file remembers it (`Prefs::pen_seen`), so the first touch
    /// after the app opens is taken for the pen it is. The panel's readout
    /// goes by `seen` alone, which says whether pressure arrives now.
    pub known: bool,
    /// The pressure it reports now, 0 to 1, while its tip is down.
    pub now: Option<f32>,
}

impl PenMeter {
    /// The pen's tip is on the tablet, reporting `force` (or none). Returns
    /// the pressure a stroke begun by this touch should open with.
    ///
    /// A pen pressed so lightly that the tablet rounds its pressure to
    /// nothing arrives with no pressure at all, which is also how a mouse
    /// arrives, and a stroke that opens that way is a mouse's: at full
    /// strength and full size, a blob at the head of a light stroke. So once
    /// a pen has shown pressure, in this session or an earlier one, a touch
    /// without any opens at no pressure instead.
    pub fn down(&mut self, force: Option<f32>) -> Option<f32> {
        let pressure = force.or((self.seen || self.known).then_some(0.0));
        self.moved(pressure);
        pressure
    }

    /// The tip is still down, now reporting `force`.
    pub fn moved(&mut self, force: Option<f32>) {
        self.seen |= force.is_some();
        self.now = force.map(|f| f.clamp(0.0, 1.0)).or(self.now);
    }

    /// The tip has left the tablet.
    pub fn lifted(&mut self) {
        self.now = None;
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
