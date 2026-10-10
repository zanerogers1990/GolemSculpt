//! The profile's own undo history.
//!
//! Forty handle drags on the scene's history would bury the mesh edits either side of them, so the
//! profile is a scaffold that keeps its own: Z answers here while the tool is up and has something to
//! undo, and falls through to the scene's history otherwise, which is how Z right after Create takes
//! the new object away again.
//!
//! A step is a snapshot of everything the artist can change by hand: the control points, whether the
//! curve is a loop, and where the lathe stands and how big it is. Detail settings are not part of the
//! shape and are not undone.
//!
//! An edit is opened before it changes anything and closed when the gesture ends; closing it pushes a
//! step only if something really changed, so it is safe to open one on every press. A streamed edit (a
//! slider) stays open for [`COALESCE_SECONDS`] after its last change and closes as one step, and a
//! discrete edit that arrives inside that window closes it first rather than joining it, so Esc on a
//! drag can never also revert the slider move before it.

use glam::Vec3;

use crate::plane::Plane;
use crate::{LatheProfile, ProfilePoint};

/// Steps kept; the oldest is forgotten past this.
pub const MAX_STEPS: usize = 128;

/// How long a streamed edit waits for its next change before it closes as a step.
pub const COALESCE_SECONDS: f64 = 0.4;

/// The scaffold as it was at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    points: Vec<ProfilePoint>,
    closed_loop: bool,
    scale: f32,
    root: Vec3,
}

impl Pose {
    pub fn capture(profile: &LatheProfile, plane: &Plane) -> Pose {
        Pose { points: profile.points().to_vec(), closed_loop: profile.closed_loop(), scale: plane.scale, root: plane.root }
    }

    /// Whether the scaffold is exactly this now: a compare, with no copy, so it can be asked every
    /// frame.
    pub fn is_current(&self, profile: &LatheProfile, plane: &Plane) -> bool {
        self.closed_loop == profile.closed_loop()
            && self.scale == plane.scale
            && self.root == plane.root
            && self.points == profile.points()
    }

    /// Puts the scaffold back as it was.
    pub fn restore(&self, profile: &mut LatheProfile, plane: &mut Plane) {
        profile.set_points(&self.points, self.closed_loop);
        plane.scale = self.scale;
        plane.root = self.root;
    }
}

struct Step {
    pose: Pose,
    label: String,
}

struct Open {
    before: Pose,
    label: String,
    /// When a streamed edit closes if nothing more arrives; `None` for a discrete one.
    deadline: Option<f64>,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    open: Option<Open>,
}

impl History {
    /// Opens an edit: snapshots now, before the change. `coalesce` of zero is a discrete edit that ends
    /// when [`commit`](History::commit) is called; above zero it is streamed, and stays open that many
    /// seconds past its latest change. Opening one that is already open (the same discrete edit, or the
    /// same stream) does nothing.
    pub fn begin(&mut self, now: f64, label: &str, coalesce: f64, profile: &LatheProfile, plane: &Plane) {
        if let Some(open) = &mut self.open {
            let streamed = open.deadline.is_some();
            let same_stream = streamed && coalesce > 0.0 && open.label == label;
            if same_stream || (!streamed && coalesce <= 0.0) {
                if coalesce > 0.0 {
                    open.deadline = Some(now + coalesce);
                }
                return;
            }
            self.commit(profile, plane);
        }
        self.open = Some(Open {
            before: Pose::capture(profile, plane),
            label: label.to_owned(),
            deadline: (coalesce > 0.0).then_some(now + coalesce),
        });
    }

    /// Closes the open edit, keeping what it did as one step if it did anything.
    pub fn commit(&mut self, profile: &LatheProfile, plane: &Plane) {
        let Some(open) = self.open.take() else { return };
        if open.before.is_current(profile, plane) {
            return;
        }
        self.undo.push(Step { pose: open.before, label: open.label });
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Throws the open edit away and returns the pose to put back, if it changed anything: Esc
    /// mid-drag.
    pub fn cancel(&mut self, profile: &LatheProfile, plane: &Plane) -> Option<Pose> {
        let open = self.open.take()?;
        (!open.before.is_current(profile, plane)).then_some(open.before)
    }

    /// Closes a streamed edit that has waited long enough. Not while `busy` (a drag is in progress).
    pub fn tick(&mut self, now: f64, busy: bool, profile: &LatheProfile, plane: &Plane) {
        if busy {
            return;
        }
        if self.open.as_ref().and_then(|open| open.deadline).is_some_and(|deadline| now >= deadline) {
            self.commit(profile, plane);
        }
    }

    /// Steps back: the pose to restore and the label of the step undone. The scaffold as it is now
    /// goes on the redo stack.
    pub fn undo(&mut self, profile: &LatheProfile, plane: &Plane) -> Option<(Pose, String)> {
        self.commit(profile, plane);
        let step = self.undo.pop()?;
        self.redo.push(Step { pose: Pose::capture(profile, plane), label: step.label.clone() });
        Some((step.pose, step.label))
    }

    pub fn redo(&mut self, profile: &LatheProfile, plane: &Plane) -> Option<(Pose, String)> {
        self.commit(profile, plane);
        let step = self.redo.pop()?;
        self.undo.push(Step { pose: Pose::capture(profile, plane), label: step.label.clone() });
        Some((step.pose, step.label))
    }

    /// An edit is open and has changed the scaffold, so Z has something to take back even though no
    /// step is pushed yet.
    fn has_open_edit(&self, profile: &LatheProfile, plane: &Plane) -> bool {
        self.open.as_ref().is_some_and(|open| !open.before.is_current(profile, plane))
    }

    pub fn can_undo(&self, profile: &LatheProfile, plane: &Plane) -> bool {
        !self.undo.is_empty() || self.has_open_edit(profile, plane)
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What Z would undo, for the panel.
    pub fn next_undo(&self, profile: &LatheProfile, plane: &Plane) -> Option<&str> {
        if self.has_open_edit(profile, plane) {
            return self.open.as_ref().map(|open| open.label.as_str());
        }
        self.undo.last().map(|step| step.label.as_str())
    }

    pub fn steps(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;

    fn scaffold() -> (LatheProfile, Plane) {
        let points = [ProfilePoint::corner(0.0, 0.0), ProfilePoint::smooth(0.5, 1.0), ProfilePoint::corner(0.0, 2.0)];
        (LatheProfile::from_points(&points, false), Plane::new(Vec3::ZERO))
    }

    #[test]
    fn an_edit_that_changed_nothing_leaves_no_step() {
        let (profile, plane) = scaffold();
        let mut history = History::default();
        history.begin(0.0, "Move Point", 0.0, &profile, &plane);
        history.commit(&profile, &plane);
        assert!(!history.can_undo(&profile, &plane) && history.steps() == 0);
    }

    #[test]
    fn undo_and_redo_walk_the_shape_back_and_forth() {
        let (mut profile, mut plane) = scaffold();
        let mut history = History::default();
        history.begin(0.0, "Move Point", 0.0, &profile, &plane);
        profile.set(1, Vec2::new(0.9, 1.0));
        history.commit(&profile, &plane);
        history.begin(1.0, "Scale", 0.0, &profile, &plane);
        plane.scale = 2.0;
        history.commit(&profile, &plane);
        assert_eq!(history.next_undo(&profile, &plane), Some("Scale"));

        let (pose, label) = history.undo(&profile, &plane).unwrap();
        pose.restore(&mut profile, &mut plane);
        assert_eq!((label.as_str(), plane.scale, profile[1].position.x), ("Scale", 1.0, 0.9));
        let (pose, _) = history.undo(&profile, &plane).unwrap();
        pose.restore(&mut profile, &mut plane);
        assert_eq!(profile[1].position.x, 0.5);
        assert!(history.undo(&profile, &plane).is_none());

        let (pose, label) = history.redo(&profile, &plane).unwrap();
        pose.restore(&mut profile, &mut plane);
        assert_eq!((label.as_str(), profile[1].position.x), ("Move Point", 0.9));
        assert!(history.can_redo());
    }

    #[test]
    fn a_new_edit_clears_what_could_have_been_redone() {
        let (mut profile, mut plane) = scaffold();
        let mut history = History::default();
        history.begin(0.0, "A", 0.0, &profile, &plane);
        profile.set(1, Vec2::new(0.7, 1.0));
        history.commit(&profile, &plane);
        let (pose, _) = history.undo(&profile, &plane).unwrap();
        pose.restore(&mut profile, &mut plane);
        assert!(history.can_redo());
        history.begin(1.0, "B", 0.0, &profile, &plane);
        profile.set(1, Vec2::new(0.6, 1.0));
        history.commit(&profile, &plane);
        assert!(!history.can_redo());
    }

    #[test]
    fn an_open_edit_that_changed_something_can_already_be_undone_and_cancelled() {
        let (mut profile, mut plane) = scaffold();
        let mut history = History::default();
        history.begin(0.0, "Move Point", 0.0, &profile, &plane);
        assert!(!history.can_undo(&profile, &plane), "nothing has changed yet");
        profile.set(1, Vec2::new(0.9, 1.0));
        assert!(history.can_undo(&profile, &plane));
        assert_eq!(history.next_undo(&profile, &plane), Some("Move Point"));

        let before = history.cancel(&profile, &plane).expect("it changed something");
        before.restore(&mut profile, &mut plane);
        assert_eq!(profile[1].position.x, 0.5);
        assert!(!history.can_undo(&profile, &plane) && history.cancel(&profile, &plane).is_none());
    }

    #[test]
    fn reopening_a_discrete_edit_is_one_step_with_the_first_snapshot() {
        let (mut profile, plane) = scaffold();
        let mut history = History::default();
        // An insert opens the step, and the drag that places the point opens it again.
        history.begin(0.0, "Add Point", 0.0, &profile, &plane);
        profile.insert(1, Vec2::new(0.3, 0.5), false);
        history.begin(0.0, "Add Point", 0.0, &profile, &plane);
        profile.set(1, Vec2::new(0.4, 0.5));
        history.commit(&profile, &plane);
        assert_eq!(history.steps(), 1);
        let (pose, _) = history.undo(&profile, &plane).unwrap();
        let mut restored = LatheProfile::new();
        pose.restore(&mut restored, &mut Plane::new(Vec3::ZERO));
        assert_eq!(restored.len(), 3, "back before the insert");
    }

    #[test]
    fn a_stream_of_slider_moves_is_one_step_closed_by_the_clock() {
        let (profile, mut plane) = scaffold();
        let mut history = History::default();
        for (i, scale) in [1.2, 1.4, 1.6].into_iter().enumerate() {
            history.begin(i as f64 * 0.1, "Scale", COALESCE_SECONDS, &profile, &plane);
            plane.scale = scale;
        }
        assert_eq!(history.steps(), 0, "still open");
        assert!(history.can_undo(&profile, &plane));
        history.tick(0.5, false, &profile, &plane);
        assert_eq!(history.steps(), 0, "0.2 + 0.4 has not passed");
        history.tick(0.7, true, &profile, &plane);
        assert_eq!(history.steps(), 0, "never mid-drag");
        history.tick(0.7, false, &profile, &plane);
        assert_eq!(history.steps(), 1);
        let (pose, _) = history.undo(&profile, &plane).unwrap();
        let mut restored = plane;
        pose.restore(&mut LatheProfile::new(), &mut restored);
        assert_eq!(restored.scale, 1.0, "all three moves came back together");
    }

    #[test]
    fn a_discrete_edit_inside_a_stream_closes_the_stream_first() {
        let (mut profile, mut plane) = scaffold();
        let mut history = History::default();
        history.begin(0.0, "Radius", COALESCE_SECONDS, &profile, &plane);
        profile.scale_radius(2.0);
        history.begin(0.1, "Move Point", 0.0, &profile, &plane);
        profile.set(1, Vec2::new(0.2, 1.0));
        // Esc on the drag puts back only the drag.
        let before = history.cancel(&profile, &plane).unwrap();
        before.restore(&mut profile, &mut plane);
        assert!((profile[1].position.x - 1.0).abs() < 1e-6, "the radius change survived");
        assert_eq!(history.steps(), 1);
    }

    #[test]
    fn a_different_stream_does_not_join_the_open_one() {
        let (profile, mut plane) = scaffold();
        let mut history = History::default();
        history.begin(0.0, "Scale", COALESCE_SECONDS, &profile, &plane);
        plane.scale = 2.0;
        history.begin(0.1, "Radius", COALESCE_SECONDS, &profile, &plane);
        assert_eq!(history.steps(), 1, "the scale stream was closed");
    }

    #[test]
    fn the_oldest_step_is_forgotten_past_the_cap() {
        let (profile, mut plane) = scaffold();
        let mut history = History::default();
        for i in 0..MAX_STEPS + 5 {
            history.begin(0.0, "Scale", 0.0, &profile, &plane);
            plane.scale = 1.0 + (i + 1) as f32 * 0.01;
            history.commit(&profile, &plane);
        }
        assert_eq!(history.steps(), MAX_STEPS);
    }
}
