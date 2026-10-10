//! The tool's pictures: each peg as a solid standing on the cut face and each
//! socket as a see-through shell sunk into the other half (the very rings the
//! builder will stack), and flat outlines over the viewport (the cut face's
//! rim and every key's socket mouth), so the joints can be seen and picked
//! from either half or with both halves shut.

use glam::{Mat4, Vec2, Vec3};
use sculpt_geom::TriMesh;
use sculpt_toolkit::{Color32, Ctx, Draw, Style};

use crate::builder;
use crate::pair::Pair;
use crate::planner::{JointPlan, PlannedKey};
use crate::tool::Keys;
use crate::types::{KeyFeature, KeyRole};

pub(crate) const PEG: Color32 = Color32::from_rgb(250, 158, 64);
const BAD: Color32 = Color32::from_rgb(242, 64, 56);
const RIM: Color32 = Color32::from_rgba_premultiplied(115, 115, 115, 115);
const MOUTH: Color32 = Color32::from_rgb(115, 204, 255);
const PEG_LINE: Color32 = Color32::from_rgb(255, 184, 89);
const HOVER: Color32 = Color32::WHITE;
const GHOST: Color32 = Color32::from_rgba_premultiplied(89, 89, 89, 89);
const DOT_EDGE: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 153);
const SOCKET: Color32 = Color32::from_rgba_premultiplied(49, 88, 110, 110);

/// One half's share of every joint, as one mesh in the world the joints were
/// planned in: the pegs that stand on it, or the sockets sunk into it, taking
/// the keys that fit or the ones that do not.
fn key_mesh(pair: &Pair, role: KeyRole, on_a: bool, valid: bool) -> TriMesh {
    let mut mesh = TriMesh::default();
    let here = |jp: &&JointPlan| (role == KeyRole::Peg) == (jp.peg_on_a == on_a);
    for jp in pair.plan.joints.iter().filter(here) {
        let Some(profile) = jp.valid_profile() else { continue };
        for key in jp.keys.iter().filter(|key| key.valid == valid) {
            let frame = jp.frame(key);
            let feature = KeyFeature { role, profile: profile.clone(), dimensions: jp.dimensions, frame, patch: 0 };
            builder::preview(&feature, jp.joint.edge, &mut mesh);
        }
    }
    // A socket's own faces look into it; seen from outside the half, as a
    // shell, it is lit like the solid it will take.
    if role == KeyRole::Hole {
        mesh.flip();
    }
    mesh
}

impl Keys {
    fn rebuild_pegs(&mut self) {
        self.pegs_stale = false;
        let Some(pair) = &self.pair else { return };
        for (side, on_a) in [true, false].into_iter().enumerate() {
            for (kind, valid) in [true, false].into_iter().enumerate() {
                self.pegs[side][kind].set(key_mesh(pair, KeyRole::Peg, on_a, valid));
            }
            self.sockets[side].set(key_mesh(pair, KeyRole::Hole, on_a, true));
        }
    }

    /// What the tool shows this frame, while it is up.
    pub(crate) fn draw(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        if self.editable().is_none() && self.job.is_none() {
            self.hover = None;
            return;
        }
        let Some(pair) = &self.pair else { return };
        // One half is left out of the picture so the cut face of the other
        // can be seen; the scene itself is not changed.
        let (show_a, show_b) = self.halves_shown();
        let (a, b) = (pair.a, pair.b);
        if !show_a {
            draw.hide(a);
        }
        if !show_b {
            draw.hide(b);
        }

        self.hover = match self.drag {
            Some(drag) => Some(crate::input::Hit { joint: drag.joint, plane: Vec2::ZERO, key: Some(drag.key) }),
            None if cx.hovering => self.pick(cx, cx.pointer, None),
            None => None,
        };
        // Hide-the-panels view shows the model and nothing else.
        if cx.clean_view {
            return;
        }
        if self.pegs_stale {
            self.rebuild_pegs();
        }
        let Some(pair) = &self.pair else { return };

        // Pegs show on the half that carries them, wherever that half is
        // now, and only while it is on screen, or they would float in the
        // air over the other half's face. Sockets likewise, drawn through
        // the half they will be sunk into.
        for (side, (on_a, shown)) in [(true, show_a), (false, show_b)].into_iter().enumerate() {
            let visible = cx.scene().info(if on_a { a } else { b }).is_some_and(|info| info.visible);
            if !shown || !visible {
                continue;
            }
            let at = [pair.display(cx.scene(), on_a)];
            draw.instances(&self.pegs[side][0], Style::solid(PEG).flat(), &at);
            draw.instances(&self.pegs[side][1], Style::solid(BAD).flat(), &at);
            draw.instances(&self.sockets[side], Style::shell(SOCKET).flat().on_top(), &at);
        }
        self.draw_overlay(cx, draw, pair);
    }

    fn draw_overlay(&self, cx: &Ctx, draw: &mut Draw, pair: &Pair) {
        for (j, jp) in pair.plan.joints.iter().enumerate() {
            let display = pair.display(cx.scene(), self.shown_side_is_a(jp));
            for rim in &pair.rims[j] {
                outline(cx, draw, &display, rim.iter().copied(), 1.2, RIM);
            }
            for (k, key) in jp.keys.iter().enumerate() {
                let hovered = self.hover.is_some_and(|hit| hit.joint == j && hit.key == Some(k));
                let mouth = if !key.valid {
                    BAD
                } else if hovered {
                    HOVER
                } else {
                    MOUTH
                };
                let line = if key.valid { PEG_LINE } else { BAD };
                let width = if hovered { 2.2 } else { 1.4 };
                key_outline(cx, draw, &display, jp, key, jp.dimensions.mouth_offset(), width, mouth);
                key_outline(cx, draw, &display, jp, key, 0.0, 1.0, line);
                if let Some(centre) = cx.view.project(display.transform_point3(jp.frame(key).origin)) {
                    draw.overlay.dot(centre, if hovered { 4.0 } else { 3.0 }, line, DOT_EDGE);
                }
            }
        }

        // Where a click would add a key.
        if let Some(hit) = self.hover.filter(|hit| hit.key.is_none() && self.drag.is_none()) {
            let jp = &pair.plan.joints[hit.joint];
            let ghost = PlannedKey::new(hit.plane, self.settings.rotation_degrees);
            let display = pair.display(cx.scene(), self.shown_side_is_a(jp));
            key_outline(cx, draw, &display, jp, &ghost, jp.dimensions.mouth_offset(), 1.0, GHOST);
        }
    }
}

/// A closed line through world points, less any behind the camera.
fn outline(cx: &Ctx, draw: &mut Draw, display: &Mat4, points: impl Iterator<Item = Vec3>, width: f32, color: Color32) {
    let screen: Vec<Vec2> = points.filter_map(|p| cx.view.project(display.transform_point3(p))).collect();
    if screen.len() > 2 {
        draw.overlay.polyline(&screen, width, color, true);
    }
}

/// A key's outline at `offset`, lying on the cut face.
#[allow(clippy::too_many_arguments)]
fn key_outline(
    cx: &Ctx,
    draw: &mut Draw,
    display: &Mat4,
    jp: &JointPlan,
    key: &PlannedKey,
    offset: f32,
    width: f32,
    color: Color32,
) {
    let Some(profile) = jp.valid_profile() else { return };
    let frame = jp.frame(key);
    outline(cx, draw, display, profile.outline(offset).into_iter().map(|p| frame.point(p, 0.0)), width, color);
}
