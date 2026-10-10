//! States worth a screenshot (`--demo-tool mask`, `trim`, `split`,
//! `extract`), each reached through the code the gestures run.
//!
//! A cut made from where the camera stands shows the camera nothing: the
//! face it leaves is swept along the line of sight, so it is seen edge on.
//! The demos that cut therefore draw their shape as seen from a second
//! camera off to one side (which is all a gesture needs: a shape and a view),
//! and the real one looks at the result.

use glam::{Quat, Vec2, Vec3};
use sculpt_cut::ScreenRegion;
use sculpt_toolkit::{Command, Ctx, MirrorRepeater, Placement, View};

use crate::action::Action;
use crate::extract::Extract;
use crate::gesture::Shape;
use crate::mask;
use crate::tool::Region;

/// How far round the model the second camera stands, in radians.
const SIDE_TURN: f32 = 0.95;

/// The selected object as the camera sees it.
struct Target {
    centre: Vec3,
    /// Half its largest side, in world units.
    reach: f32,
    /// Its middle on screen, and about how far its edge is from there.
    at: Vec2,
    radius: f32,
}

fn target(cx: &Ctx) -> Option<Target> {
    let scene = cx.scene();
    let bounds = scene.bounds(scene.active());
    if bounds.is_empty() {
        return None;
    }
    let (centre, reach) = (bounds.center(), bounds.size().max_element() * 0.5);
    let at = cx.view.project(centre)?;
    let edge = cx.view.project(centre + cx.view.right * reach)?;
    Some(Target { centre, reach, at, radius: at.distance(edge) })
}

/// A camera turned `turn` radians round the model from the real one, with
/// the model in the middle of its picture.
fn side_view(cx: &Ctx, target: &Target, turn: f32) -> View {
    let eye = target.centre + Quat::from_axis_angle(cx.view.up, turn) * (cx.view.eye - target.centre);
    let fov = 2.0 * cx.view.tan_half_fov.atan().to_degrees();
    View { pixels_per_point: cx.view.pixels_per_point, ..View::perspective(eye, target.centre, fov, cx.view.size) }
}

/// Part of a wobbly loop round `centre`: `turns` of a full turn, as a hand
/// would draw it.
fn drawn_loop(centre: Vec2, radius: f32, turns: f32) -> Vec<Vec2> {
    let steps = (120.0 * turns) as usize;
    (0..=steps)
        .map(|i| {
            let angle = 2.6 + i as f32 / 120.0 * std::f32::consts::TAU;
            let wobble = 1.0 + 0.09 * (angle * 3.0).sin() + 0.05 * (angle * 5.0 + 1.0).cos();
            centre + Vec2::new(angle.cos(), angle.sin() * 0.85) * radius * wobble
        })
        .collect()
}

/// Presses at the first point and drags through the rest, leaving the button
/// down.
fn drag(tool: &mut Region, cx: &mut Ctx, points: &[Vec2]) {
    let Some((&first, rest)) = points.split_first() else { return };
    if tool.begin(cx, first, false) {
        for &p in rest {
            tool.drag_to(cx, p);
        }
    }
}

/// A line from the bottom of the picture to the top, through its middle:
/// what is left of it (the near side, seen from the real camera) is covered.
fn upright_line(view: &View) -> [Vec2; 2] {
    let size = view.size;
    [Vec2::new(size.x * 0.5, size.y * 0.92), Vec2::new(size.x * 0.5, size.y * 0.08)]
}

/// The selected object made smaller, stood to one side of the world's middle
/// and turned, with a live mirror across X, so that its copy stands on the
/// other side and a shape can be drawn over that copy. Returns the copy as a
/// target: where it is on screen and how big.
fn mirrored_copy(cx: &mut Ctx) -> Option<Target> {
    let scene = cx.scene();
    let object = scene.active();
    let info = scene.info(object)?;
    let bounds = scene.bounds(object);
    let reach = bounds.size().max_element() * 0.5;
    let placement = Placement {
        position: info.placement.position + Vec3::X * reach * 0.8,
        rotation: Quat::from_rotation_y(0.5) * Quat::from_rotation_x(-0.3),
        scale: info.placement.scale * 0.55,
    };
    let mirror = MirrorRepeater::new(1, Vec3::ZERO);
    cx.commit("Demo Mirror", vec![Command::Place { object, placement }, Command::Mirror { object, mirror }]);
    // The copy is where the mesh is drawn through the reflected matrix; its middle is
    // the middle of the object's reflected across the world's YZ plane.
    let bounds = cx.scene().bounds(object);
    let centre = bounds.center() * Vec3::new(-1.0, 1.0, 1.0);
    let reach = bounds.size().max_element() * 0.5;
    let at = cx.view.project(centre)?;
    let edge = cx.view.project(centre + cx.view.right * reach)?;
    Some(Target { centre, reach, at, radius: at.distance(edge) })
}

/// The tools over a live mirror copy: a shape drawn over the copy cuts the
/// one mesh, so the object on the other side shows the same hole, cut or mask.
fn over_the_copy(tool: &mut Region, cx: &mut Ctx) {
    let Some(copy) = mirrored_copy(cx) else { return };
    let real = cx.view;
    let r = copy.radius;
    match tool.action() {
        Action::Hide => {
            tool.set_shape(cx, Shape::Box);
            drag(tool, cx, &[copy.at + Vec2::new(-1.05, -0.9) * r, copy.at + Vec2::new(0.1, 0.25) * r]);
            tool.finish(cx, &real);
        }
        Action::Mask => {
            tool.set_shape(cx, Shape::Lasso);
            drag(tool, cx, &drawn_loop(copy.at + Vec2::new(-0.2, -0.1) * r, r * 0.55, 1.0));
            tool.finish(cx, &real);
        }
        Action::Trim | Action::Split => {
            // Seen from the side, as the other cutting demos are, so the cut face shows the real camera.
            tool.set_shape(cx, Shape::Lasso);
            let side = side_view(cx, &copy, SIDE_TURN);
            let centre = side.size * 0.5 + Vec2::new(-0.85, -0.25) * r;
            drag(tool, cx, &drawn_loop(centre, r * 0.6, 1.0));
            tool.finish(cx, &side);
            tool.settle(cx);
            if tool.action() == Action::Split {
                move_piece_aside(tool, cx, Vec3::NEG_Y * copy.reach * 1.5);
            }
        }
    }
}

/// The region tools. Mask: a lasso mid-draw (`box`: a box mid-drag;
/// `masked`: the lasso applied). Hide: half the model hidden from the side,
/// so the inside of what is left shows (`lasso`: a hole through the middle;
/// `drag`: a box mid-drag). Trim: a straight cut showing its flat face
/// (`lasso`: a bite showing a curved one; `drag`: the line mid-drag). Split:
/// the model cut in two with the piece moved aside (`drag`: the line
/// mid-drag). All four: `mirror` draws the shape over the live mirror copy of
/// the object, which is put on the other side of the world's middle first.
pub fn region(tool: &mut Region, cx: &mut Ctx, what: &str) {
    if what == "mirror" {
        return over_the_copy(tool, cx);
    }
    let Some(target) = target(cx) else { return };
    let real = cx.view;
    match (tool.action(), what) {
        (Action::Mask, "box") | (Action::Hide, "drag") => {
            tool.set_shape(cx, Shape::Box);
            let r = target.radius;
            drag(tool, cx, &[target.at + Vec2::new(-1.15, -0.95) * r, target.at + Vec2::new(0.15, 0.35) * r]);
        }
        (Action::Hide, "lasso") => {
            tool.set_shape(cx, Shape::Lasso);
            drag(tool, cx, &drawn_loop(target.at, target.radius * 0.5, 1.0));
            tool.finish(cx, &real);
        }
        (Action::Hide, _) => {
            // The near half, as the second camera sees it: what is left is
            // open toward the real one.
            tool.set_shape(cx, Shape::Box);
            let side = side_view(cx, &target, SIDE_TURN);
            drag(tool, cx, &[Vec2::ZERO, Vec2::new(side.size.x * 0.5, side.size.y)]);
            tool.finish(cx, &side);
        }
        (Action::Mask, applied) => {
            tool.set_shape(cx, Shape::Lasso);
            let whole = applied == "masked";
            let centre = target.at + Vec2::new(-0.2, -0.15) * target.radius;
            drag(tool, cx, &drawn_loop(centre, target.radius * 0.55, if whole { 1.0 } else { 0.78 }));
            if whole {
                tool.finish(cx, &real);
            }
        }
        (Action::Trim | Action::Split, "drag") => {
            tool.set_shape(cx, Shape::Line);
            let r = target.radius;
            drag(tool, cx, &[target.at + Vec2::new(-0.5, 1.3) * r, target.at + Vec2::new(0.35, -1.25) * r]);
        }
        (Action::Trim, "lasso") => {
            tool.set_shape(cx, Shape::Lasso);
            let side = side_view(cx, &target, SIDE_TURN);
            let centre = side.size * 0.5 + Vec2::new(-0.85, -0.25) * target.radius;
            drag(tool, cx, &drawn_loop(centre, target.radius * 0.6, 1.0));
            tool.finish(cx, &side);
            tool.settle(cx);
        }
        (Action::Trim | Action::Split, _) => {
            tool.set_shape(cx, Shape::Line);
            let side = side_view(cx, &target, SIDE_TURN);
            drag(tool, cx, &upright_line(&side));
            tool.finish(cx, &side);
            tool.settle(cx);
            if tool.action() == Action::Split {
                // Sideways across the picture, clear of the half it came off.
                move_piece_aside(tool, cx, -real.right * target.reach * 1.1);
            }
        }
    }
}

/// Slides the piece a split just made away from the cut, so the two read as
/// separate objects and the face left on the first shows.
fn move_piece_aside(tool: &Region, cx: &mut Ctx, by: Vec3) {
    let Some(piece) = tool.last_piece else { return };
    let Some(placement) = cx.scene().info(piece).map(|info| info.placement) else { return };
    let moved = Placement { position: placement.position + by, ..placement };
    cx.commit("Move Piece", vec![Command::Place { object: piece, placement: moved }]);
}

/// Extract: the preview shell on a masked patch of the selected object,
/// which is masked here if nothing on it is. `made` goes on to accept it.
pub fn extract(tool: &mut Extract, cx: &mut Ctx, what: &str) {
    let Some(target) = target(cx) else { return };
    let masked = cx.scene().active_mesh().is_some_and(|mesh| mesh.mask.iter().any(|&m| m > 0.0));
    if !masked {
        let centre = target.at + Vec2::new(-0.15, -0.2) * target.radius;
        let view = cx.view;
        if let Ok(region) = ScreenRegion::lasso(&drawn_loop(centre, target.radius * 0.5, 1.0)) {
            // A problem here shows up as the tool's own "nothing masked".
            let _ = mask::apply(cx, &view, &region, 1.0, false, &mut Vec::new());
        }
    }
    tool.refresh(cx);
    tool.settle(cx);
    if what == "made" {
        tool.accept(cx);
        tool.settle(cx);
    }
}
