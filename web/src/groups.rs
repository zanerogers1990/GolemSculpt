//! Face groups on the web: the Face Groups card on the Mesh tab, what its
//! commands do, the click on the model that picks and isolates a group,
//! Stay in Group, the groups' colours on the model, and the groups (and the
//! paint) following the model through Remesh and Decimate.
//!
//! What a group is, and what each command makes, is the desktop's
//! `mesh/groups.rs`, compiled here as it is. This file is the desktop's
//! `ui/groups.rs` (the card, word for word) and `app/groups.rs` (carrying
//! its commands out) put together for the web version, which has one undo
//! list (`Step::Groups`) and no toasts:
//!
//! - Every command is one undo step. Hide Group, Show All and the two mask
//!   commands are the app's ordinary hide, show and mask steps.
//! - Ctrl + Shift + click on a group isolates it and picks it for the
//!   card's buttons; again, or on the background, shows everything. While a
//!   tool or the gizmo is up the card's Pick button arms the next click.
//! - Stay in Group is asked as a stroke begins (`stay`), and the mesh holds
//!   the stroke to the groups under its start (`Mesh::stay_in_groups`).
//!
//! The one difference from the desktop is how the colours are drawn: the
//! desktop reads one colour per triangle from a storage buffer, which the
//! WebGL2 build cannot have, so here each vertex carries the colour of a
//! group it is on, and a triangle across a border blends the two.

use std::cell::RefCell;

use eframe::egui::{self, Color32, Modifiers, Ui};
use glam::{Vec2, Vec3};

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::ui::tip;
use super::{GolemApp, Mode, Step};
use crate::mesh::Mesh;
use crate::mesh::groups::{self, Carried, Fit, GroupsStep, Made, Source};
use crate::mesh::paint::transfer as paint_transfer;
use crate::sculpt::{PoseFrom, Sculptor};

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
    /// The object it is a group of.
    pub object: u64,
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
    GroupMasked,
    GroupVisible,
    Auto,
    ByAngle,
    Clear,
    /// Arm the next click on the model, or with `None` let it go.
    Arm(Option<Armed>),
    Isolate,
    Hide,
    ShowAll,
    Mask { others: bool },
}

thread_local! {
    /// The card's state; kept here, like the Layers card's, since nothing
    /// outside this file and the renderer's hook reads it.
    static CARD: RefCell<Card> = RefCell::new(Card::default());
    /// The groups' colours as last sent to the graphics card: what they were
    /// made from (`Mesh::uid`, groups version, vertex and corner counts,
    /// shown), so they are made again only when one of those moves.
    static SENT: RefCell<Option<(u64, u64, usize, usize, bool)>> = const { RefCell::new(None) };
    /// A mesh took another's place and could not take its groups or its
    /// paint: said on the status line by the next frame's look.
    static LEFT_BEHIND: RefCell<(bool, bool)> = const { RefCell::new((false, false)) };
}

fn card_get() -> Card {
    CARD.with(|c| c.borrow().clone())
}

fn card_edit<R>(f: impl FnOnce(&mut Card) -> R) -> R {
    CARD.with(|c| f(&mut c.borrow_mut()))
}

/// Whether Show Group Colours is on.
pub fn colours_on() -> bool {
    CARD.with(|c| c.borrow().colours)
}

/// A count with its thousands set apart: 100,444.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn fmt_count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.0}k", n as f32 / 1e3),
        _ => format!("{:.2}M", n as f32 / 1e6),
    }
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

// ------------------------------------------------------------ Stay in Group

/// Stay in Group, asked as a stroke is about to begin at pixel `pos`: if the
/// brush that will make the stroke has the tick, keeps the stroke to the
/// face groups under its start and under the start of each of its
/// symmetric copies. A stroke that then does not begin lets go ([`began`]).
pub(crate) fn stay(mesh: &mut Mesh, sculptor: &Sculptor, view: &crate::camera::View, pos: Vec2, smooth_held: bool) -> bool {
    if !stays(mesh, sculptor, smooth_held) {
        return false;
    }
    let (origin, dir) = view.ray(pos);
    let Some(hit) = mesh.raycast(origin, dir) else { return false };
    hold(mesh, sculptor, hit.pos, mesh.group_of(hit.tri));
    true
}

/// [`stay`] for a point of the model instead of a pixel: what the Pose
/// brush's preview asks, so that the chain it draws before the press is the
/// chain the press will make. The preview lets go again (`Mesh::leave_groups`).
pub(crate) fn stay_at(mesh: &mut Mesh, sculptor: &Sculptor, at: Vec3, smooth_held: bool) -> bool {
    if !stays(mesh, sculptor, smooth_held) {
        return false;
    }
    let Some(triangle) = showing_face_at(mesh, at) else { return false };
    hold(mesh, sculptor, at, mesh.group_of(triangle));
    true
}

/// Whether a stroke begun now would be kept to its group: the mesh has
/// groups, and the brush that would do the work has the tick. Never for
/// Pose working from face groups: the group is what it takes hold of
/// already, and the fence would stop it finding the rest of the model.
fn stays(mesh: &Mesh, sculptor: &Sculptor, smooth_held: bool) -> bool {
    let settings = &sculptor.settings;
    let brush = sculptor.effective_brush(smooth_held);
    let by_group = brush == crate::sculpt::Brush::Pose && !settings.mask_mode && settings.pose.from == PoseFrom::Group;
    mesh.has_groups() && !by_group && settings.in_use(brush, smooth_held && !settings.mask_mode).0.stay_in_group
}

/// The face that shows nearest `at`.
fn showing_face_at(mesh: &Mesh, at: Vec3) -> Option<u32> {
    let hidden = |t: u32| mesh.is_triangle_hidden(t);
    mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, at, f32::INFINITY, hidden).map(|(t, _)| t)
}

/// Keeps the stroke that starts at `at`, on a face of group `group`, to
/// that group and to the groups its symmetric copies start on.
fn hold(mesh: &mut Mesh, sculptor: &Sculptor, at: Vec3, group: u16) {
    let mut inside = vec![group];
    for op in sculptor.settings.symmetry_group().iter().skip(1) {
        inside.extend(showing_face_at(mesh, op.apply_point(at)).map(|t| mesh.group_of(t)));
    }
    mesh.stay_in_groups(&inside);
}

/// Whether the stroke [`stay`] was asked about began, passed through; one
/// that did not lets go of the groups it was to be kept to.
pub(crate) fn began(mesh: &mut Mesh, began: bool) -> bool {
    if !began {
        mesh.leave_groups();
    }
    began
}

// ------------------------------------------------------ following the model

/// `new`, about to take the place of `old` as the same shape built again
/// (Remesh, Decimate), given `old`'s groups by the nearest face and its
/// paint by the nearest point, unless it came with its own.
pub(crate) fn follow(old: &Mesh, new: &mut Mesh) {
    if old.has_groups() && !new.has_groups() && groups::carry(new, &[Source::of(old)], Fit::Whole) == Carried::Dropped {
        LEFT_BEHIND.with(|l| l.borrow_mut().0 = true);
    }
    if old.has_paint()
        && !new.has_paint()
        && paint_transfer::carry(new, &[Source::of(old)], Fit::Whole) == paint_transfer::Carried::Dropped
    {
        LEFT_BEHIND.with(|l| l.borrow_mut().1 = true);
    }
}

// ------------------------------------------------------------- the colours

/// The groups' colours for the graphics card, one `[r, g, b, strength]` a
/// vertex, when they need sending: Show Group Colours turned on or off, or
/// the groups or the triangles changed while it is on. With it off the
/// list is empty (nothing tints). `whole` is set when the card is about to
/// get the whole mesh, which always needs them again.
pub(crate) fn colours_to_send(mesh: &Mesh, whole: bool) -> Option<Vec<u8>> {
    let on = colours_on() && mesh.has_groups();
    let now = (mesh.uid, mesh.groups_version(), mesh.vertex_count(), mesh.indices.len(), on);
    let stale = SENT.with(|s| *s.borrow() != Some(now));
    if !stale && !whole {
        return None;
    }
    SENT.with(|s| *s.borrow_mut() = Some(now));
    let mut out = vec![0u8; mesh.vertex_count() * 4];
    if on {
        // Each vertex takes the colour of the last grouped face on it that is drawn.
        for (t, (&id, corners)) in mesh.group_ids().iter().zip(mesh.indices.chunks_exact(3)).enumerate() {
            if id == 0 || mesh.is_triangle_hidden(t as u32) {
                continue;
            }
            let [r, g, b] = groups::colour(id).map(|x| (x * 255.0).round() as u8);
            for &v in corners {
                let at = v as usize * 4;
                if let Some(slot) = out.get_mut(at..at + 4) {
                    slot.copy_from_slice(&[r, g, b, 255]);
                }
            }
        }
    }
    Some(out)
}

// ------------------------------------------------------- a stroke begins

impl GolemApp {
    /// Begins a stroke as the desktop's `Doc::begin_stroke` does: the sculpt
    /// layers swing with a Pose drag and ride every other brush, and a brush
    /// with Stay in Group is kept to the face group it begins on.
    pub(super) fn begin_stroke(
        &mut self,
        view: &crate::camera::View,
        sample: crate::sculpt::PointerSample,
        invert: bool,
        smooth_held: bool,
    ) -> bool {
        let posing = self.sculptor.effective_brush(smooth_held) == crate::sculpt::Brush::Pose;
        self.mesh.layer_ride = if posing { crate::mesh::layers::Ride::Turned } else { crate::mesh::layers::Ride::Still };
        stay(&mut self.mesh, &self.sculptor, view, sample.pos, smooth_held);
        let begun = self.sculptor.begin(&mut self.mesh, view, sample, invert, smooth_held);
        began(&mut self.mesh, begun)
    }
}

// --------------------------------------------------------------- the card

impl GolemApp {
    /// The Face Groups card (the desktop's `ui::groups::card`).
    pub(super) fn groups_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let count = self.mesh.group_count();
        let summary = if count == 0 { String::new() } else { counted(count) };
        let mut card = card_get();
        let picked = card.picked.filter(|p| p.object == self.active);
        let any_hidden = self.mesh.any_hidden();
        let mut asks: Vec<Ask> = Vec::new();
        theme::category(ui, "Face Groups", TINT, &summary, false, |ui| {
            theme::about(
                ui,
                "Regions of the surface, each with a colour of its own, to hide, mask and sculpt one at a time. Make one \
                 from the mask, from what shows, or let the model's own parts and edges split it.\n\n\
                 Stay in Group, on the Brush tab, keeps a brush inside the group its stroke begins on. Groups are saved \
                 with the scene and written to OBJ files.",
            );
            let help = "Tints every face with the colour of its group, over whatever material is showing. A group keeps \
                        its colour for good; surface in no group keeps the material's own colour.";
            tip(theme::checkbox(ui, &mut card.colours, "Show Group Colours", ACCENT()), tips, help);
            ui.add_space(4.0);
            making(ui, &mut card, tips, &mut asks);
            ui.add_space(6.0);
            using(ui, &card, picked, any_hidden, tips, &mut asks);
            ui.add_space(6.0);
            ui.add_enabled_ui(count > 0, |ui| {
                let help = "Takes every group off this object. Its shape does not change. Undo (Ctrl+Z) brings them back.";
                if tip(theme::button(ui, ui.available_width(), "Clear Groups", ButtonStyle::default()), tips, help)
                    .clicked()
                {
                    asks.push(Ask::Clear);
                }
            });
        });
        CARD.with(|c| *c.borrow_mut() = card);
        for ask in asks {
            self.groups_act(ask);
        }
    }

    /// The Stay in Group tick of the brush page, under Front Facing Only.
    pub(super) fn stay_in_group_tick(ui: &mut Ui, brush: &mut crate::sculpt::BrushSettings, tips: bool) {
        let label = if brush.stay_in_group { "Stay in Group" } else { "Stay in Group (Off)" };
        let help = "A stroke changes only the face group it begins on, and stops dead at the group's border. With \
                    symmetry, the stroke is kept to the groups its sides begin on. Does nothing on an object without \
                    groups (Mesh tab, Face Groups).";
        tip(theme::checkbox(ui, &mut brush.stay_in_group, label, ACCENT()), tips, help);
    }
}

/// Group Masked and its tick, Group Visible, Auto Groups, Groups by Angle.
fn making(ui: &mut Ui, card: &mut Card, tips: bool, asks: &mut Vec<Ask>) {
    let ask = |ui: &mut Ui, width: f32, label: &str, help: &str, ask: Ask, asks: &mut Vec<Ask>| {
        if tip(theme::button(ui, width, label, ButtonStyle::default()), tips, help).clicked() {
            asks.push(ask);
        }
    };
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let help = "The masked surface becomes a new group, and the mask is cleared. Mask a part with the mask brush \
                    or a mask tool, then press this.";
        ask(ui, w, "Group Masked", help, Ask::GroupMasked, asks);
        let help = "Everything that shows becomes a new group: hide what should stay out first. With nothing hidden \
                    the whole object becomes one group.";
        ask(ui, w, "Group Visible", help, Ask::GroupVisible, asks);
    });
    let help = "Group Masked leaves the mask as it is instead of clearing it.";
    tip(theme::checkbox(ui, &mut card.keep_mask, "Keep the mask", ACCENT()), tips, help);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let help = "Each separate part of the object becomes a group of its own: the parts a Join brought together, \
                    or shells that only touch. One connected piece is one group.";
        ask(ui, w, "Auto Groups", help, Ask::Auto, asks);
        let help = "Splits the object into groups along its sharp edges, where the surface turns by more than the \
                    angle below. For hard shapes; on a soft sculpt a low angle makes a great many small groups.";
        ask(ui, w, "Groups by Angle", help, Ask::ByAngle, asks);
    });
    theme::slider(ui, "Angle", &mut card.angle, 5.0..=90.0, |v| format!("{v:.0}\u{00b0}"));
}

/// The group picked, and what can be done with it.
fn using(ui: &mut Ui, card: &Card, picked: Option<Picked>, any_hidden: bool, tips: bool, asks: &mut Vec<Ask>) {
    let armed = card.armed;
    picked_line(ui, picked, armed);
    let arm = |ui: &mut Ui, width: f32, label: &str, help: &str, what: Armed, asks: &mut Vec<Ask>| {
        let lit = armed == Some(what);
        let style = ButtonStyle { active: lit.then(ACCENT), ..Default::default() };
        if tip(theme::button(ui, width, label, style), tips, help).clicked() {
            asks.push(Ask::Arm((!lit).then_some(what)));
        }
    };
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        let help = "Then click a group on the model to pick it, for the buttons below. Ctrl + Shift + click on the \
                    model picks a group and isolates it at once, without this button.";
        arm(ui, w, "Pick", help, Armed::Pick, asks);
        ui.add_enabled_ui(picked.is_some(), |ui| {
            let help = "Then click another group on the model: the group picked becomes part of it.";
            arm(ui, w, "Merge Into...", help, Armed::Merge, asks);
        });
    });
    let ask = |ui: &mut Ui, width: f32, label: &str, help: &str, ask: Ask, asks: &mut Vec<Ask>| {
        if tip(theme::button(ui, width, label, ButtonStyle::default()), tips, help).clicked() {
            asks.push(ask);
        }
    };
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        ui.add_enabled_ui(picked.is_some(), |ui| {
            let help = "Hides everything but the group picked, so it can be worked on alone. Press again, or Show All, \
                        to bring the rest back. Ctrl + Shift + click on the model does the same.";
            ask(ui, w, "Isolate", help, Ask::Isolate, asks);
            let help = "Hides the group picked. Show All brings it back.";
            ask(ui, w, "Hide Group", help, Ask::Hide, asks);
        });
        ui.add_enabled_ui(any_hidden, |ui| {
            let help = "Shows every hidden face of this object again.";
            ask(ui, w, "Show All", help, Ask::ShowAll, asks);
        });
    });
    ui.add_enabled_ui(picked.is_some(), |ui| {
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            let help = "Masks the group picked and unmasks everything else: brushes then leave it alone.";
            ask(ui, w, "Mask Group", help, Ask::Mask { others: false }, asks);
            let help = "Masks everything but the group picked, so brushes work on it alone. The mask can be blurred \
                        for a soft edge, which Stay in Group cannot give.";
            ask(ui, w, "Mask All But", help, Ask::Mask { others: true }, asks);
        });
    });
}

/// One line saying which group is picked, with a swatch of its colour, or
/// what to do to pick one.
fn picked_line(ui: &mut Ui, picked: Option<Picked>, armed: Option<Armed>) {
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

// -------------------------------------------------------------- the commands

impl GolemApp {
    /// The group picked on the model, if it is a group of the active object.
    pub(crate) fn group_picked(&self) -> Option<Picked> {
        card_get().picked.filter(|p| p.object == self.active)
    }

    /// The group picked, or a line saying that none is.
    pub(crate) fn group_picked_or_say(&mut self) -> Option<u16> {
        let picked = self.group_picked().map(|p| p.group);
        if picked.is_none() {
            self.status = "No group is picked: Ctrl + Shift + click one on the model, or press Pick and click it.".into();
        }
        picked
    }

    /// Makes `group` the group picked, counting its faces.
    fn group_pick(&mut self, group: u16) -> Picked {
        let mesh = &self.mesh;
        let faces = mesh.group_triangles(group, true).len();
        let picked = Picked { object: self.active, group, faces, counted: (mesh.uid, mesh.groups_version()) };
        card_edit(|c| c.picked = Some(picked));
        picked
    }

    /// Carries out a command of the Face Groups card.
    pub(super) fn groups_act(&mut self, ask: Ask) {
        if self.sculptor.is_active() {
            return;
        }
        self.layer_strength_done();
        // Any command but the arming itself lets an armed click go.
        if !matches!(ask, Ask::Arm(_)) {
            card_edit(|c| c.armed = None);
        }
        match ask {
            Ask::GroupMasked => self.group_masked(),
            Ask::GroupVisible => {
                let made = groups::visible(&self.mesh);
                self.groups_made(made, false, |made, count| {
                    format!(
                        "Group Visible: {} faces are a new group ({}).",
                        grouped(made.triangles),
                        counted(count).to_lowercase()
                    )
                });
            }
            Ask::Auto => {
                let made = groups::parts(&self.mesh);
                self.groups_made(made, false, |made, _| match made.groups {
                    1 => "Auto Groups: this object is one connected piece, so it is one group.".to_owned(),
                    parts => format!("Auto Groups: {parts} groups, one for each separate part."),
                });
            }
            Ask::ByAngle => {
                self.tidy_merged();
                let angle = card_get().angle;
                let made = groups::by_angle(&self.mesh, angle);
                self.groups_made(made, false, |made, _| match made.groups {
                    1 => format!("Groups by Angle: no edge of this object is sharper than {angle:.0}\u{00b0}, so it is one group."),
                    parts => format!(
                        "Groups by Angle: {} groups, split where the surface turns by more than {angle:.0}\u{00b0}.",
                        grouped(parts)
                    ),
                });
            }
            Ask::Clear => self.groups_clear(),
            Ask::Arm(armed) => self.groups_arm(armed),
            Ask::Isolate => {
                if let Some(group) = self.group_picked_or_say() {
                    self.group_isolate(group);
                }
            }
            Ask::Hide => {
                if let Some(group) = self.group_picked_or_say() {
                    let triangles = self.mesh.group_triangles(group, true);
                    if let Some(delta) = self.mesh.set_triangles_hidden(&triangles, true) {
                        self.faces_changed(delta);
                        self.status = "Hid the group picked. Show All brings it back.".into();
                    }
                }
            }
            Ask::ShowAll => self.show_all_faces_now(),
            Ask::Mask { others } => {
                if let Some(group) = self.group_picked_or_say() {
                    self.group_mask(group, others);
                }
            }
        }
    }

    /// What dynamic topology's merges leave inside the mesh, taken out as
    /// its own undo step first (the desktop's `tidy_merged`).
    fn tidy_merged(&mut self) {
        if let Some(tidy) = super::tidy(&self.mesh) {
            let before = std::mem::replace(&mut self.mesh, tidy);
            self.push_undo(Step::Mesh(self.active, Box::new(before)));
            self.upload_whole = true;
        }
    }

    /// Shows every hidden face again, one undo step.
    fn show_all_faces_now(&mut self) {
        if let Some(delta) = self.mesh.show_all() {
            self.faces_changed(delta);
            self.status = "Every hidden face shows again.".into();
        }
    }

    /// Records a change to the groups the mesh has already made.
    fn groups_step(&mut self, step: GroupsStep) {
        self.push_undo(Step::Groups(self.active, step));
        self.note_changed();
    }

    /// Puts a command's new group list on the mesh as one undo step and says
    /// what `said` makes of it, or says why nothing was made.
    fn groups_made(&mut self, made: Result<Made, String>, clear_mask: bool, said: impl FnOnce(&Made, usize) -> String) -> bool {
        let made = match made {
            Ok(made) => made,
            Err(why) => {
                self.status = why;
                return false;
            }
        };
        let first = !self.mesh.has_groups();
        let summary = Made { ids: Vec::new(), ..made };
        let before = self.mesh.set_group_ids(made.ids);
        let mask = if clear_mask { self.mesh.remap_mask(|_, _, _| 0.0) } else { None };
        self.groups_step(GroupsStep::groups(before, mask));
        let mut text = said(&summary, self.mesh.group_count());
        // The first groups of an object would be made unseen: show them.
        if first && !colours_on() {
            card_edit(|c| c.colours = true);
            text += " Show Group Colours is on now, so they can be seen.";
        }
        self.status = text;
        true
    }

    fn group_masked(&mut self) {
        let keep = card_get().keep_mask;
        let made = groups::masked(&self.mesh);
        let group = made.as_ref().map_or(0, |made| made.group);
        let done = self.groups_made(made, !keep, |made, count| {
            let mask = if keep { "The mask is as it was." } else { "The mask was cleared." };
            format!(
                "Group Masked: {} faces are a new group ({}). {mask}",
                grouped(made.triangles),
                counted(count).to_lowercase()
            )
        });
        if done {
            self.group_pick(group);
        }
    }

    fn groups_clear(&mut self) {
        let count = self.mesh.group_count();
        if count == 0 {
            self.status = "This object has no groups.".into();
            return;
        }
        let before = self.mesh.set_group_ids(Vec::new());
        self.groups_step(GroupsStep::groups(before, None));
        card_edit(|c| c.picked = None);
        self.status = format!("Cleared {}. Undo (Ctrl+Z) brings them back.", counted(count).to_lowercase());
    }

    fn groups_arm(&mut self, armed: Option<Armed>) {
        if armed == Some(Armed::Merge) && self.group_picked_or_say().is_none() {
            return;
        }
        card_edit(|c| c.armed = armed);
        match armed {
            Some(Armed::Pick) => self.status = "Click a group on the model to pick it.".into(),
            Some(Armed::Merge) => {
                self.status = "Click the group on the model that the picked group should become part of.".into()
            }
            None => {}
        }
    }

    /// Whether everything but `group` is hidden and all of `group` shows.
    fn group_is_isolated(&self, group: u16) -> bool {
        let mesh = &self.mesh;
        let Some(hidden) = mesh.hidden_triangles() else { return false };
        mesh.group_ids().iter().zip(hidden).all(|(&id, &hidden)| (id != group) == hidden)
    }

    /// Isolate: hides everything of the object but `group`; with that
    /// already so, shows everything. One undo step either way.
    fn group_isolate(&mut self, group: u16) {
        if self.group_is_isolated(group) {
            self.show_all_faces_now();
            return;
        }
        let flags: Vec<bool> = self.mesh.group_ids().iter().map(|&id| id != group).collect();
        if !self.mesh.has_groups() || !flags.contains(&true) {
            self.status = "That group is the whole object, so there is nothing else to hide.".into();
            return;
        }
        let before = self.mesh.hidden_triangles().map(<[bool]>::to_vec);
        self.mesh.restore_hidden_triangles(&flags);
        self.groups_step(GroupsStep::hidden(before.as_deref()));
        let shown = self.mesh.visible_triangle_count();
        self.status = format!(
            "Isolated a group of {} faces: the rest of the object is hidden. Ctrl + Shift + click it again, or \
             Show All, brings everything back.",
            grouped(shown)
        );
    }

    /// Mask Group (the group masked, the rest free) or, with `others`, Mask
    /// All But. The border's vertices are the group's. One mask step.
    fn group_mask(&mut self, group: u16, others: bool) {
        let on = self.mesh.group_vertices(group);
        let delta = self.mesh.remap_mask(|mesh, v, m| {
            // What is hidden keeps the mask it has.
            if mesh.is_vertex_hidden(v) {
                m
            } else if on[v as usize] != others {
                1.0
            } else {
                0.0
            }
        });
        let Some(delta) = delta else {
            self.status = "The mask is exactly that already.".into();
            return;
        };
        self.stroke_done(delta);
        self.status = if others {
            "Masked everything but the group picked: brushes work on it alone. Clear on the Masking card takes the mask off."
        } else {
            "Masked the group picked: brushes leave it alone. Clear on the Masking card takes the mask off."
        }
        .into();
    }

    /// Merge: group `from` becomes part of group `into`. One undo step.
    fn group_merge(&mut self, from: u16, into: u16) -> bool {
        let made = groups::merged(&self.mesh, from, into);
        let done = self.groups_made(made, false, |made, count| {
            format!("Merged {} faces into the group clicked ({} now).", grouped(made.triangles), counted(count).to_lowercase())
        });
        if done {
            self.group_pick(into);
        }
        done
    }

    // ------------------------------------------------------- the click on the model

    /// The face group under pixel `px`, if the active object is what is
    /// under it (itself or one of its live mirror copies).
    fn group_under(&self, px: Vec2) -> Option<u16> {
        let (id, _) = self.pick(px)?;
        if id != self.active {
            return None;
        }
        let hit = self.hit(px)?;
        let o = self.active_object();
        let m = o.placed_matrix(hit.side);
        let view = if m == glam::Mat4::IDENTITY { self.view } else { self.view.local_to(m) };
        let (origin, dir) = view.ray(px);
        self.mesh.raycast(origin, dir).map(|h| self.mesh.group_of(h.tri))
    }

    /// Whether a left press on the view is for the groups and not for the
    /// brush or tool: a click a button of the card armed, in any mode, or
    /// Ctrl + Shift + click while a brush is up.
    pub(super) fn groups_takes_press(&self, mods: Modifiers) -> bool {
        let gesture = mods.ctrl && mods.shift && !mods.alt;
        card_get().armed.is_some() || (gesture && self.mode == Mode::Sculpt)
    }

    /// The press [`Self::groups_takes_press`] claimed: the click is the whole of it.
    pub(super) fn groups_pressed(&mut self, px: Vec2) {
        let under = self.group_under(px);
        let armed = card_edit(|c| c.armed.take());
        self.groups_clicked(armed, under);
    }

    /// What a click on the group `under` (none: off the active object)
    /// does, with the card armed as `armed`.
    fn groups_clicked(&mut self, armed: Option<Armed>, under: Option<u16>) {
        if self.sculptor.is_active() {
            return;
        }
        const OFF: &str = "That is not the selected object: click a group on the object the brush works on.";
        match (armed, under) {
            (Some(_), None) => self.status = OFF.into(),
            (Some(Armed::Pick), Some(group)) => {
                let picked = self.group_pick(group);
                let what = if group == 0 { "the surface in no group" } else { "a group" };
                self.status = format!("Picked {what}: {} faces.", grouped(picked.faces));
            }
            (Some(Armed::Merge), Some(into)) => {
                if let Some(from) = self.group_picked_or_say() {
                    self.group_merge(from, into);
                }
            }
            // The gesture: isolate the group, or with it isolated already, or on the background, show all.
            (None, Some(group)) => {
                if !self.mesh.has_groups() {
                    self.status = "This object has no groups yet: make some on the Face Groups card (Mesh tab).".into();
                    return;
                }
                self.group_pick(group);
                self.group_isolate(group);
            }
            (None, None) => {
                if self.mesh.any_hidden() {
                    self.show_all_faces_now();
                }
            }
        }
    }

    // ------------------------------------------------------------ once a frame

    /// Once a frame: keeps the picked group's face count true, lets a pick
    /// of a group that no longer exists go, and says so when a mesh has
    /// just taken another's place without its groups or paint.
    pub(super) fn groups_watch(&mut self) {
        if let Some(picked) = self.group_picked() {
            let mesh = &self.mesh;
            let now = (mesh.uid, mesh.groups_version());
            if picked.counted != now {
                let faces = mesh.group_triangles(picked.group, true).len();
                card_edit(|c| c.picked = (faces > 0).then_some(Picked { faces, counted: now, ..picked }));
            }
        }
        let (groups, paint) = LEFT_BEHIND.with(|l| std::mem::take(&mut *l.borrow_mut()));
        if groups && !self.status.is_empty() {
            self.status += " Its face groups were left behind: the new shape is not the shape they were on.";
        }
        if paint && !self.status.is_empty() {
            self.status += " Its paint was left behind: the new shape is not the shape it was on.";
        }
    }
}
