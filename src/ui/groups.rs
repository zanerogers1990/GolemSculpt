//! The Face Groups card of the Sculpt panel's Mesh tab, and the Stay in Group
//! tick of the brush page.
//!
//! A face group is a region of the surface with a colour of its own
//! (`mesh/groups.rs`). The card makes them (from the mask, from what shows,
//! from the model's separate parts or its sharp edges) and uses them: hide
//! everything but one, hide one, mask one. Like every card it only asks:
//! each button pushes a command the app carries out after the interface pass
//! (`app/groups.rs`), one undo step each.
//!
//! **The card lists no groups.** A model can have hundreds, and a list of
//! colour swatches says nothing about where on the model each is; the model
//! does. So a group is picked by clicking it on the model, and the card
//! shows the one picked as a swatch of its colour with its size. Ctrl + Shift
//! + click on the model picks a group and isolates it in one go; while a tool
//! that uses Ctrl and Shift itself is up, the Pick button arms the next
//! click instead.

use egui::Color32;

use super::theme::{self, ACCENT, ButtonStyle};
use super::{fmt_count, tip};
use crate::doc::{Action, Doc};
use crate::mesh::groups;
use crate::sculpt::BrushSettings;

/// The card's colour on the panel.
const TINT: Color32 = Color32::from_rgb(120, 200, 190);

/// The angle Groups by Angle starts at: sharp enough to split a hard model
/// at its edges, soft enough to leave a rounded form in one piece.
const ANGLE: f32 = 35.0;

/// What the next click on the model does, when a button of the card armed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Armed {
    /// Picks the group under the pointer.
    Pick,
    /// Merges the group picked into the group under the pointer.
    Merge,
}

/// The group last picked on the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Picked {
    /// The scene's number for the mesh it is a group of.
    pub mesh: u64,
    pub group: u16,
    /// How many faces it has, and the state of the mesh that was counted on
    /// (`Mesh::uid` and `Mesh::groups_version`): counted again when either moves.
    pub faces: usize,
    pub counted: (u64, u64),
}

/// What the card keeps between frames. None of it is saved: it is how the
/// card is being used just now, not part of the model.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// Show Group Colours: tint every face with the colour of its group.
    pub colours: bool,
    /// Group Masked leaves the mask on instead of clearing it.
    pub keep_mask: bool,
    /// Groups by Angle splits where the surface turns by more than this, in degrees.
    pub angle: f32,
    pub picked: Option<Picked>,
    pub armed: Option<Armed>,
}

impl Default for Card {
    fn default() -> Card {
        Card { colours: false, keep_mask: false, angle: ANGLE, picked: None, armed: None }
    }
}

/// What the card can ask for. Everything that names no group works on the
/// group picked on the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ask {
    /// The masked surface becomes a new group.
    GroupMasked,
    /// What shows becomes a new group.
    GroupVisible,
    /// Each connected part becomes a group of its own.
    Auto,
    /// Split into groups at edges sharper than the card's angle.
    ByAngle,
    /// Take every group away.
    Clear,
    /// Arm the next click on the model, or with `None` let it go.
    Arm(Option<Armed>),
    /// Hide everything but the group picked; again, show everything.
    Isolate,
    /// Hide the group picked.
    Hide,
    /// Mask the group picked and unmask the rest, or with `others` the
    /// other way about.
    Mask { others: bool },
}

/// "No groups", "1 group", "312 groups".
pub fn counted(count: usize) -> String {
    match count {
        0 => "No groups".to_owned(),
        1 => "1 group".to_owned(),
        many => format!("{} groups", fmt_count(many)),
    }
}

fn swatch_colour(group: u16) -> Color32 {
    if group == 0 {
        return Color32::from_gray(150);
    }
    let [r, g, b] = groups::colour(group).map(|x| (x * 255.0).round() as u8);
    Color32::from_rgb(r, g, b)
}

pub fn card(ui: &mut egui::Ui, doc: &mut Doc, actions: &mut Vec<Action>) {
    let tips = doc.opts.tooltips;
    let count = doc.mesh.group_count();
    let summary = if count == 0 { String::new() } else { counted(count) };
    theme::category(ui, "Face Groups", TINT, &summary, false, |ui| {
        theme::hint(
            ui,
            "Regions of the surface, each with a colour of its own, to hide, mask and sculpt one at a time. Make one \
             from the mask, from what shows, or let the model's own parts and edges split it.",
        );
        let help = "Tints every face with the colour of its group, over whatever material is showing. A group keeps \
                    its colour for good; surface in no group keeps the material's own colour.";
        tip(theme::checkbox(ui, &mut doc.opts.groups.colours, "Show Group Colours", ACCENT()), tips, help);
        ui.add_space(4.0);
        making(ui, doc, tips, actions);
        ui.add_space(6.0);
        using(ui, doc, tips, actions);
        ui.add_space(6.0);
        ui.add_enabled_ui(count > 0, |ui| {
            let help = "Takes every group off this object. Its shape does not change. Undo (Z) brings them back.";
            if tip(theme::button(ui, ui.available_width(), "Clear Groups", ButtonStyle::default()), tips, help)
                .clicked()
            {
                actions.push(Action::Groups(Ask::Clear));
            }
        });
        theme::hint(
            ui,
            "Stay in Group, on the Brush tab, keeps a brush inside the group its stroke begins on. Groups are saved \
             with the scene and written to OBJ files.",
        );
    });
}

/// Group Masked and its tick, Group Visible, Auto Groups, Groups by Angle.
fn making(ui: &mut egui::Ui, doc: &mut Doc, tips: bool, actions: &mut Vec<Action>) {
    let ask = |ui: &mut egui::Ui, width: f32, label: &str, help: &str, ask: Ask, actions: &mut Vec<Action>| {
        if tip(theme::button(ui, width, label, ButtonStyle::default()), tips, help).clicked() {
            actions.push(Action::Groups(ask));
        }
    };
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let help = "The masked surface becomes a new group, and the mask is cleared. Mask a part with the mask brush \
                    or a mask tool, then press this.";
        ask(ui, w, "Group Masked", help, Ask::GroupMasked, actions);
        let help = "Everything that shows becomes a new group: hide what should stay out first. With nothing hidden \
                    the whole object becomes one group.";
        ask(ui, w, "Group Visible", help, Ask::GroupVisible, actions);
    });
    let help = "Group Masked leaves the mask as it is instead of clearing it.";
    tip(theme::checkbox(ui, &mut doc.opts.groups.keep_mask, "Keep the mask", ACCENT()), tips, help);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let help = "Each separate part of the object becomes a group of its own: the parts a Join brought together, \
                    or shells that only touch. One connected piece is one group.";
        ask(ui, w, "Auto Groups", help, Ask::Auto, actions);
        let help = "Splits the object into groups along its sharp edges, where the surface turns by more than the \
                    angle below. For hard shapes; on a soft sculpt a low angle makes a great many small groups.";
        ask(ui, w, "Groups by Angle", help, Ask::ByAngle, actions);
    });
    theme::slider(ui, "Angle", &mut doc.opts.groups.angle, 5.0..=90.0, |v| format!("{v:.0}\u{00b0}"));
}

/// The group picked, and what can be done with it.
fn using(ui: &mut egui::Ui, doc: &mut Doc, tips: bool, actions: &mut Vec<Action>) {
    let card = &doc.opts.groups;
    let picked = card.picked.filter(|picked| picked.mesh == doc.scene.active_mesh());
    let armed = card.armed;
    picked_line(ui, picked, armed);
    let arm = |ui: &mut egui::Ui, width: f32, label: &str, help: &str, what: Armed, actions: &mut Vec<Action>| {
        let lit = armed == Some(what);
        let style = ButtonStyle { active: lit.then(ACCENT), ..Default::default() };
        if tip(theme::button(ui, width, label, style), tips, help).clicked() {
            actions.push(Action::Groups(Ask::Arm((!lit).then_some(what))));
        }
    };
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let help = "Then click a group on the model to pick it, for the buttons below. Ctrl + Shift + click on the \
                    model picks a group and isolates it at once, without this button.";
        arm(ui, w, "Pick", help, Armed::Pick, actions);
        ui.add_enabled_ui(picked.is_some(), |ui| {
            let help = "Then click another group on the model: the group picked becomes part of it.";
            arm(ui, w, "Merge Into...", help, Armed::Merge, actions);
        });
    });
    let ask = |ui: &mut egui::Ui, width: f32, label: &str, help: &str, ask: Ask, actions: &mut Vec<Action>| {
        if tip(theme::button(ui, width, label, ButtonStyle::default()), tips, help).clicked() {
            actions.push(Action::Groups(ask));
        }
    };
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        ui.add_enabled_ui(picked.is_some(), |ui| {
            let help = "Hides everything but the group picked, so it can be worked on alone. Press again, or Show All, \
                        to bring the rest back. Ctrl + Shift + click on the model does the same.";
            ask(ui, w, "Isolate", help, Ask::Isolate, actions);
            let help = "Hides the group picked. Show All brings it back.";
            ask(ui, w, "Hide Group", help, Ask::Hide, actions);
        });
        ui.add_enabled_ui(doc.mesh.any_hidden(), |ui| {
            let help = "Shows every hidden face of this object again.";
            if tip(theme::button(ui, w, "Show All", ButtonStyle::default()), tips, help).clicked() {
                actions.push(Action::ShowAllFaces);
            }
        });
    });
    ui.add_enabled_ui(picked.is_some(), |ui| {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let help = "Masks the group picked and unmasks everything else: brushes then leave it alone.";
            ask(ui, w, "Mask Group", help, Ask::Mask { others: false }, actions);
            let help = "Masks everything but the group picked, so brushes work on it alone. The mask can be blurred \
                        for a soft edge, which Stay in Group cannot give.";
            ask(ui, w, "Mask All But", help, Ask::Mask { others: true }, actions);
        });
    });
}

/// One line saying which group is picked, with a swatch of its colour, or
/// what to do to pick one.
fn picked_line(ui: &mut egui::Ui, picked: Option<Picked>, armed: Option<Armed>) {
    let waiting = match armed {
        Some(Armed::Pick) => Some("Click a group on the model."),
        Some(Armed::Merge) => Some("Click the group to merge it into."),
        None => None,
    };
    ui.horizontal(|ui| {
        if let Some(picked) = picked {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, swatch_colour(picked.group));
        }
        let text = match (waiting, picked) {
            (Some(waiting), _) => waiting.to_owned(),
            (None, Some(picked)) if picked.group == 0 => {
                format!("Picked: the surface in no group, {} faces", fmt_count(picked.faces))
            }
            (None, Some(picked)) => format!("Picked: a group of {} faces", fmt_count(picked.faces)),
            (None, None) => "No group picked: Ctrl + Shift + click one on the model, or press Pick.".to_owned(),
        };
        let colour = if waiting.is_some() { theme::KEY } else { ui.visuals().text_color() };
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(12.0)).color(colour)).wrap());
    });
}

/// The Stay in Group tick of the brush page, under Front Facing Only.
pub fn stay_in_group(ui: &mut egui::Ui, brush: &mut BrushSettings, tips: bool) {
    let label = if brush.stay_in_group { "Stay in Group" } else { "Stay in Group (Off)" };
    let help = "A stroke changes only the face group it begins on, and stops dead at the group's border. With \
                symmetry, the stroke is kept to the groups its sides begin on. Does nothing on an object without \
                groups (Mesh tab, Face Groups).";
    tip(theme::checkbox(ui, &mut brush.stay_in_group, label, ACCENT()), tips, help);
}
