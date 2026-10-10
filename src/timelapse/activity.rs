//! The one "did the model actually change just now?" signal the timelapse
//! records by.
//!
//! The recorder has to answer one question every frame: does this frame
//! belong in the video? The honest answer is "only if the sculpt itself
//! moved". Nothing about the input can say that: the button is held while
//! orbiting, the pointer moves while a stroke is only being lined up, and a
//! click that misses the model looks just like one that lands. So the signal
//! comes from where the model changes: a stroke that is laying down dabs, a
//! gizmo drag, and every step pushed onto the undo history (strokes, remesh,
//! objects added, joined or moved).
//!
//! Undo and redo are left out on purpose, or a timelapse would faithfully
//! record you taking your own work back. They never push a step, so they
//! report nothing.
//!
//! There is one signal for the whole program ([`edited`], [`edited_at`]),
//! reached without a handle the way the history reaches it; the rules
//! themselves are in [`Activity`], which takes the time as an argument.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use glam::Vec3;

#[derive(Clone, Copy, Debug)]
pub struct Activity {
    /// When the model last changed, in seconds; minus infinity until
    /// something happens, which reads as "idle forever": right, before the
    /// first stroke.
    last_edit: f64,
    /// How many changes have been reported. The cheapest way to see that
    /// the hooks are firing.
    pub count: u64,
    /// Where the most recent change landed: an object and a point in that
    /// object's own space, so the point stays right when the object is moved
    /// afterwards. Whole-object changes (a remesh, a join) leave it alone:
    /// the last place you were working is still the best guess at what to
    /// look at.
    pub point: Option<(u64, Vec3)>,
}

impl Activity {
    pub const fn new() -> Activity {
        Activity { last_edit: f64::NEG_INFINITY, count: 0, point: None }
    }

    /// The model changed at `now`.
    pub fn report(&mut self, now: f64) {
        self.last_edit = now;
        self.count += 1;
    }

    /// The model changed at `now`, around `point` of `object` (in that
    /// object's own space).
    pub fn report_at(&mut self, now: f64, object: u64, point: Vec3) {
        self.report(now);
        self.point = Some((object, point));
    }

    pub fn seconds_since_edit(&self, now: f64) -> f64 {
        now - self.last_edit
    }

    /// The whole activity rule: something changed within the last `grace`
    /// seconds. The grace bridges the natural gaps between dabs and between
    /// one stroke and the next, so they do not each become a hard cut.
    pub fn editing(&self, now: f64, grace: f32) -> bool {
        self.seconds_since_edit(now) <= grace.max(0.0) as f64
    }
}

static SIGNAL: Mutex<Activity> = Mutex::new(Activity::new());

/// Seconds on the clock the signal keeps its times by.
pub fn now() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn signal() -> std::sync::MutexGuard<'static, Activity> {
    // The data is plain numbers: a thread that panicked holding the lock
    // cannot have left them in a state worth refusing.
    SIGNAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Call from anywhere that has just changed the model for real.
pub fn edited() {
    signal().report(now());
}

/// As [`edited`], for a change that knows where on `object` it landed.
pub fn edited_at(object: u64, point: Vec3) {
    signal().report_at(now(), object, point);
}

/// The signal as it stands.
pub fn current() -> Activity {
    *signal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_opens_on_an_edit_and_closes_after_the_idle_time() {
        let mut activity = Activity::new();
        // Nothing has happened yet: closed, however generous the grace.
        assert!(!activity.editing(100.0, 0.75));
        assert!(!activity.editing(100.0, 1e9));

        activity.report(100.0);
        assert!(activity.editing(100.0, 0.75));
        assert!(activity.editing(100.7, 0.75));
        assert!(!activity.editing(100.8, 0.75));
        assert_eq!(activity.count, 1);

        // Another dab inside the grace keeps it open from then.
        activity.report(100.7);
        assert!(activity.editing(101.4, 0.75));
        assert!(!activity.editing(101.5, 0.75));

        // No grace: open on the frame of the edit only.
        assert!(activity.editing(100.7, 0.0));
        assert!(!activity.editing(100.71, 0.0));
        assert!(activity.editing(100.7, -3.0));
    }

    #[test]
    fn a_change_without_a_place_keeps_the_last_place() {
        let mut activity = Activity::new();
        assert_eq!(activity.point, None);
        activity.report_at(1.0, 7, Vec3::new(0.1, 0.2, 0.3));
        activity.report(2.0);
        assert_eq!(activity.point, Some((7, Vec3::new(0.1, 0.2, 0.3))));
        assert_eq!(activity.count, 2);
        assert!((activity.seconds_since_edit(2.5) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_program_wide_signal_counts_reports() {
        let before = current().count;
        edited();
        edited_at(3, Vec3::ONE);
        let after = current();
        // Other tests may report too, so at least these two.
        assert!(after.count >= before + 2);
        assert!(after.editing(now(), 5.0));
    }
}
