//! Face groups, on the app's side: what the Face Groups card asks for, the
//! click on the model that picks and isolates a group, Stay in Group, and
//! the groups following the model through whatever rebuilds it.
//!
//! What a group is, and what each command makes, is `mesh/groups.rs`. Here
//! is how the running app carries the commands out and records them.
//!
//! - **Every command is one undo step**, exact: the step keeps the group
//!   list it replaced, and with it whatever else the same command changed
//!   (Group Masked clears the mask; Isolate changes what is hidden), so one
//!   Undo takes all of it back (`history::Edit::Groups`). Hide Group, Show
//!   All and the two mask commands are the app's ordinary hide, show and
//!   mask steps.
//! - **A group is picked on the model.** Ctrl + Shift + click on a group
//!   isolates it (everything else of the object is hidden) and picks it for
//!   the card's buttons; the same click again, or on the background, shows
//!   everything. It is ZBrush's gesture, and the one combination nothing
//!   else here uses on a click while a brush is up: Ctrl alone inverts a
//!   brush, Shift alone smooths, Alt and its combinations move the camera,
//!   and Ctrl with Shift on a brush stroke means only "invert a smooth",
//!   which does nothing. The region tools, the measure and the tube tool do
//!   read Ctrl and Shift, each for itself, so while a tool is up the gesture
//!   is theirs and the card's Pick button arms the next click instead. That
//!   is also the way with a pen and no keyboard: Pick, tap the group, Isolate.
//! - **Stay in Group** is one question asked as a stroke begins ([`stay`]):
//!   if the brush that will do the work has the tick, the mesh is told which
//!   groups the stroke may touch, and from there the mesh sees to it
//!   (`Mesh::stay_in_groups`). With symmetry, the groups are those under the
//!   start of every copy of the stroke. A copy is not held to its own group
//!   alone: the kernels work every copy out together and share one weight
//!   for a vertex between them, so the mesh, which is where the holding is
//!   done, cannot tell whose write a write is. The difference shows only
//!   where two of the groups come within a brush width of each other.
//! - **Whatever puts a new mesh in an object's place takes the groups
//!   across** by the nearest old face ([`follow`]), unless the routine that
//!   built the mesh carried them itself (a change of level, a copy). The
//!   paint goes the same way, by the nearest old point
//!   (`mesh/paint/transfer.rs`): every function below that carries groups
//!   carries paint too.
//!
//! The scripted run of all of it is `--demo-groups play` (`groups/play.rs`).

use std::cell::Cell;

use glam::{Affine3A, Mat4, Vec2};

use super::{Drag, Running};
use crate::camera::View;
use crate::doc::Action;
use crate::mesh::Mesh;
use crate::mesh::groups::{self, Carried, Fit, GroupsStep, Made, Source};
use crate::mesh::paint::transfer as paint;
use crate::scene::Scene;
use crate::sculpt::Sculptor;
use crate::ui::groups::{Armed, Ask, Picked, counted};

mod bridge;
mod play;

pub(super) use bridge::{group_at, state_of};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod paint_tests;

thread_local! {
    /// A mesh took another's place and could not take its groups: said on
    /// the status line by the next frame's look (`groups_watch`). Kept here
    /// and not in `Running`, like the levels' own note: nothing else reads it.
    static LEFT_BEHIND: Cell<bool> = const { Cell::new(false) };
    /// The same for paint.
    static PAINT_LEFT_BEHIND: Cell<bool> = const { Cell::new(false) };
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

// ------------------------------------------------------------ Stay in Group

/// Stay in Group, asked as a stroke is about to begin at `pos`: if the brush
/// that will make the stroke has the tick, keeps the stroke to the face
/// groups under its start and under the start of each of its symmetric
/// copies. Returns whether it did. A stroke that then does not begin has to
/// let go again ([`began`]).
pub(crate) fn stay(mesh: &mut Mesh, sculptor: &Sculptor, view: &View, pos: Vec2, smooth_held: bool) -> bool {
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
/// chain the press will make. The preview has to let go again when it has
/// looked (`Mesh::leave_groups`): no stroke follows it to do so.
pub(crate) fn stay_at(mesh: &mut Mesh, sculptor: &Sculptor, at: glam::Vec3, smooth_held: bool) -> bool {
    if !stays(mesh, sculptor, smooth_held) {
        return false;
    }
    let Some(triangle) = showing_face_at(mesh, at) else { return false };
    hold(mesh, sculptor, at, mesh.group_of(triangle));
    true
}

/// Whether a stroke begun now would be kept to its group: the mesh has
/// groups, and the brush that would do the work has the tick. With Shift
/// held that is the Smooth brush Shift brings up, by its own tick.
fn stays(mesh: &Mesh, sculptor: &Sculptor, smooth_held: bool) -> bool {
    let settings = &sculptor.settings;
    let brush = sculptor.effective_brush(smooth_held);
    mesh.has_groups() && settings.in_use(brush, smooth_held && !settings.mask_mode).0.stay_in_group
}

/// The face that shows nearest `at`.
fn showing_face_at(mesh: &Mesh, at: glam::Vec3) -> Option<u32> {
    let hidden = |t: u32| mesh.is_triangle_hidden(t);
    mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, at, f32::INFINITY, hidden).map(|(t, _)| t)
}

/// Keeps the stroke that starts at `at`, on a face of group `group`, to
/// that group and to the groups its symmetric copies start on: each copy
/// begins on the surface nearest the image of `at`.
fn hold(mesh: &mut Mesh, sculptor: &Sculptor, at: glam::Vec3, group: u16) {
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

/// `new`, about to take the place of `old`, given `old`'s groups by the
/// nearest face. Nothing is done if `old` has none, or if `new` came with
/// its own (the routine that built it carried them).
pub(crate) fn follow(old: &Mesh, new: &mut Mesh, fit: Fit) {
    if old.has_groups() && !new.has_groups() {
        noted(groups::carry(new, &[Source::of(old)], fit));
    }
    if old.has_paint() && !new.has_paint() {
        noted_paint(paint::carry(new, &[Source::of(old)], fit));
    }
}

/// Remembers a mesh that could not take its groups, for the status line.
fn noted(carried: Carried) {
    if carried == Carried::Dropped {
        LEFT_BEHIND.with(|left| left.set(true));
    }
}

/// Remembers a mesh that could not take its paint, for the status line.
fn noted_paint(carried: paint::Carried) {
    if carried == paint::Carried::Dropped {
        PAINT_LEFT_BEHIND.with(|left| left.set(true));
    }
}

/// `mesh`, about to take the place of object `id`'s mesh as the same shape
/// built again (back from Blender), with that mesh's groups.
pub(crate) fn taking_over(scene: &Scene, slot: &Mesh, id: u64, mut mesh: Mesh) -> Mesh {
    if let Some(old) = scene.get(id).and_then(|o| scene.mesh(o.mesh, slot)) {
        follow(old, &mut mesh, Fit::Whole);
    }
    mesh
}

/// `mesh`, a piece of object `from` or a copy of it about to become an
/// object placed by `model`, with the groups of the surface of `from` it
/// lies on. A piece lies on the object where the two stand in the scene (what
/// a cut leaves, a part split off). A copy stands somewhere else and is the
/// object's own mesh again in its own space (an array's copies made
/// separate objects), so when nothing of the mesh is found on the object
/// where it stands, it is looked for there.
pub(crate) fn piece_of(scene: &Scene, slot: &Mesh, from: u64, model: Mat4, mesh: &mut Mesh) {
    let Some((object, old)) = scene.get(from).and_then(|o| Some((o, scene.mesh(o.mesh, slot)?))) else { return };
    let place = Affine3A::from_mat4(object.matrix().inverse() * model);
    if old.has_groups() && !mesh.has_groups() {
        let in_scene = Source { place, ..Source::of(old) };
        if groups::carry(mesh, &[in_scene], Fit::Part) == Carried::Nothing {
            groups::carry(mesh, &[Source::of(old)], Fit::Part);
        }
    }
    if old.has_paint() && !mesh.has_paint() {
        let in_scene = Source { place, ..Source::of(old) };
        if paint::carry(mesh, &[in_scene], Fit::Part) == paint::Carried::Nothing {
            paint::carry(mesh, &[Source::of(old)], Fit::Part);
        }
    }
}

/// A mesh a tool handed over, built for sculpting, with where each of its
/// triangles stood in the list the tool handed over: what tells an array
/// made real from any other mesh (`groups::copies`).
pub(crate) struct Handed {
    pub mesh: Mesh,
    triangle: Vec<u32>,
    /// Where each vertex handed over went: what tells an array made real
    /// which vertex is a copy of which, for its paint (`paint::copies`).
    vertex: Vec<u32>,
}

impl Handed {
    /// [`Mesh::new`], keeping the order the triangles came in.
    pub(crate) fn built(positions: Vec<glam::Vec3>, indices: Vec<u32>, mask: Option<Vec<f32>>) -> Handed {
        let (mesh, numbering) = Mesh::new_numbered(positions, indices, mask, &[]);
        Handed { mesh, triangle: numbering.triangle, vertex: numbering.vertex }
    }
}

/// The mesh a tool handed over to take the place of object `id`'s, with
/// that mesh's groups. An array made real as one object is the mesh copied
/// out several times, and every copy keeps the groups; anything else (what
/// a cut leaves) keeps them on the old surface it still is ([`piece_of`]).
pub(crate) fn replacing(scene: &Scene, slot: &Mesh, id: u64, handed: Handed) -> Mesh {
    let Handed { mut mesh, triangle, vertex } = handed;
    let Some((object, old)) = scene.get(id).and_then(|o| Some((o, scene.mesh(o.mesh, slot)?))) else { return mesh };
    if old.has_paint() {
        paint::copies(old, &mut mesh, &vertex);
    }
    if let Some(ids) = old.has_groups().then(|| groups::copies(old, &mesh, &triangle)).flatten() {
        mesh.set_group_ids(ids);
    }
    // What is not a copy keeps what lies on the old surface (each of the two only if it has none yet).
    piece_of(scene, slot, id, object.matrix(), &mut mesh);
    mesh
}

/// `mesh`, built from the scene's whole selection in the active object's own
/// space (a join, a boolean), with the groups of the objects it was made of.
/// Two objects' groups stay two groups: where they share a number, the later
/// object's is renamed.
pub(crate) fn of_selection(scene: &Scene, slot: &Mesh, mesh: &mut Mesh, fit: Fit) {
    let active = scene.active();
    let others = scene.selected().iter().filter(|&&id| id != active.id).filter_map(|&id| scene.get(id));
    let objects: Vec<_> =
        std::iter::once(active).chain(others).filter_map(|o| Some((o, scene.mesh(o.mesh, slot)?))).collect();
    let meshes: Vec<&Mesh> = objects.iter().map(|(_, mesh)| *mesh).collect();
    if meshes.iter().any(|mesh| mesh.has_paint()) && !mesh.has_paint() {
        let sources: Vec<Source> = objects
            .iter()
            .map(|(object, old)| Source {
                mesh: old,
                ids: (&[][..]).into(),
                place: Affine3A::from_mat4(object.matrix().inverse() * active.matrix()),
                fold: None,
            })
            .collect();
        noted_paint(paint::carry(mesh, &sources, fit));
    }
    if meshes.iter().all(|mesh| !mesh.has_groups()) {
        return;
    }
    let sources: Vec<Source> = objects
        .iter()
        .zip(groups::kept_apart(&meshes))
        .map(|((object, old), ids)| Source {
            mesh: old,
            ids: ids.into(),
            // From the new mesh's space (the active object's) into this object's own.
            place: Affine3A::from_mat4(object.matrix().inverse() * active.matrix()),
            fold: None,
        })
        .collect();
    noted(groups::carry(mesh, &sources, fit));
}

/// [`of_selection`] for a join. A plain join's faces are the objects' own, so
/// each keeps exactly the group it had; one that was remeshed afterwards is
/// the whole shape built again.
pub(crate) fn of_joined(scene: &Scene, slot: &Mesh, mesh: &mut Mesh, remeshed: bool) {
    of_selection(scene, slot, mesh, if remeshed { Fit::Whole } else { Fit::Part });
}

/// `new`, `old` cut at its own plane facing `axis` and rebuilt as the side
/// kept (the positive one when `positive`) and its mirror image, with
/// `old`'s groups: the rebuilt side takes those of the side it is the image
/// of. Every face of the result lies in a face of the side kept, or in its
/// image, so each takes exactly that face's group.
pub(crate) fn mirrored(old: &Mesh, new: &mut Mesh, axis: usize, positive: bool) {
    if old.has_groups() {
        let source = Source { fold: Some((axis, positive)), ..Source::of(old) };
        groups::carry(new, &[source], Fit::Part);
    }
    if old.has_paint() {
        let source = Source { fold: Some((axis, positive)), ..Source::of(old) };
        paint::carry(new, &[source], Fit::Part);
    }
}

// -------------------------------------------------------------- the commands

impl Running {
    /// The scene's number for the mesh in hand.
    fn groups_mesh(&self) -> u64 {
        self.doc.scene.active_mesh()
    }

    /// The group picked on the model, if it is a group of the mesh in hand.
    fn group_picked(&self) -> Option<Picked> {
        self.doc.opts.groups.picked.filter(|picked| picked.mesh == self.groups_mesh())
    }

    /// The group picked, or a line saying that none is.
    pub(super) fn group_picked_or_say(&mut self) -> Option<u16> {
        let picked = self.group_picked().map(|picked| picked.group);
        if picked.is_none() {
            self.status(
                "No group is picked: Ctrl + Shift + click one on the model, or press Pick and click it.",
                false,
            );
        }
        picked
    }

    /// Makes `group` the group picked, counting its faces.
    fn group_pick(&mut self, group: u16) -> Picked {
        let mesh = &self.doc.mesh;
        let faces = mesh.group_triangles(group, true).len();
        let picked = Picked { mesh: self.groups_mesh(), group, faces, counted: (mesh.uid, mesh.groups_version()) };
        self.doc.opts.groups.picked = Some(picked);
        self.redraw = true;
        picked
    }

    /// Carries out a command of the Face Groups card.
    pub(super) fn groups_act(&mut self, ask: Ask) {
        // Any command but the arming itself lets an armed click go.
        if !matches!(ask, Ask::Arm(_)) {
            self.doc.opts.groups.armed = None;
        }
        match ask {
            Ask::GroupMasked => self.group_masked(),
            Ask::GroupVisible => {
                let made = groups::visible(&self.doc.mesh);
                self.groups_made(made, false, "Group Visible", |made, count| {
                    format!(
                        "Group Visible: {} faces are a new group ({}).",
                        grouped(made.triangles),
                        counted(count).to_lowercase()
                    )
                });
            }
            Ask::Auto => {
                let made = groups::parts(&self.doc.mesh);
                self.groups_made(made, false, "Auto Groups", |made, _| match made.groups {
                    1 => "Auto Groups: this object is one connected piece, so it is one group.".to_owned(),
                    parts => format!("Auto Groups: {parts} groups, one for each separate part."),
                });
            }
            Ask::ByAngle => {
                let angle = self.doc.opts.groups.angle;
                let made = groups::by_angle(&self.doc.mesh, angle);
                self.groups_made(made, false, "Groups by Angle", |made, _| match made.groups {
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
                    let triangles = self.doc.mesh.group_triangles(group, true);
                    self.hide_faces(&triangles, true);
                }
            }
            Ask::Mask { others } => {
                if let Some(group) = self.group_picked_or_say() {
                    self.group_mask(group, others);
                }
            }
        }
    }

    /// Puts a command's new group list on the mesh as one undo step and says
    /// what `said` makes of it (given what was made and how many groups
    /// there are now), or says why nothing was made.
    fn groups_made(
        &mut self,
        made: Result<Made, String>,
        clear_mask: bool,
        toast: &str,
        said: impl FnOnce(&Made, usize) -> String,
    ) -> bool {
        let made = match made {
            Ok(made) => made,
            Err(why) => {
                self.status(why, false);
                return false;
            }
        };
        let doc = &mut self.doc;
        let first = !doc.mesh.has_groups();
        let summary = Made { ids: Vec::new(), ..made };
        let before = doc.mesh.set_group_ids(made.ids);
        let mask = if clear_mask { doc.mesh.remap_mask(|_, _, _| 0.0) } else { None };
        doc.history.push_groups(GroupsStep::groups(before, mask));
        let mut text = said(&summary, doc.mesh.group_count());
        // The first groups of an object would be made unseen: show them.
        if first && !doc.opts.groups.colours {
            doc.opts.groups.colours = true;
            text += " Show Group Colours is on now, so they can be seen.";
        }
        self.redraw = true;
        self.toast(toast);
        self.status(text, true);
        true
    }

    fn group_masked(&mut self) {
        let keep = self.doc.opts.groups.keep_mask;
        let made = groups::masked(&self.doc.mesh);
        let group = made.as_ref().map_or(0, |made| made.group);
        let done = self.groups_made(made, !keep, "Group Masked", |made, count| {
            let mask = if keep { "The mask is as it was." } else { "The mask was cleared." };
            format!(
                "Group Masked: {} faces are a new group ({}). {mask}",
                grouped(made.triangles),
                counted(count).to_lowercase()
            )
        });
        // The group just made is the one the card's buttons work on.
        if done {
            self.group_pick(group);
        }
    }

    fn groups_clear(&mut self) {
        let count = self.doc.mesh.group_count();
        if count == 0 {
            return self.status("This object has no groups.", true);
        }
        let doc = &mut self.doc;
        let before = doc.mesh.set_group_ids(Vec::new());
        doc.history.push_groups(GroupsStep::groups(before, None));
        doc.opts.groups.picked = None;
        self.redraw = true;
        self.toast("Clear Groups");
        self.status(format!("Cleared {}. Undo (Z) brings them back.", counted(count).to_lowercase()), true);
    }

    fn groups_arm(&mut self, armed: Option<Armed>) {
        if armed == Some(Armed::Merge) && self.group_picked_or_say().is_none() {
            return;
        }
        self.doc.opts.groups.armed = armed;
        self.redraw = true;
        match armed {
            Some(Armed::Pick) => self.status("Click a group on the model to pick it.", true),
            Some(Armed::Merge) => {
                self.status("Click the group on the model that the picked group should become part of.", true)
            }
            None => {}
        }
    }

    /// Whether everything but `group` is hidden and all of `group` shows.
    fn group_is_isolated(&self, group: u16) -> bool {
        let mesh = &self.doc.mesh;
        let Some(hidden) = mesh.hidden_triangles() else { return false };
        mesh.group_ids().iter().zip(hidden).all(|(&id, &hidden)| (id != group) == hidden)
    }

    /// Isolate: hides everything of the object but `group`; with that
    /// already so, shows everything. One undo step either way.
    pub(super) fn group_isolate(&mut self, group: u16) {
        if self.group_is_isolated(group) {
            self.show_all_faces();
            return;
        }
        let doc = &mut self.doc;
        let flags: Vec<bool> = doc.mesh.group_ids().iter().map(|&id| id != group).collect();
        if !doc.mesh.has_groups() || !flags.contains(&true) {
            return self.status("That group is the whole object, so there is nothing else to hide.", true);
        }
        let before = doc.mesh.hidden_triangles().map(<[bool]>::to_vec);
        doc.mesh.restore_hidden_triangles(&flags);
        doc.history.push_groups(GroupsStep::hidden(before.as_deref()));
        let shown = doc.mesh.visible_triangle_count();
        self.redraw = true;
        self.toast("Isolate");
        self.status(
            format!(
                "Isolated a group of {} faces: the rest of the object is hidden. Ctrl + Shift + click it again, or \
                 Show All, brings everything back.",
                grouped(shown)
            ),
            true,
        );
    }

    /// Mask Group (the group masked, the rest free) or, with `others`, Mask
    /// All But (the rest masked, the group free). The border's vertices are
    /// the group's. One undo step, like any change to the mask.
    fn group_mask(&mut self, group: u16, others: bool) {
        let doc = &mut self.doc;
        let on = doc.mesh.group_vertices(group);
        let delta = doc.mesh.remap_mask(|mesh, v, m| {
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
            return self.status("The mask is exactly that already.", true);
        };
        doc.history.push_stroke(delta);
        self.redraw = true;
        self.toast(if others { "Mask All But" } else { "Mask Group" });
        self.status(
            if others {
                "Masked everything but the group picked: brushes work on it alone. Clear on the Masking card takes the mask off."
            } else {
                "Masked the group picked: brushes leave it alone. Clear on the Masking card takes the mask off."
            },
            true,
        );
    }

    /// Merge: group `from` becomes part of group `into`. One undo step.
    pub(super) fn group_merge(&mut self, from: u16, into: u16) -> bool {
        let made = groups::merged(&self.doc.mesh, from, into);
        let done = self.groups_made(made, false, "Merge", |made, count| {
            format!(
                "Merged {} faces into the group clicked ({} now).",
                grouped(made.triangles),
                counted(count).to_lowercase()
            )
        });
        if done {
            self.group_pick(into);
        }
        done
    }

    // ------------------------------------------------------- the click on the model

    /// The face group under a pixel, if the selected object's mesh is what
    /// is under it (the object, an instance or a mirror copy of it).
    pub(super) fn group_under(&mut self, pos: Vec2) -> Option<u16> {
        self.sync_view();
        let (id, copy) = self.pick_placed(pos)?;
        let doc = &mut self.doc;
        if !doc.scene.shows_active_mesh(id) {
            return None;
        }
        // The ray in the mesh's own space, through the placement that was clicked.
        doc.scene.aim_brush(id, copy);
        let local = self.view.local_to(doc.scene.brush_matrix());
        doc.scene.release_brush();
        let (origin, dir) = local.ray(pos);
        doc.mesh.raycast(origin, dir).map(|hit| doc.mesh.group_of(hit.tri))
    }

    /// Whether a left press on the view is for the groups and not for the
    /// brush or tool: a click a button of the card armed, in any mode, or
    /// Ctrl + Shift + click while a brush is up (the module notes say why
    /// only then).
    pub(super) fn groups_takes_press(&self) -> bool {
        let mods = self.input.mods;
        let gesture = mods.control_key() && mods.shift_key() && !mods.alt_key();
        let brush_up = !self.tools_active() && !self.spheres_active() && !self.transform_active();
        self.doc.opts.groups.armed.is_some() || (gesture && brush_up)
    }

    /// The press [`Self::groups_takes_press`] claimed. Nothing follows the
    /// pointer afterwards: the click is the whole of it.
    pub(super) fn groups_pressed(&mut self) -> Drag {
        let under = self.group_under(self.input.pointer);
        let armed = self.doc.opts.groups.armed.take();
        self.groups_clicked(armed, under);
        Drag::Ui
    }

    /// What a click on the group `under` (none: off the selected object)
    /// does, with the card armed as `armed`.
    pub(super) fn groups_clicked(&mut self, armed: Option<Armed>, under: Option<u16>) {
        self.redraw = true;
        const OFF: &str = "That is not the selected object: click a group on the object the brush works on.";
        match (armed, under) {
            (Some(_), None) => self.status(OFF, false),
            (Some(Armed::Pick), Some(group)) => {
                let picked = self.group_pick(group);
                let what = if group == 0 { "the surface in no group" } else { "a group" };
                self.status(format!("Picked {what}: {} faces.", grouped(picked.faces)), true);
            }
            (Some(Armed::Merge), Some(into)) => {
                if let Some(from) = self.group_picked_or_say() {
                    self.group_merge(from, into);
                }
            }
            // The gesture: isolate the group, or with it isolated already, or on the background, show all.
            (None, Some(group)) => {
                if !self.doc.mesh.has_groups() {
                    return self
                        .status("This object has no groups yet: make some on the Face Groups card (Mesh tab).", true);
                }
                self.group_pick(group);
                self.group_isolate(group);
            }
            (None, None) => {
                if self.doc.mesh.any_hidden() {
                    self.show_all_faces();
                }
            }
        }
    }

    // ------------------------------------------------------------ once a frame

    /// Once a frame: keeps the picked group's face count true, lets a pick
    /// of a group that no longer exists go, and says so when a mesh has
    /// just taken another's place without its groups.
    pub(super) fn groups_watch(&mut self) {
        if let Some(picked) = self.group_picked() {
            let mesh = &self.doc.mesh;
            if picked.counted != (mesh.uid, mesh.groups_version()) {
                let faces = mesh.group_triangles(picked.group, true).len();
                let counted = (mesh.uid, mesh.groups_version());
                self.doc.opts.groups.picked = (faces > 0).then_some(Picked { faces, counted, ..picked });
            }
        }
        if LEFT_BEHIND.with(|left| left.replace(false)) && !self.ui.status.0.is_empty() {
            self.ui.status.0 += " Its face groups were left behind: the new shape is not the shape they were on.";
        }
        if PAINT_LEFT_BEHIND.with(|left| left.replace(false)) && !self.ui.status.0.is_empty() {
            self.ui.status.0 += " Its paint was left behind: the new shape is not the shape it was on.";
        }
    }
}

/// The Face Groups commands the keys and the bridge reach by `Action`.
impl Running {
    /// Runs a card command as the card would.
    pub(super) fn groups_ask(&mut self, ask: Ask) {
        self.act(Action::Groups(ask));
    }
}
