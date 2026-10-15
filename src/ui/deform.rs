//! The Deform card, under the Transform card while the gizmo is up: the
//! eight deformers, the settings of the one picked, and Apply and Cancel.
//!
//! The card edits the settings in place (`Card::params`) and the app shows
//! the result on the model every frame they change (app/deform.rs), so a
//! slider dragged is a deformation seen as it is dragged. Every value has a
//! number box beside its slider, to type it exactly. Nothing is kept until
//! Apply (or Enter); Cancel (or Esc) puts the model back as it was.

use egui::{Color32, Ui, vec2};

use super::assets::Assets;
use super::theme::{self, ACCENT, ButtonStyle};
use crate::deform::{Anchor, Kind, Params, lattice};
use crate::doc::{Action, Doc};

/// The card's colour.
const ACCENT_DEFORM: Color32 = Color32::from_rgb(230, 170, 90);

/// What the card holds between frames.
#[derive(Clone, Debug, Default)]
pub struct Card {
    /// Whether a deformer is picked: the gizmo then shows the deformer's
    /// handles in place of its arrows, and the model shows what it does.
    pub on: bool,
    pub params: Params,
    /// Set by the app every frame: a deformation is on the model that has
    /// not been applied yet.
    pub pending: bool,
}

/// What the card can ask of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// Bring this deformer up (whatever is on the model is applied first),
    /// or with `None` put the deformers away.
    Pick(Option<Kind>),
    /// Keep what is on the model as one undo step.
    Apply,
    /// Put the model back as it was before the deformation.
    Cancel,
    /// Deform with these settings and keep it, in one go: what the Claude
    /// bridge asks for.
    Run(Box<Params>),
}

/// A slider with a number box beside it, both on one value: drag the one,
/// or double-click the other and type. `scale` is what the number box shows
/// the value times (100 for a percentage).
fn number(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    scale: f32,
    unit: &str,
    tip: &str,
    tips: bool,
) {
    let (min, max) = (*range.start(), *range.end());
    ui.horizontal(|ui| {
        let box_width = 62.0;
        let slider_width = (ui.available_width() - box_width - 6.0).max(60.0);
        ui.allocate_ui(vec2(slider_width, theme::SLIDER_HEIGHT), |ui| {
            let r = theme::slider(ui, label, value, min..=max, |v| format!("{:.0}{unit}", v * scale));
            theme::tip(r, tips, tip);
        });
        ui.vertical(|ui| {
            ui.add_space(theme::SLIDER_HEIGHT - 24.0);
            let mut shown = *value * scale;
            let drag = egui::DragValue::new(&mut shown)
                .speed(((max - min) * scale / 400.0).max(0.01))
                .range(min * scale..=max * scale)
                .max_decimals(2)
                .suffix(unit);
            let r = ui.add_sized([box_width, 22.0], drag);
            if r.changed() {
                *value = shown / scale;
            }
            theme::tip(r, tips, "Double-click to type a value, or drag left and right.");
        });
    });
}

/// The deformer's own values.
fn values(ui: &mut Ui, p: &mut Params, tips: bool) {
    let axis = ["X", "Y", "Z"][p.axis];
    match p.kind {
        Kind::Bend => {
            number(
                ui,
                "Angle",
                &mut p.angle,
                -360.0..=360.0,
                1.0,
                "°",
                "How far the shape bends round over the range.",
                tips,
            );
            let next = ["Y", "Z", "X"][p.axis];
            let help = format!("Which way it bends, turning round the {axis} axis. 0° bends toward {next}.");
            number(ui, "Direction", &mut p.direction, -180.0..=180.0, 1.0, "°", &help, tips);
        }
        Kind::BendArc => {
            theme::hint(
                ui,
                "Drag the yellow tip handle where the end should go, or type how far it has moved (as a share of the range's length).",
            );
            for (i, name) in ["Tip X", "Tip Y", "Tip Z"].into_iter().enumerate() {
                let help = format!("How far the tip has been moved along the gizmo's {}.", ["X", "Y", "Z"][i]);
                number(ui, name, &mut p.tip[i], -2.0..=2.0, 100.0, "%", &help, tips);
            }
        }
        Kind::Twist => {
            number(
                ui,
                "Angle",
                &mut p.angle,
                -720.0..=720.0,
                1.0,
                "°",
                "How far the far end turns about the axis.",
                tips,
            );
        }
        Kind::Taper => {
            number(
                ui,
                "Start Size",
                &mut p.scales[0],
                0.0..=3.0,
                100.0,
                "%",
                "How big across the shape is at the start of the range.",
                tips,
            );
            number(
                ui,
                "End Size",
                &mut p.scales[1],
                0.0..=3.0,
                100.0,
                "%",
                "How big across the shape is at the end of the range.",
                tips,
            );
        }
        Kind::Flatten => {
            number(
                ui,
                "Amount",
                &mut p.flatten,
                0.0..=1.0,
                100.0,
                "%",
                "How much flatter: at 100% the range is pressed flat.",
                tips,
            );
        }
        Kind::Inflate => {
            number(
                ui,
                "Amount",
                &mut p.inflate,
                -0.25..=0.25,
                100.0,
                "%",
                "How far the surface moves out (or in), as a share of the model's size.",
                tips,
            );
        }
        Kind::Stretch => {
            number(
                ui,
                "Length",
                &mut p.stretch,
                0.25..=4.0,
                100.0,
                "%",
                "How long the range becomes: 200% is twice as long.",
                tips,
            );
            let r = theme::checkbox(ui, &mut p.keep_volume, "Keep Volume", ACCENT());
            theme::tip(r, tips, "Thin the shape as it gets longer and thicken it as it gets shorter, as clay does.");
        }
        Kind::Lattice => lattice_values(ui, p, tips),
    }
}

/// The cage's size, and the number boxes of the point picked.
fn lattice_values(ui: &mut Ui, p: &mut Params, tips: bool) {
    theme::hint(ui, "Drag a white point of the cage on the model. The clay inside follows it smoothly.");
    let mut count = p.lattice.count;
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        for (i, n) in count.iter_mut().enumerate() {
            let drag = egui::DragValue::new(n)
                .range(lattice::MIN_POINTS..=lattice::MAX_POINTS)
                .prefix(format!("{} ", ["X", "Y", "Z"][i]));
            let r = ui.add_sized([w, 22.0], drag);
            theme::tip(r, tips, "How many points the cage has this way, 2 to 6. Changing it starts the cage afresh.");
        }
    });
    if count != p.lattice.count {
        p.lattice = p.lattice.with_count(count);
    }
    let picked = p.lattice.picked.min(p.lattice.len() - 1);
    let [i, j, k] = p.lattice.ijk(picked);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        if theme::button(ui, w, "Previous", ButtonStyle::default()).clicked() {
            p.lattice.picked = (picked + p.lattice.len() - 1) % p.lattice.len();
        }
        ui.add_sized([w, 26.0], egui::Label::new(format!("Point {} {} {}", i + 1, j + 1, k + 1)));
        if theme::button(ui, w, "Next", ButtonStyle::default()).clicked() {
            p.lattice.picked = (picked + 1) % p.lattice.len();
        }
    });
    let picked = p.lattice.picked.min(p.lattice.len() - 1);
    for c in 0..3 {
        let help = format!(
            "How far the picked point has been moved along the gizmo's {}, as a share of the cage.",
            ["X", "Y", "Z"][c]
        );
        let mut value = p.lattice.moved[picked][c];
        number(ui, &format!("Point {}", ["X", "Y", "Z"][c]), &mut value, -1.0..=1.0, 100.0, "%", &help, tips);
        p.lattice.moved[picked][c] = value;
    }
    if theme::button(ui, ui.available_width(), "Reset Cage", ButtonStyle::default()).clicked() {
        p.lattice.rest();
    }
}

/// The card. Only shown while the Transform tool is up.
pub fn card(ui: &mut Ui, doc: &mut Doc, assets: &Assets, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let card = &mut doc.opts.deform;
    let summary = if card.on { card.params.kind.name() } else { "" };
    theme::category(ui, "Deform", theme::lit(ACCENT_DEFORM), summary, true, |ui| {
        theme::about(
            ui,
            "Bend, twist or reshape the whole selected object along one of the gizmo's axes. Masked parts stay where they are.",
        );
        const PER_ROW: usize = 4;
        let w = theme::row_width(ui, PER_ROW, 4.0);
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for row in Kind::ALL.chunks(PER_ROW) {
            ui.horizontal(|ui| {
                for &kind in row {
                    let picked = card.on && card.params.kind == kind;
                    let r = theme::tile(
                        ui,
                        vec2(w, 48.0),
                        assets.icon(kind.icon()),
                        kind.name(),
                        "",
                        picked.then_some(ACCENT()),
                    );
                    let text = format!("{}\n{}", kind.name(), kind.tip());
                    if theme::tip(r, tips, &text).clicked() {
                        actions.push(Action::Deform(Ask::Pick(if picked { None } else { Some(kind) })));
                    }
                }
            });
        }
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        if !card.on {
            theme::hint(ui, "Pick one to see its handles on the model.");
            return;
        }
        let p = &mut card.params;

        // Along which of the gizmo's axes.
        theme::heading(ui, "Along");
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 3, 6.0);
            for (i, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                let style = ButtonStyle { active: (p.axis == i).then_some(theme::AXIS[i]), ..Default::default() };
                let help = format!("Work along the gizmo's {name} axis (its {} arrow).", ["red", "green", "blue"][i]);
                if theme::tip(theme::button(ui, w, name, style), tips, &help).clicked() {
                    p.axis = i;
                }
            }
        });
        values(ui, p, tips);
        if p.kind.anchors() {
            theme::heading(ui, "Stays Put");
            ui.horizontal(|ui| {
                let anchors: &[Anchor] =
                    if p.kind == Kind::BendArc { &[Anchor::Start, Anchor::End] } else { &Anchor::ALL };
                let w = theme::row_width(ui, anchors.len(), 6.0);
                for &anchor in anchors {
                    let style = ButtonStyle { active: (p.anchor == anchor).then_some(ACCENT()), ..Default::default() };
                    let help = match anchor {
                        Anchor::Start => "The start of the range stays where it is; the far end moves.",
                        Anchor::Middle => "The middle of the range stays where it is; both ends move alike.",
                        Anchor::End => "The end of the range stays where it is; the start moves.",
                    };
                    if theme::tip(theme::button(ui, w, anchor.name(), style), tips, help).clicked() {
                        p.anchor = anchor;
                    }
                }
            });
        }
        theme::heading(ui, "Range");
        number(
            ui,
            "Start",
            &mut p.range[0],
            0.0..=1.0,
            100.0,
            "%",
            "Where along the axis the deformation starts. Or drag the first round handle on the axis.",
            tips,
        );
        number(
            ui,
            "End",
            &mut p.range[1],
            0.0..=1.0,
            100.0,
            "%",
            "Where along the axis it ends. Or drag the second round handle.",
            tips,
        );
        if p.range[0] > p.range[1] {
            p.range.swap(0, 1);
        }
        if p.kind != Kind::Lattice {
            number(
                ui,
                "Softness",
                &mut p.softness,
                0.0..=1.0,
                100.0,
                "%",
                "How gently the deformation fades in and out at the ends of the range: 0% starts it sharply.",
                tips,
            );
        }
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(card.pending, |ui| {
                let style = ButtonStyle { active: Some(ACCENT()), badge: "Enter", ..Default::default() };
                if theme::tip(
                    theme::button(ui, w, "Apply", style),
                    tips,
                    "Keep the deformation. Undo (Z) takes it back.",
                )
                .clicked()
                {
                    actions.push(Action::Deform(Ask::Apply));
                }
                let style = ButtonStyle { badge: "Esc", ..Default::default() };
                if theme::tip(theme::button(ui, w, "Cancel", style), tips, "Put the model back as it was.").clicked() {
                    actions.push(Action::Deform(Ask::Cancel));
                }
            });
        });
        let style = ButtonStyle::default();
        if theme::tip(
            theme::button(ui, ui.available_width(), "Done", style),
            tips,
            "Keep what is on the model and put the deformers away: the gizmo's arrows come back.",
        )
        .clicked()
        {
            actions.push(Action::Deform(Ask::Pick(None)));
        }
    });
}
