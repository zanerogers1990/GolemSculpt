//! The Pose brush in the interface: its settings in the brush panel, and
//! the preview drawn over the model, which shows where the limb will bend
//! before the pen comes down and how the chain stands during a drag.

use egui::{Color32, LayerId, Pos2, Stroke, Ui};

use super::theme::{self, ACCENT, ButtonStyle};
use crate::sculpt::{PoseMode, PoseSettings, Settings};

/// The preview's lines and joints. White on a dark edge, as the brush ring
/// is, so it shows on any material.
const LINE: Color32 = Color32::from_rgba_premultiplied(242, 242, 242, 242);
const EDGE: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 150);

/// Draws each chain: a line from the grabbed point through its joints, a dot
/// on every joint that bends, and a ringed dot on the last one, which is the
/// end that stays put. `chains` are in the interface's points.
///
/// Drawn over the model rather than in it: a chain runs through the inside
/// of a limb, where the Unity app's first, depth-tested line was reported as
/// "maybe a slight outline".
pub fn overlay(ctx: &egui::Context, chains: &[Vec<Pos2>]) {
    if chains.is_empty() {
        return;
    }
    let p = ctx.layer_painter(LayerId::background());
    for chain in chains {
        if chain.len() < 2 {
            continue;
        }
        p.line(chain.clone(), Stroke::new(4.5, EDGE));
        p.line(chain.clone(), Stroke::new(2.0, LINE));
        let last = chain.len() - 1;
        for (i, &at) in chain.iter().enumerate() {
            match i {
                // The grabbed point: the brush ring's own dot marks it
                // before a press, and after it this one stays with the limb.
                0 => {
                    p.circle_filled(at, 3.5, EDGE);
                    p.circle_filled(at, 2.25, LINE);
                }
                i if i == last => {
                    p.circle_stroke(at, 8.0, Stroke::new(4.0, EDGE));
                    p.circle_stroke(at, 8.0, Stroke::new(1.75, LINE));
                    p.circle_filled(at, 4.5, EDGE);
                    p.circle_filled(at, 3.25, LINE);
                }
                _ => {
                    p.circle_filled(at, 6.0, EDGE);
                    p.circle_filled(at, 4.5, LINE);
                }
            }
        }
    }
}

/// The Pose brush's part of the brush settings card.
pub fn settings(ui: &mut Ui, s: &mut Settings, tips: bool) {
    let pose = &mut s.pose;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = theme::row_width(ui, PoseMode::ALL.len(), 4.0);
        for mode in PoseMode::ALL {
            let style = ButtonStyle { active: (pose.mode == mode).then_some(ACCENT), ..Default::default() };
            if theme::tip(theme::button(ui, w, mode.name(), style), tips, mode.help()).clicked() {
                pose.mode = mode;
            }
        }
    });
    theme::hint(ui, pose.mode.help());

    if pose.mode == PoseMode::Bend {
        let mut joints = pose.joints as f32;
        let most = PoseSettings::MAX_JOINTS as f32;
        let slider = theme::slider(ui, "Segments (Joints)", &mut joints, 1.0..=most, |v| format!("{:.0}", v.round()));
        let help = "How many joints the limb bends at. One turns it at a single joint. More make a chain of \
                    joints back along the limb, which reaches for the pen like an arm while its far end stays put.";
        if theme::tip(slider, tips, help).changed() {
            pose.joints = joints.round().clamp(1.0, most) as u32;
        }
    }
    theme::tip(
        theme::slider(ui, "Joint Offset", &mut pose.offset, 0.0..=PoseSettings::MAX_OFFSET, |v| format!("{v:.2}")),
        tips,
        "Moves the joint further up the limb, past the brush ring, by this many brush sizes. \
         For a hand or a face, where a brush small enough to pick out one part would put the joint too close to it.",
    );
    theme::tip(
        theme::slider(ui, "Rigidity (Soft - Rigid)", &mut pose.rigidity, 0.0..=1.0, |v| format!("{v:.2}")),
        tips,
        "How sharply the limb bends at a joint: low bends in a long soft curve, high like a hinge.",
    );
    theme::hint(
        ui,
        "The white line shows where it will bend before you press: a dot on each joint, and a ring round the one that \
         stays put. The brush size sets how far up the limb the joint is.",
    );
}
