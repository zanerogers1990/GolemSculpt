//! The gizmo's deformers on the app's side: Bend, Bend Arc, Twist, Taper,
//! Flatten, Inflate, Stretch and Lattice, as the Deform card picks them.
//!
//! The maths is `crate::deform`; this file decides what is deformed, keeps
//! the deformation open while the card and the handles change it, and turns
//! it into an undo step.
//!
//! **A session.** While a deformer is picked and the Transform tool is up, a
//! session is open on the selected objects: the box they fill in the gizmo's
//! frame has been measured, and from the first change on each of their meshes
//! is in a stroke (`deform::Target`). Every frame the card's settings have changed, every mesh
//! is put where the deformer has it, worked out from where it was when the
//! session opened. Apply (Enter) closes the strokes and keeps them as the
//! undo step; Cancel (Esc) puts every vertex back as it was, to the bit, and
//! keeps nothing. Then a fresh session opens on the shape as it now is, so
//! the next deformation starts from there.
//!
//! **Anything else applies first.** A command from anywhere (the panels, a
//! key, the Claude bridge) finds the deformation applied before it runs, as
//! the Layers card's Strength slider is let go first: so Undo of a
//! deformation not yet applied takes it back and Redo puts it on again, and
//! no command ever finds a mesh in the middle of a stroke. A change of the
//! selection or of the gizmo's axes is a new frame, so the session that was
//! open is applied and another opened.
//!
//! **Several objects** are deformed together in one frame, the gizmo's, and
//! one box round all of them: a bend of three posts in a row bends the row.
//! Each mesh keeps a step of its own in the history, and the steps are tied
//! (`History::join_last`), so that one Undo takes them all. Instances share a
//! mesh, which is deformed once, through the active one's placement if it is
//! among them and the first selected one's if not.
//!
//! **Masks and hidden parts.** A vertex moves `1 - mask` of the way the
//! deformer takes it, as under a gizmo drag of a masked object, and hidden
//! vertices stay where they are (the stroke calls pass over them). Stay in
//! Group has no say: it keeps a brush stroke to the group under the pen, and
//! a deformation has no place it begins. To deform one group, Mask All But
//! Group on the Face Groups card first.
//!
//! **Levels and layers.** A deformation is a stroke, so it goes where a
//! stroke goes: on a coarser subdivision level the finer levels follow when
//! the level is changed, and with a sculpt layer recording it goes into the
//! layer. Where a stroke would be refused (the layer that records is hidden
//! or at 0%) the deformation is refused, in the same words. A deformation of
//! the model underneath carries the layers' detail round with the surface
//! (`Ride::Turned`), as the Pose brush does, except Inflate, which pushes
//! the surface along and leaves it standing (`Ride::Still`).
//!
//! **Symmetry.** The result is mirror-symmetric when the deformation is: when
//! it maps every point's mirror image to the mirror image of where it maps the
//! point. That is checked on the model itself for each mirror plane the
//! object has ticked, the moment the settings change, and the status line
//! says so in red if it is not, as Decimate's question does for its own loss.

mod bridge;
mod handles;
mod play;
#[cfg(test)]
mod tests;

use glam::{DVec3, Mat4};
use rayon::prelude::*;

use super::Running;
use super::transform::selection_targets;
use crate::deform::{Bounds, Deformer, Frame, Kind, Params, Target};
use crate::doc::{Action, Doc, Tool};
use crate::gizmo;
use crate::mesh::Mesh;
use crate::mesh::layers::Ride;
use crate::ui::deform::Ask;

pub(super) use handles::Grab;

#[derive(Default)]
pub(super) struct State {
    session: Option<Session>,
    /// A handle drag under way.
    grab: Option<Grab>,
    /// What the last `Ask::Run` came to, for the Claude bridge to read back.
    ran: Option<Result<String, String>>,
}

impl State {
    /// Whether a handle is being dragged.
    pub(super) fn grabbing(&self) -> bool {
        self.grab.is_some()
    }
}

/// One mesh a session is deforming.
struct Part {
    /// The object it is deformed through (for its name and its symmetry).
    object: u64,
    /// The scene's number for the mesh.
    mesh: u64,
    /// The mesh's own space into the world, as it was when the session opened.
    model: Mat4,
    target: Target,
}

/// What a session was opened on: when any of it changes, it is applied and
/// another opened.
#[derive(Clone, PartialEq, Debug)]
struct Key {
    objects: Vec<u64>,
    meshes: Vec<u64>,
    models: Vec<Mat4>,
    frame: Frame,
}

struct Session {
    key: Key,
    frame: Frame,
    /// The box every part fills, in the gizmo's frame.
    bounds: Bounds,
    parts: Vec<Part>,
    /// The settings the parts were last put where, if any.
    shown: Option<Params>,
    /// The mirror planes the status line has said the deformation breaks.
    warned: bool,
}

impl Session {
    fn pending(&self) -> bool {
        self.shown.as_ref().is_some_and(|p| !p.is_identity())
    }
}

/// The mesh with scene number `id`: the active one, or one parked in the scene.
fn mesh_of(doc: &mut Doc, id: u64) -> Option<&mut Mesh> {
    if doc.scene.active_mesh() == id {
        return Some(&mut doc.mesh);
    }
    doc.scene.parked_mut().find(|(mesh, _)| *mesh == id).map(|(_, mesh)| mesh)
}

/// What the selection would be deformed as now: the objects and meshes, each
/// mesh once, the placements, and the gizmo's frame. `None` with nothing
/// selected that shows.
fn key_now(doc: &Doc) -> Option<Key> {
    let (ids, targets) = selection_targets(&doc.scene, &doc.mesh);
    let origin = gizmo::pivot_of(&targets)?;
    let rotation = gizmo::rotation_of(&targets, doc.opts.gizmo_axes);
    let frame = Frame { origin: origin.as_dvec3(), rotation: rotation.as_dquat() };
    let mut key = Key { objects: Vec::new(), meshes: Vec::new(), models: Vec::new(), frame };
    let active = doc.scene.active_id();
    for id in ids {
        let Some(object) = doc.scene.get(id) else { continue };
        if key.meshes.contains(&object.mesh) {
            continue;
        }
        // The active object as the gizmo shows it: on the side of it picked.
        let model = if id == active { doc.scene.shown_matrix() } else { object.matrix() };
        key.objects.push(id);
        key.meshes.push(object.mesh);
        key.models.push(model);
    }
    Some(key)
}

/// How the settings read on the status line: "bent 45° along Y".
fn done_words(p: &Params) -> String {
    let axis = ["X", "Y", "Z"][p.axis];
    match p.kind {
        Kind::Bend => format!("bent {:.0}° along {axis}", p.angle),
        Kind::BendArc => format!("bent into an arc along {axis}"),
        Kind::Twist => format!("twisted {:.0}° about {axis}", p.angle),
        Kind::Taper => format!("tapered along {axis} from {:.0}% to {:.0}%", p.scales[0] * 100.0, p.scales[1] * 100.0),
        Kind::Flatten => format!("flattened {:.0}% along {axis}", p.flatten * 100.0),
        Kind::Inflate if p.inflate < 0.0 => format!("deflated by {:.1}% of its size", -p.inflate * 100.0),
        Kind::Inflate => format!("inflated by {:.1}% of its size", p.inflate * 100.0),
        Kind::Stretch => format!("stretched to {:.0}% along {axis}", p.stretch * 100.0),
        Kind::Lattice => "reshaped with the lattice".to_owned(),
    }
}

impl Running {
    /// Whether the deformers are up: a deformer picked on the card, with the
    /// Transform tool up and the panels showing.
    pub(super) fn deform_on(&self) -> bool {
        self.doc.opts.deform.on && self.transform_active() && !self.doc.opts.clean_view
    }

    /// Whether a deformation is on the model and not yet applied.
    pub(super) fn deform_pending(&self) -> bool {
        self.transform.deform.session.as_ref().is_some_and(Session::pending)
    }

    /// Whether a mesh is in the middle of a deformation's stroke, which
    /// whatever would put another mesh in its place has to wait for.
    pub(super) fn deform_holds_mesh(&self) -> bool {
        self.transform.deform.session.as_ref().is_some_and(|s| s.parts.iter().any(|p| p.target.holds()))
    }

    /// Runs every frame, before the mesh is sent to be drawn: opens, renews
    /// or closes the session, and puts the model where the card's settings
    /// have it if they have changed.
    pub(super) fn deform_frame(&mut self) {
        if !self.deform_on() {
            self.deform_commit();
            self.doc.opts.deform.pending = false;
            return;
        }
        let key = key_now(&self.doc);
        let mut stale = self.transform.deform.session.as_ref().is_some_and(|s| Some(&s.key) != key.as_ref());
        // A mesh put in another's place under the session (it should never be) is let go.
        if let Some(session) = self.transform.deform.session.as_ref() {
            let meshes: Vec<(u64, u64)> = session.parts.iter().map(|p| (p.mesh, p.target.uid)).collect();
            stale |= meshes.into_iter().any(|(mesh, uid)| mesh_of(&mut self.doc, mesh).map(|m| m.uid) != Some(uid));
        }
        if stale && self.transform.deform.grab.is_none() {
            self.deform_commit();
        }
        if self.transform.deform.session.is_none() {
            if let Some(key) = key {
                self.deform_open(key);
            }
        }
        self.deform_preview();
        self.doc.opts.deform.pending = self.deform_pending();
    }

    /// Opens a session on what `key` names.
    fn deform_open(&mut self, key: Key) {
        let doc = &mut self.doc;
        // Reset Shape goes back to the shape before the first thing that moved the mesh (scene.rs).
        doc.scene.keep_baseline(&doc.mesh);
        let frame = key.frame;
        let mut parts = Vec::new();
        for ((&object, &mesh), &model) in key.objects.iter().zip(&key.meshes).zip(&key.models) {
            let Some(m) = mesh_of(doc, mesh) else { continue };
            parts.push(Part { object, mesh, model, target: Target::begin(m, model, &frame) });
        }
        let Some(bounds) =
            parts.iter().map(|p| p.target.bounds).reduce(|a, b| Bounds::new(a.min.min(b.min), a.max.max(b.max)))
        else {
            return;
        };
        self.transform.deform.session = Some(Session { key, frame, bounds, parts, shown: None, warned: false });
    }

    /// Puts every part where the card's settings have it, if they have
    /// changed since the last time. The first change of a session is where a
    /// deformation can be refused.
    fn deform_preview(&mut self) {
        let params = self.doc.opts.deform.params.sanitized();
        let Some(session) = self.transform.deform.session.as_ref() else { return };
        if session.shown.as_ref() == Some(&params) {
            return;
        }
        if !session.pending() && !params.is_identity() {
            if let Some(why) = self.deform_refusal() {
                let card = &mut self.doc.opts.deform;
                card.params = card.params.reset_values();
                self.status(why, false);
                return;
            }
        }
        let Some(session) = self.transform.deform.session.as_mut() else { return };
        let deformer = Deformer::new(&params, &session.bounds);
        for part in &mut session.parts {
            if let Some(mesh) = mesh_of(&mut self.doc, part.mesh) {
                part.target.apply(mesh, deformer.as_ref(), params.axis);
            }
        }
        session.shown = Some(params.clone());
        self.doc.sculptor.last_hit = None;
        self.redraw = true;
        if let Some(deformer) = &deformer {
            self.deform_check_symmetry(&params, deformer);
        }
    }

    /// Why a deformation of the session's parts would be refused, if it would.
    fn deform_refusal(&mut self) -> Option<String> {
        let session = self.transform.deform.session.as_ref()?;
        if session.parts.iter().all(|p| p.target.is_pinned()) {
            let one = session.parts.len() == 1;
            return Some(
                if one {
                    "Everything of the object is masked, so nothing can move. Clear or invert the mask first."
                } else {
                    "Everything of the selected objects is masked, so nothing can move. Clear or invert the mask first."
                }
                .to_owned(),
            );
        }
        let parts: Vec<u64> = session.parts.iter().map(|p| p.mesh).collect();
        parts.into_iter().find_map(|mesh| mesh_of(&mut self.doc, mesh)?.layer_refusal())
    }

    /// Says in red, once a session, if the deformation would make a
    /// mirrored model lopsided.
    fn deform_check_symmetry(&mut self, params: &Params, deformer: &Deformer) {
        let Some(session) = self.transform.deform.session.as_ref() else { return };
        if session.warned {
            return;
        }
        let parts: Vec<(u64, u64, Mat4)> = session.parts.iter().map(|p| (p.object, p.mesh, p.model)).collect();
        for (i, (object, mesh_id, model)) in parts.into_iter().enumerate() {
            let symmetry = self.doc.scene.symmetry(object);
            let planes: Vec<usize> = (0..3).filter(|&a| symmetry.mirror[a]).collect();
            if planes.is_empty() {
                continue;
            }
            let world = symmetry.space == crate::scene::Space::World;
            let Some(mesh) = mesh_of(&mut self.doc, mesh_id) else { continue };
            let Some(session) = self.transform.deform.session.as_ref() else { return };
            let broken: Vec<&str> = planes
                .into_iter()
                .filter(|&a| !keeps_mirror(mesh, &session.parts[i].target, deformer, model, a, world))
                .map(|a| ["X", "Y", "Z"][a])
                .collect();
            if broken.is_empty() {
                continue;
            }
            let name = self.doc.scene.get(object).map(|o| o.name.clone()).unwrap_or_default();
            let text = asymmetry_words(params.kind, &broken.join(" and "), &name);
            if let Some(session) = self.transform.deform.session.as_mut() {
                session.warned = true;
            }
            self.status(text, false);
            return;
        }
    }

    /// Keeps what the session has done as one undo step (one a mesh, tied
    /// together), and closes it.
    pub(super) fn deform_commit(&mut self) {
        let Some(session) = self.transform.deform.session.take() else { return };
        self.transform.deform.grab = None;
        let params = session.shown.clone().unwrap_or_default();
        let ride = if params.kind == Kind::Inflate { Ride::Still } else { Ride::Turned };
        let doc = &mut self.doc;
        let active = doc.history.target;
        let mut steps = 0;
        let mut names = Vec::new();
        let mut masked = false;
        for part in session.parts {
            let Some(mesh) = mesh_of(doc, part.mesh) else { continue };
            masked |= mesh.mask.par_iter().with_min_len(1 << 16).any(|&m| m > 0.0);
            if let Some(delta) = part.target.end(mesh, ride) {
                doc.history.target = part.mesh;
                doc.history.push_stroke(delta);
                if steps > 0 {
                    doc.history.join_last();
                }
                steps += 1;
                names.push(doc.scene.get(part.object).map(|o| o.name.clone()).unwrap_or_default());
            }
        }
        doc.history.target = active;
        let card = &mut doc.opts.deform;
        card.params = card.params.reset_values();
        card.pending = false;
        if steps == 0 {
            return;
        }
        doc.sculptor.last_hit = None;
        let who = match names.len() {
            1 => format!("\"{}\"", names[0]),
            n => format!("{n} objects"),
        };
        let mut text = format!("{}.", capitalised(&insert_name(&done_words(&params), &who)));
        if masked {
            text += " The masked part stayed where it was.";
        }
        if session.warned {
            text += " The two halves of the mirrored model now differ.";
        }
        text += " Undo (Z) puts it back.";
        self.toast(params.kind.name());
        self.status(text, !session.warned);
        self.redraw = true;
    }

    /// Puts the model back as it was before the deformation, keeping nothing.
    pub(super) fn deform_cancel(&mut self) {
        let Some(session) = self.transform.deform.session.take() else { return };
        self.transform.deform.grab = None;
        let had = session.pending();
        for part in session.parts {
            if let Some(mesh) = mesh_of(&mut self.doc, part.mesh) {
                part.target.cancel(mesh);
            }
        }
        let card = &mut self.doc.opts.deform;
        card.params = card.params.reset_values();
        card.pending = false;
        self.doc.sculptor.last_hit = None;
        if had {
            self.toast("Cancelled");
            self.status("The deformation was cancelled; the model is as it was.", true);
        }
        self.redraw = true;
    }

    /// Before any command but the Deform card's own: what is on the model is
    /// applied, so the command finds no mesh mid-stroke.
    pub(super) fn deform_before(&mut self, action: &Action) {
        if !matches!(action, Action::Deform(_)) {
            self.deform_commit();
        }
    }

    /// A command of the Deform card.
    pub(super) fn deform_act(&mut self, ask: Ask) {
        match ask {
            Ask::Pick(kind) => {
                self.deform_commit();
                let card = &mut self.doc.opts.deform;
                match kind {
                    Some(kind) => {
                        card.on = true;
                        card.params = Params { kind, ..card.params.clone() }.reset_values();
                        if self.doc.tool != Tool::Transform {
                            self.set_tool(Tool::Transform);
                        }
                        self.toast(kind.name());
                    }
                    None => card.on = false,
                }
            }
            Ask::Apply => self.deform_commit(),
            Ask::Cancel => self.deform_cancel(),
            Ask::Run(params) => {
                let ran = self.deform_run(&params);
                self.transform.deform.ran = Some(ran);
            }
        }
    }

    /// Deforms the selection with `params` and applies it, whether or not the
    /// card is up: what the Claude bridge asks for. Gives back the status
    /// line, which is an error if the deformation was refused.
    pub(super) fn deform_run(&mut self, params: &Params) -> Result<String, String> {
        self.deform_commit();
        let key = key_now(&self.doc).ok_or("Nothing is selected that shows.")?;
        let card = std::mem::replace(&mut self.doc.opts.deform.params, params.sanitized());
        let before = std::mem::take(&mut self.ui.status);
        let edits = self.doc.history.edits;
        self.deform_open(key);
        self.deform_preview();
        self.deform_commit();
        self.doc.opts.deform.params = card;
        let (text, colour) = self.ui.status.clone();
        if self.doc.history.edits == edits {
            if colour == crate::ui::theme::ERROR && !text.is_empty() {
                return Err(text);
            }
            self.ui.status = before;
            return Ok("Nothing changed: those settings leave the model as it is.".into());
        }
        Ok(text)
    }

    /// Esc cancels what is on the model, or with nothing on it puts the
    /// deformers away; Enter applies. True if the key was the card's.
    pub(super) fn deform_key(&mut self, code: winit::keyboard::KeyCode) -> bool {
        use winit::keyboard::KeyCode;
        if !self.deform_on() || self.input.drag != super::Drag::None {
            return false;
        }
        match code {
            KeyCode::Escape if self.deform_pending() => self.deform_cancel(),
            KeyCode::Escape => {
                self.deform_commit();
                self.doc.opts.deform.on = false;
                self.toast("Transform");
            }
            KeyCode::Enter | KeyCode::NumpadEnter => self.deform_commit(),
            _ => return false,
        }
        self.redraw = true;
        true
    }
}

/// "Bent "Cube" 45° along Y": the name after the verb.
fn insert_name(words: &str, who: &str) -> String {
    match words.split_once(' ') {
        Some((verb, rest)) => format!("{verb} {who} {rest}"),
        None => format!("{words} {who}"),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or(String::new(), |c| c.to_uppercase().collect::<String>() + chars.as_str())
}

/// The status line's warning that `kind` breaks the mirror across `planes`.
fn asymmetry_words(kind: Kind, planes: &str, name: &str) -> String {
    let advice = match kind {
        Kind::Twist => {
            "A twist turns one half one way and its mirror image the other, so it cannot keep a mirrored model mirrored."
        }
        Kind::Bend => {
            "Bend along an axis that lies on the mirror plane, toward a direction along it (0° or 180°), to keep the halves alike."
        }
        Kind::Lattice => "Move the points of the cage in mirrored pairs to keep the halves alike.",
        _ => "Work along an axis that lies on the mirror plane to keep the halves alike.",
    };
    format!(
        "This {} is not mirror-symmetric across {planes}: the two halves of \"{name}\" will come out different. {advice}",
        kind.name().to_lowercase()
    )
}

/// Whether `deformer` maps the mirror image (across mirror plane `axis`) of
/// each of a sample of the mesh's vertices to the mirror image of where it
/// maps the vertex. The planes are the object's own (through its origin,
/// along its axes) or, with `world`, the world's, carried into the mesh's
/// space through `model`. Masks play no part: this is about the deformation.
fn keeps_mirror(mesh: &Mesh, target: &Target, deformer: &Deformer, model: Mat4, axis: usize, world: bool) -> bool {
    let flip = |p: DVec3| {
        let mut q = p;
        q[axis] = -q[axis];
        q
    };
    let model = model.as_dmat4();
    let inverse = model.inverse();
    let reflect = |p: DVec3| if world { inverse.transform_point3(flip(model.transform_point3(p))) } else { flip(p) };
    let reflect_dir =
        |n: DVec3| if world { inverse.transform_vector3(flip(model.transform_vector3(n))) } else { flip(n) };
    let n = mesh.vertex_count();
    let stride = (n / 256).max(1);
    let size = mesh.bounds().size().max_element().max(1e-12) as f64;
    (0..n).step_by(stride).all(|v| {
        let p = mesh.start_pos(v as u32).as_dvec3();
        let normal = mesh.start_nrm(v as u32).as_dvec3();
        let there = target.map(deformer, p, normal);
        let mirrored = target.map(deformer, reflect(p), reflect_dir(normal));
        mirrored.distance(reflect(there)) <= size * 1e-5
    })
}
