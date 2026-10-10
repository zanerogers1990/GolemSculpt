//! Where the rig stands in the world, and the object it is attached to.
//!
//! Sphere positions and sizes are kept in the rig's own space. A [`Frame`]
//! places that space in the world: a position, a turn and one even size. Its
//! x = 0 plane is the mirror plane. A fresh rig's frame is the world's own;
//! attaching the rig to an object seats the frame on that object, so the two
//! share a mirror plane, and from then on the frame is carried along whenever
//! the object is moved, turned or resized.
//!
//! The frame never takes an uneven stretch. A sphere stretched along one axis
//! is no longer a sphere, and every radius and the skin's grid assume it is
//! one, so the rig is carried by a rigid move plus an even resize and nothing
//! else.
//!
//! Following is not an edit: it leaves no undo step. Attaching, detaching and
//! re-centring the mirror plane are edits, and each is one step in the rig's
//! own history.

use glam::{Mat4, Quat, Vec3};

use super::SSpheres;
use crate::scene::Object;

/// How far apart the three growth factors of a resize may be (relative to
/// the factor itself) and still count as an even resize. Loose enough for an
/// even resize whose three numbers were each rounded on the way.
const EVEN_RESIZE_TOLERANCE: f32 = 1e-4;

/// The rig's own space in the world.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Frame {
    pub position: Vec3,
    pub rotation: Quat,
    /// One number, never three: see the module notes.
    pub scale: f32,
}

impl Frame {
    pub const IDENTITY: Frame = Frame { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: 1.0 };

    /// True for a rig standing in the world's own space, which is every rig
    /// until it is attached to something that has been moved or turned.
    pub fn is_identity(&self) -> bool {
        *self == Frame::IDENTITY
    }

    pub fn to_world(&self, point: Vec3) -> Vec3 {
        self.position + self.rotation * (point * self.scale)
    }

    pub fn to_rig(&self, point: Vec3) -> Vec3 {
        self.rotation.inverse() * (point - self.position) / self.scale
    }

    /// A direction out of the rig's space. The scale is even, so directions
    /// and normals only turn.
    pub fn direction_to_world(&self, direction: Vec3) -> Vec3 {
        self.rotation * direction
    }

    pub fn direction_to_rig(&self, direction: Vec3) -> Vec3 {
        self.rotation.inverse() * direction
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(Vec3::splat(self.scale), self.rotation, self.position)
    }

    /// This frame moved onto an object: the object's place and turn, so its
    /// own x = 0 plane becomes the mirror plane. The size stays the rig's:
    /// taking the object's would resize every sphere on the spot.
    pub fn seated_on(self, body: &Body) -> Frame {
        Frame { position: body.position, rotation: body.rotation, scale: self.scale }
    }

    /// This frame after the object it follows went from `was` to `now`: it
    /// keeps its place relative to the object through a move, a turn and an
    /// even resize. A stretch along one axis is not followed (see the module
    /// notes); the rig keeps its size and its distance from the object's
    /// centre.
    pub fn carried(self, was: &Body, now: &Body) -> Frame {
        let grow = even_resize(was.scale, now.scale);
        // A frame seated on the object (the usual case) stays seated on it
        // exactly, however many small steps a gizmo drag arrives in. The
        // sums below give the same answer, but with a little rounding each
        // time, which would add up.
        if self.position == was.position && self.rotation == was.rotation {
            return Frame { position: now.position, rotation: now.rotation, scale: self.scale * grow };
        }
        let turn = (now.rotation * was.rotation.inverse()).normalize();
        Frame {
            position: now.position + turn * (self.position - was.position) * grow,
            rotation: (turn * self.rotation).normalize(),
            scale: self.scale * grow,
        }
    }
}

/// How much an object grew going from size `was` to size `now`, when it grew
/// by the same amount along all three of its axes; 1 for anything else.
fn even_resize(was: Vec3, now: Vec3) -> f32 {
    let ratio = now / was;
    let factor = ratio.x;
    let even = ratio.is_finite()
        && factor > 1e-6
        && (ratio - Vec3::splat(factor)).abs().max_element() <= factor * EVEN_RESIZE_TOLERANCE;
    if even { factor } else { 1.0 }
}

/// A scene object as the rig knows it: which one, and how it stands.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Body {
    pub id: u64,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Body {
    pub fn of(object: &Object) -> Body {
        Body { id: object.id, position: object.position, rotation: object.rotation, scale: object.scale }
    }
}

impl SSpheres {
    pub fn frame(&self) -> Frame {
        self.frame
    }

    /// The object the rig follows, if it is attached to one.
    pub fn attached_to(&self) -> Option<u64> {
        self.attached.map(|body| body.id)
    }

    /// Moves the frame without moving a sphere: each one is written back in
    /// the new frame at the world point it had in the old. Without that the
    /// whole blockout would jump with the mirror plane. Returns whether the
    /// frame moved at all.
    ///
    /// Only for frames of the same size, which is what [`Frame::seated_on`]
    /// gives: a change of size would need every radius rewritten too.
    fn reseat(&mut self, frame: Frame) -> bool {
        if frame == self.frame {
            return false;
        }
        let old = self.frame;
        for index in 0..self.rig.nodes().len() {
            if let Some(position) = self.rig.get(index).map(|node| node.position) {
                self.rig.set_position(index, frame.to_rig(old.to_world(position)));
            }
        }
        self.frame = frame;
        self.seat_version += 1;
        true
    }

    /// Stands a rig with no spheres on an object, ready for its first one.
    /// Nothing is drawn of an empty rig but the ghost and the mirror plane,
    /// so this is not an edit. It starts again at its natural size, and the
    /// size of the next sphere is kept as it looks rather than as a number.
    pub(super) fn seat_empty(&mut self, body: &Body) {
        if !self.rig.is_empty() {
            return;
        }
        self.last_radius *= self.frame.scale;
        self.frame = Frame { position: body.position, rotation: body.rotation, scale: 1.0 };
    }

    /// Attaches the rig to an object: its mirror plane becomes the object's
    /// own, and the rig follows the object from now on. No sphere moves.
    /// Returns whether the mirror plane moved, which under symmetry moves
    /// the mirrored half. One undo step.
    pub fn attach(&mut self, body: Body) -> bool {
        self.end_drag();
        self.commit_edit();
        self.begin_edit("Attach Spheres");
        if self.attached_to() != Some(body.id) {
            self.seat_version += 1;
        }
        self.attached = Some(body);
        let moved = if self.rig.is_empty() {
            let before = self.frame;
            self.seat_empty(&body);
            self.frame != before
        } else {
            self.reseat(self.frame.seated_on(&body))
        };
        self.commit_edit();
        moved
    }

    /// Lets go of the object. The spheres and the mirror plane stay exactly
    /// where they are; they just stop following. One undo step. False when
    /// nothing was attached.
    pub fn detach(&mut self) -> bool {
        if self.attached.is_none() {
            return false;
        }
        self.end_drag();
        self.commit_edit();
        self.begin_edit("Detach Spheres");
        self.attached = None;
        self.seat_version += 1;
        self.commit_edit();
        true
    }

    /// Moves the mirror plane onto an object without moving a sphere, and
    /// without attaching to it. One undo step. False when the plane was
    /// already there.
    pub fn recentre(&mut self, body: Body) -> bool {
        self.end_drag();
        self.commit_edit();
        self.begin_edit("Re-centre Plane");
        let moved = self.reseat(self.frame.seated_on(&body));
        self.commit_edit();
        moved
    }

    /// Carries the rig along with the object it is attached to. Called once
    /// a frame, with the tool up or put away: the object is usually moved
    /// with the Transform gizmo, while this tool is not the one in hand.
    /// `find` looks an object up by its id.
    ///
    /// The attachment remembers how the object stood when the rig last
    /// caught up with it, and undo steps keep that with the frame they
    /// restore. So stepping back to before the object was moved still lands
    /// the rig beside the object where it is now.
    pub fn follow(&mut self, find: impl Fn(u64) -> Option<Body>) {
        let Some(was) = self.attached else { return };
        match find(was.id) {
            Some(now) if now != was => {
                self.frame = self.frame.carried(&was, &now);
                self.attached = Some(now);
            }
            Some(_) => {}
            None => {
                self.attached = None;
                self.say("The object the spheres were attached to is gone. They stay where they are.");
            }
        }
    }
}
