//! The timelapse's own camera: where it sits for each frame of the video.
//!
//! It is a camera apart from the one you work with, even though by default
//! it follows yours. Filming through the working camera would tie the video
//! to the window's size and shape and, worse, hand the file your raw input:
//! every nudge, every half-orbit while you think, all of it sped up eight or
//! twenty times. This one takes its position from yours through a spring,
//! which is what turns that input into a shot.
//!
//! The spring is stepped once per frame OF VIDEO, by one frame's worth of
//! video time, and only on frames that are recorded. Chasing the subject in
//! real time looks fine live and jerks badly in the file: a re-frame that
//! takes a comfortable half second of real time is squeezed by the timelapse
//! speed with everything else, so at 8x it lands inside two frames and reads
//! as a snap. Stepped in video time, the camera's easing is the same in the
//! file however fast the app ran and whatever the speed is set to.

use glam::Vec3;

use super::{Framing, Options};
use crate::mesh::bvh::Aabb;

/// A camera position in the orbit camera's own terms: angles in degrees
/// about a pivot, at a distance. Springs work well on these four; a spring
/// on a position in space cuts the corner of a fast orbit and drags the
/// camera through the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub pivot: Vec3,
    pub ortho: bool,
}

/// What the rig looks at for one frame.
#[derive(Clone, Copy)]
pub struct Subject {
    /// Where the artist's own camera is.
    pub artist: Shot,
    /// Where the model last changed, in the world, if anywhere yet.
    pub edit_point: Option<Vec3>,
    /// The bounds of what is being sculpted, in the world.
    pub bounds: Option<Aabb>,
    /// The lens: vertical field of view in degrees, and the picture's width
    /// over its height.
    pub fov_y: f32,
    pub aspect: f32,
}

#[derive(Default)]
pub struct Rig {
    yaw: f32,
    pitch: f32,
    distance: f32,
    pivot: Vec3,
    /// The first recorded frame sets the shot up; later ones ease.
    framed: bool,
    yaw_speed: f32,
    pitch_speed: f32,
    distance_speed: f32,
    pivot_speed: Vec3,
}

/// Signed shortest way from one angle to another, in degrees.
fn delta_angle(from: f32, to: f32) -> f32 {
    let d = (to - from).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// One step of a critically damped spring from `current` toward `target`,
/// carrying its speed across steps (the Unity engine's SmoothDamp, which the
/// Unity app's rig used). A spring and not a plain blend, because a blend
/// starts its ease afresh at every new target and reads as a string of little
/// lunges; this is one continuous move. `time` is roughly how long it takes
/// to arrive.
fn smooth_damp(current: f32, target: f32, speed: &mut f32, time: f32, dt: f32) -> f32 {
    let omega = 2.0 / time.max(1e-4);
    let x = omega * dt;
    let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = current - target;
    let temp = (*speed + omega * change) * dt;
    *speed = (*speed - omega * temp) * decay;
    let mut out = target + (change + temp) * decay;
    // Never past the target.
    if (target - current > 0.0) == (out > target) {
        out = target;
        *speed = 0.0;
    }
    out
}

fn smooth_damp_point(current: Vec3, target: Vec3, speed: &mut Vec3, time: f32, dt: f32) -> Vec3 {
    let omega = 2.0 / time.max(1e-4);
    let x = omega * dt;
    let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = current - target;
    let temp = (*speed + omega * change) * dt;
    *speed = (*speed - omega * temp) * decay;
    let mut out = target + (change + temp) * decay;
    if (target - current).dot(out - target) > 0.0 {
        out = target;
        *speed = Vec3::ZERO;
    }
    out
}

impl Rig {
    /// Moves the camera on by one frame of video and returns where it is.
    pub fn step(&mut self, options: &Options, subject: &Subject) -> Shot {
        // No smoothing means "snap", which the spring cannot do as such (it
        // divides by the time), so the time is held just above nothing.
        let smoothing = options.camera_smoothing.max(0.001);
        let dt = 1.0 / options.frame_rate.max(1) as f32;
        match options.framing {
            Framing::FollowArtist => self.follow_artist(options, subject, smoothing, dt),
            Framing::Orbit => self.orbit(options, subject, smoothing, dt),
        }
    }

    /// Films the model from wherever the artist is looking at it from,
    /// leaning the framing toward the part being worked on.
    ///
    /// An artist turns to face whatever they are sculpting, all the time and
    /// without being asked, so their angle is already the best answer to
    /// "where is the interesting thing right now". It gives the angle but
    /// not the subject, though: the pivot sits in the middle of the model
    /// while they detail an ear at the edge of the frame, and the detail is
    /// what the video is about. So the pivot is pulled from theirs toward
    /// the last place the model actually changed.
    fn follow_artist(&mut self, options: &Options, subject: &Subject, smoothing: f32, dt: f32) -> Shot {
        let artist = subject.artist;
        let mut pivot = artist.pivot;
        if options.focus_strength > 0.0 {
            // Before the first stroke there is no such place; the middle of
            // the model is a better guess than the pivot, which may have
            // been panned anywhere.
            let focus = subject.edit_point.or(subject.bounds.map(|b| b.center())).unwrap_or(artist.pivot);
            pivot = artist.pivot.lerp(focus, options.focus_strength.clamp(0.0, 1.0));
        }
        let distance = artist.distance * options.focus_zoom.max(0.2);
        // The short way round: the artist's yaw may be any number of turns
        // away from ours and still be the same view.
        let yaw = self.yaw + delta_angle(self.yaw, artist.yaw);

        if !self.framed {
            (self.pivot, self.distance, self.yaw, self.pitch) = (pivot, distance, yaw, artist.pitch);
            self.framed = true;
        } else {
            self.pivot = smooth_damp_point(self.pivot, pivot, &mut self.pivot_speed, smoothing, dt);
            self.distance = smooth_damp(self.distance, distance, &mut self.distance_speed, smoothing, dt);
            self.yaw = smooth_damp(self.yaw, yaw, &mut self.yaw_speed, smoothing, dt);
            self.pitch = smooth_damp(self.pitch, artist.pitch, &mut self.pitch_speed, smoothing, dt);
        }
        self.shot(artist.ortho)
    }

    /// The turntable: a steady orbit around the subject that pays the
    /// artist's camera no attention.
    fn orbit(&mut self, options: &Options, subject: &Subject, smoothing: f32, dt: f32) -> Shot {
        // Degrees per second OF VIDEO: one steady turn in the finished file
        // however stop-start the session was.
        self.yaw += options.orbit_speed * dt;

        let (mut pivot, mut distance) = (self.pivot, self.distance);
        if let Some(bounds) = subject.bounds.filter(|b| !b.is_empty() && (options.follow_subject || !self.framed)) {
            let radius = (bounds.size() * 0.5).length().max(1e-4);
            // The distance at which the subject just fills the smaller of the
            // two view angles, so a wide picture frames it by height and a
            // tall one by width, and neither crops it.
            let half_v = subject.fov_y.to_radians() * 0.5;
            let half_h = (half_v.tan() * subject.aspect.max(0.01)).atan();
            let fitted = radius / half_v.min(half_h).sin();
            pivot = bounds.center();
            distance = fitted * options.orbit_distance.max(0.2);
        }

        if !self.framed {
            (self.pivot, self.distance, self.pitch) = (pivot, distance.max(1e-4), options.orbit_height);
            self.framed = true;
        } else {
            self.pivot = smooth_damp_point(self.pivot, pivot, &mut self.pivot_speed, smoothing, dt);
            self.distance = smooth_damp(self.distance, distance, &mut self.distance_speed, smoothing, dt);
            // Smoothed for one reason: the Height slider is live, and
            // dragging it mid-recording would otherwise put a jump in the
            // middle of the video.
            self.pitch = smooth_damp(self.pitch, options.orbit_height, &mut self.pitch_speed, smoothing, dt);
        }
        // Always perspective: an orthographic view has no distance for the
        // orbit's own framing to work with.
        self.shot(false)
    }

    fn shot(&self, ortho: bool) -> Shot {
        Shot {
            yaw: self.yaw,
            pitch: self.pitch.clamp(-89.0, 89.0),
            distance: self.distance.max(1e-5),
            pivot: self.pivot,
            ortho,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artist(yaw: f32, pivot: Vec3) -> Shot {
        Shot { yaw, pitch: 15.0, distance: 3.0, pivot, ortho: false }
    }

    fn subject(artist: Shot) -> Subject {
        Subject { artist, edit_point: None, bounds: None, fov_y: 60.0, aspect: 16.0 / 9.0 }
    }

    #[test]
    fn the_first_frame_sets_the_shot_up_without_easing_into_it() {
        let mut rig = Rig::default();
        let options = Options { focus_strength: 0.0, ..Default::default() };
        let shot = rig.step(&options, &subject(artist(140.0, Vec3::new(1.0, 2.0, 3.0))));
        assert_eq!(shot, Shot { yaw: 140.0, pitch: 15.0, distance: 3.0, pivot: Vec3::new(1.0, 2.0, 3.0), ortho: false });
    }

    #[test]
    fn it_follows_the_artist_smoothly_and_the_short_way_round() {
        let mut rig = Rig::default();
        let options = Options { focus_strength: 0.0, ..Default::default() };
        // Yaw 350 is ten degrees short of the front; the rig may call it -10.
        let start = rig.step(&options, &subject(artist(350.0, Vec3::ZERO))).yaw;
        assert!((start - 350.0).rem_euclid(360.0) < 1e-3 || (start - 350.0).rem_euclid(360.0) > 360.0 - 1e-3);
        // The artist turns 20 degrees on, through the wrap, to yaw 10.
        let there = subject(artist(10.0, Vec3::ZERO));
        let end = start + 20.0;
        let first = rig.step(&options, &there).yaw;
        assert!(first > start && first < start + 2.0, "it lunged from {start} to {first}");
        let mut last = first;
        for _ in 0..150 {
            let yaw = rig.step(&options, &there).yaw;
            assert!(yaw >= last - 1e-3 && yaw <= end + 1e-3, "{last} then {yaw}: not the short way round");
            last = yaw;
        }
        // Five seconds of video on, it has arrived.
        assert!((last - end).abs() < 0.05, "ended at {last}");
    }

    #[test]
    fn the_framing_leans_toward_where_the_model_changed() {
        let mut rig = Rig::default();
        let options = Options { focus_strength: 0.5, camera_smoothing: 0.0, ..Default::default() };
        let mut s = subject(artist(0.0, Vec3::ZERO));
        s.edit_point = Some(Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(rig.step(&options, &s).pivot, Vec3::new(1.0, 0.0, 0.0));
        // No edit yet: the middle of the model stands in for it.
        let mut rig = Rig::default();
        s.edit_point = None;
        s.bounds = Some(Aabb { min: Vec3::new(-1.0, 0.0, -1.0), max: Vec3::new(1.0, 4.0, 1.0) });
        assert_eq!(rig.step(&options, &s).pivot, Vec3::new(0.0, 1.0, 0.0));
        // Zoom is a multiple of the artist's own distance.
        let mut rig = Rig::default();
        let closer = Options { focus_zoom: 0.5, ..options };
        assert_eq!(rig.step(&closer, &s).distance, 1.5);
    }

    #[test]
    fn the_orbit_turns_at_a_steady_rate_of_video_time_and_fits_the_subject() {
        let mut rig = Rig::default();
        let options = Options { framing: Framing::Orbit, orbit_speed: 9.0, frame_rate: 30, ..Default::default() };
        let mut s = subject(artist(77.0, Vec3::splat(9.0)));
        s.bounds = Some(Aabb { min: Vec3::splat(-0.5), max: Vec3::splat(0.5) });
        let first = rig.step(&options, &s);
        assert!(!first.ortho);
        assert_eq!((first.pivot, first.pitch), (Vec3::ZERO, 15.0));
        // The cube's bounding ball, radius sqrt(0.75), fills the 60 degree
        // height of a wide picture at radius / sin(30), with a quarter
        // more for air.
        assert!((first.distance - 0.75f32.sqrt() / 0.5 * 1.25).abs() < 1e-4, "at {}", first.distance);
        let mut shot = first;
        for _ in 0..29 {
            shot = rig.step(&options, &s);
        }
        // One second of video is nine degrees, whatever the artist does.
        assert!((shot.yaw - first.yaw - 9.0 * 29.0 / 30.0).abs() < 1e-3);

        // A tall picture frames by its width instead, so it stands further back.
        let mut tall = Rig::default();
        s.aspect = 9.0 / 16.0;
        assert!(tall.step(&options, &s).distance > first.distance * 1.5);
    }

    #[test]
    fn the_spring_arrives_without_overshooting() {
        let (mut value, mut speed) = (0.0f32, 0.0f32);
        let mut biggest = 0.0f32;
        for _ in 0..120 {
            let next = smooth_damp(value, 10.0, &mut speed, 0.8, 1.0 / 30.0);
            assert!(next >= value && next <= 10.0);
            biggest = biggest.max(next - value);
            value = next;
        }
        assert!((value - 10.0).abs() < 0.01);
        // Eased: no single frame carries more than a small part of the move.
        assert!(biggest < 1.0);
        let mut point_speed = Vec3::ZERO;
        let point = smooth_damp_point(Vec3::ZERO, Vec3::X, &mut point_speed, 0.001, 1.0 / 30.0);
        assert!((point - Vec3::X).length() < 5e-3, "no smoothing snaps");
    }
}
