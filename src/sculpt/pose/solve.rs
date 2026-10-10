//! Turning a drag into a pose: what each joint of a chain does, and where
//! that puts the surface it carries.
//!
//! A pose is worked out afresh from the chain at rest for wherever the
//! pointer is now, never from the pose of the frame before. So the same
//! pointer position always gives the same shape, nothing drifts however long
//! the drag lasts, and dragging back to where the press was puts the limb
//! back exactly.
//!
//! Joints are composed the way a skeleton's are: a joint's move is made
//! about its place at rest, and then everything it carries is carried on by
//! the joints after it, toward the one that stays still. Surface in a bend
//! follows a share of its joint's move, a turn through part of the angle
//! rather than a mix of the turned and the unturned position: mixing
//! positions pinches an elbow flat (what animators call the candy-wrapper
//! collapse), and turning part of the way keeps its thickness.

use glam::{Affine3A, Mat3, Quat, Vec3};

use super::build::{MAX_JOINTS, Skeleton};

/// How many times the chain reaches for the pointer and is pulled back to
/// its anchor before the answer is taken. It stops sooner once the grabbed
/// point is on the pointer.
const REACH_PASSES: usize = 64;

/// How far a drag may shrink or grow what it resizes.
const SIZE_RANGE: (f32, f32) = (0.05, 20.0);

/// What one joint does to the surface it carries: a turn and a change of
/// size about the joint, and a shift.
#[derive(Clone, Copy, Debug)]
pub struct JointMove {
    /// Where the joint is at rest.
    origin: Vec3,
    axis: Vec3,
    angle: f32,
    /// The direction `along` sizes; `across` sizes everything square to it.
    grain: Vec3,
    along: f32,
    across: f32,
    shift: Vec3,
}

impl JointMove {
    fn still(origin: Vec3) -> JointMove {
        JointMove { origin, axis: Vec3::X, angle: 0.0, grain: Vec3::X, along: 1.0, across: 1.0, shift: Vec3::ZERO }
    }

    fn turn(origin: Vec3, turn: Quat) -> JointMove {
        // The short way round, so that part of the turn is part of the
        // same turn.
        let turn = if turn.w < 0.0 { -turn } else { turn };
        let (axis, angle) = turn.to_axis_angle();
        JointMove { axis, angle, ..JointMove::still(origin) }
    }

    fn is_still(&self) -> bool {
        self.angle == 0.0 && self.along == 1.0 && self.across == 1.0 && self.shift == Vec3::ZERO
    }

    /// Where `p` goes when it follows `share` of this move.
    #[inline]
    fn part(&self, p: Vec3, share: f32) -> Vec3 {
        if self.is_still() {
            // Not `origin + (p - origin)`, which is not always `p` exactly:
            // a press that moves nothing must change nothing.
            return p;
        }
        let mut d = p - self.origin;
        if self.along != 1.0 || self.across != 1.0 {
            let along = d.dot(self.grain);
            d = self.grain * (along * self.along.powf(share)) + (d - self.grain * along) * self.across.powf(share);
        }
        if self.angle != 0.0 {
            let (sin, cos) = (self.angle * share).sin_cos();
            d = d * cos + self.axis.cross(d) * sin + self.axis * (self.axis.dot(d) * (1.0 - cos));
        }
        self.origin + d + self.shift * share
    }

    /// The whole move as one transform.
    fn whole(&self) -> Affine3A {
        if self.is_still() {
            return Affine3A::IDENTITY;
        }
        let g = self.grain;
        let stretch = Mat3::from_diagonal(Vec3::splat(self.across))
            + Mat3::from_cols(g * g.x, g * g.y, g * g.z) * (self.along - self.across);
        let linear = Mat3::from_axis_angle(self.axis, self.angle) * stretch;
        Affine3A::from_mat3_translation(linear, self.origin + self.shift - linear * self.origin)
    }
}

/// A chain's pose for one frame.
pub struct Pose {
    moves: [JointMove; MAX_JOINTS],
    /// `carried[k]` is where surface that follows joint `k` and every joint
    /// after it in full ends up. The entry after the last joint changes
    /// nothing: that surface stays where it is.
    carried: [Affine3A; MAX_JOINTS + 1],
    /// The grabbed point, then each joint, as they now stand.
    pub points: Vec<Vec3>,
}

impl Pose {
    fn of(skeleton: &Skeleton, moves: &[JointMove]) -> Pose {
        let n = moves.len();
        let mut pose = Pose {
            moves: [JointMove::still(Vec3::ZERO); MAX_JOINTS],
            carried: [Affine3A::IDENTITY; MAX_JOINTS + 1],
            points: Vec::with_capacity(n + 1),
        };
        pose.moves[..n].copy_from_slice(moves);
        for k in (0..n).rev() {
            pose.carried[k] = pose.carried[k + 1] * moves[k].whole();
        }
        pose.points.push(pose.carried[0].transform_point3(skeleton.head));
        // A joint is moved by the joints after it, not by itself.
        for (k, &joint) in skeleton.joints.iter().enumerate().take(n) {
            pose.points.push(pose.carried[k + 1].transform_point3(joint));
        }
        pose
    }

    /// The chain as it was found: nothing moves.
    pub fn rest(skeleton: &Skeleton) -> Pose {
        let moves: Vec<JointMove> = skeleton.joints.iter().map(|&j| JointMove::still(j)).collect();
        Pose::of(skeleton, &moves)
    }

    /// Where the point `p` of the surface goes when it follows `share` of
    /// joint `joint` and all of the joints after it.
    #[inline]
    pub fn place(&self, joint: usize, share: f32, p: Vec3) -> Vec3 {
        if share >= 1.0 {
            self.carried[joint].transform_point3(p)
        } else {
            self.carried[joint + 1].transform_point3(self.moves[joint].part(p, share))
        }
    }
}

/// Bend: the grabbed point goes to `goal` as nearly as the chain's lengths
/// allow, the last joint stays where it is, and every segment keeps its
/// length. One joint simply points the limb at the pointer; with more, the
/// chain reaches for it like an arm (FABRIK: pull the chain to the goal from
/// its tip, pull it back to the anchor from its root, and again, which
/// settles in a few passes and needs no angles).
///
/// Blender makes a single pass and then slides the chain back to the anchor,
/// which leaves the grabbed point trailing the pointer by however far the
/// anchor was pulled.
pub fn bend(skeleton: &Skeleton, goal: Vec3) -> Pose {
    let n = skeleton.joints.len();
    let rest: Vec<Vec3> = std::iter::once(skeleton.head).chain(skeleton.joints.iter().copied()).collect();
    let lengths: Vec<f32> = rest.windows(2).map(|pair| pair[0].distance(pair[1])).collect();
    let total: f32 = lengths.iter().sum();
    if goal == skeleton.head || total <= 0.0 {
        return Pose::rest(skeleton);
    }
    // The next point of the chain: `length` from `from`, toward where it was.
    let toward = |from: Vec3, to: Vec3, length: f32, was: Vec3| {
        from + (to - from).try_normalize().unwrap_or(was.normalize_or_zero()) * length
    };
    let mut at = rest.clone();
    let out_of_reach = (goal - rest[n]).try_normalize().filter(|_| goal.distance(rest[n]) >= total);
    if let Some(straight) = out_of_reach {
        // The passes below only creep toward a pointer they cannot reach:
        // the answer is the chain laid out straight at it.
        for i in (0..n).rev() {
            at[i] = at[i + 1] + straight * lengths[i];
        }
    } else {
        for _ in 0..REACH_PASSES {
            at[0] = goal;
            for i in 0..n {
                at[i + 1] = toward(at[i], at[i + 1], lengths[i], rest[i + 1] - rest[i]);
            }
            at[n] = rest[n];
            for i in (0..n).rev() {
                at[i] = toward(at[i + 1], at[i], lengths[i], rest[i] - rest[i + 1]);
            }
            if at[0].distance(goal) <= total * 1e-6 {
                break;
            }
        }
    }

    // Each segment's turn, from the anchor outward: what its joint adds to
    // the turn of the segment that carries it.
    let mut moves = [JointMove::still(Vec3::ZERO); MAX_JOINTS];
    let mut carried = Quat::IDENTITY;
    for i in (0..n).rev() {
        moves[i] = JointMove::still(rest[i + 1]);
        let was = (rest[i] - rest[i + 1]).try_normalize();
        let now = (at[i] - at[i + 1]).try_normalize();
        let (Some(was), Some(now)) = (was, now) else { continue };
        let swing = Quat::from_rotation_arc((carried * was).normalize(), now);
        let turned = (swing * carried).normalize();
        moves[i] = JointMove::turn(rest[i + 1], carried.inverse() * turned);
        carried = turned;
    }
    Pose::of(skeleton, &moves[..n])
}

/// The line a joint's twist turns about: toward the middle of what the
/// first joint carries, and from joint to joint after that.
fn spine(skeleton: &Skeleton, joint: usize) -> Vec3 {
    let toward = if joint == 0 { skeleton.core } else { skeleton.joints[joint - 1] };
    (toward - skeleton.joints[joint]).normalize_or_zero()
}

/// The direction along the limb at the grabbed end, pointing away from the
/// first joint. Zero if the brush found no limb to speak of.
pub fn grain(skeleton: &Skeleton) -> Vec3 {
    spine(skeleton, 0)
}

/// Twist: each joint turns what it carries about the limb's own line. The
/// whole `angle` reaches the grabbed end; `kept` says how much of it is left
/// at each place along the chain, from 1 at the grabbed end (0.0) to nothing
/// at the anchor (1.0), so a chain of several joints winds up along its
/// length instead of all at one joint.
pub fn twist(skeleton: &Skeleton, angle: f32, kept: impl Fn(f32) -> f32) -> Pose {
    let n = skeleton.joints.len();
    let moves: Vec<JointMove> = (0..n)
        .map(|k| {
            let here = kept(k as f32 / n as f32);
            let beyond = if k + 1 == n { 0.0 } else { kept((k + 1) as f32 / n as f32) };
            let still = JointMove::still(skeleton.joints[k]);
            match spine(skeleton, k) {
                Vec3::ZERO => still,
                axis => JointMove { axis, angle: angle * (here - beyond), ..still },
            }
        })
        .collect();
    Pose::of(skeleton, &moves)
}

/// Resize: what the first joint carries turns to point at `goal` and grows
/// or shrinks about the joint until the grabbed point is on it.
pub fn resize(skeleton: &Skeleton, goal: Vec3) -> Pose {
    let joint = skeleton.joints[0];
    let (was, now) = (skeleton.head - joint, goal - joint);
    let (Some(from), Some(to)) = (was.try_normalize(), now.try_normalize()) else { return Pose::rest(skeleton) };
    if goal == skeleton.head {
        return Pose::rest(skeleton);
    }
    let size = (now.length() / was.length()).clamp(SIZE_RANGE.0, SIZE_RANGE.1);
    let turn = JointMove::turn(joint, Quat::from_rotation_arc(from, to));
    Pose::of(skeleton, &[JointMove { along: size, across: size, ..turn }])
}

/// Slide: what the first joint carries moves by `by` without turning.
pub fn slide(skeleton: &Skeleton, by: Vec3) -> Pose {
    Pose::of(skeleton, &[JointMove { shift: by, ..JointMove::still(skeleton.joints[0]) }])
}

/// Stretch: what the first joint carries is made longer or shorter along its
/// own line, by as much as `goal` lies further along it than the grabbed
/// point, and thinner or fatter to keep its volume.
pub fn stretch(skeleton: &Skeleton, goal: Vec3) -> Pose {
    let joint = skeleton.joints[0];
    let grain = grain(skeleton);
    if grain == Vec3::ZERO || goal == skeleton.head {
        return Pose::rest(skeleton);
    }
    // How far along the limb the grabbed point is from the joint. Grabbed
    // close beside the joint, the least pull would otherwise be a great
    // stretch: half the way to the middle of the limb is the least it counts
    // as.
    let reach = (skeleton.head - joint).dot(grain).max(0.5 * skeleton.core.distance(joint));
    let along = (1.0 + (goal - skeleton.head).dot(grain) / reach).clamp(SIZE_RANGE.0, SIZE_RANGE.1);
    let stretched = JointMove { grain, along, across: along.sqrt().recip(), ..JointMove::still(joint) };
    Pose::of(skeleton, &[stretched])
}
