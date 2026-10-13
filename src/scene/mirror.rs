//! Live mirror copies of an object (Nomad's Mirror), on the scene's side.
//!
//! A copy is not a second mesh kept in step. It is the object's own mesh drawn
//! again through a reflected matrix, so there is nothing to sync and nothing
//! that can drift: every stroke, remesh or cut is already an edit to every
//! copy. What an object keeps is only `Object::mirror`, the axes it is
//! mirrored across and the point the planes pass through (see
//! `sculpt_symmetry::mirror` for the arithmetic). Because that lives in the
//! object, turning the mirror on or off, and validating it, are ordinary
//! object-list edits: one undo step each, restored with everything else.
//!
//! Either side can be worked on. A ray meets the copies as well as the object
//! (`Picked::copy`), a stroke that begins on a copy is mapped through the
//! copy's matrix (`Scene::aim_brush`), so it edits exactly the vertices the
//! copy shows, and the transform gizmo can sit on a copy: the copy has no
//! placement of its own to write, so a drag moves the original, reflected
//! back (`Scene::place_shown`).

use glam::{Mat4, Vec3};
use sculpt_symmetry::{MirrorCopy, MirrorRepeater, Placement};
use serde::{Deserialize, Serialize};

use super::{Object, Scene};
use crate::io::{Details, slot};
use crate::mesh::Mesh;
use crate::mesh::bvh::Aabb;

/// What the Mirror section of the Objects card asks for. Each acts on the
/// active object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorCommand {
    /// Live copies across these world axes (X = 1, Y = 2, Z = 4), replacing
    /// the ones it had; 0 takes them away.
    Set(u8),
    /// Turns the copies into real, separate objects exactly where they are.
    Validate,
    /// Takes the live copies away.
    Remove,
    /// Puts the gizmo on one side of the object: a copy's flip mask, or 0 for
    /// the object itself.
    Side(u8),
}

/// The box around `bounds` once it is placed by `model`.
pub(super) fn box_through(bounds: Aabb, model: &Mat4) -> Aabb {
    let mut through = Aabb::EMPTY;
    for corner in 0..8 {
        let pick = |bit: usize, axis: usize| if corner >> bit & 1 == 0 { bounds.min[axis] } else { bounds.max[axis] };
        through.grow(model.transform_point3(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2))));
    }
    through
}

impl Object {
    /// Where the object sits, as the geometry crate takes it.
    pub fn placement(&self) -> Placement {
        Placement::new(self.position, self.rotation, self.scale)
    }

    /// Whether `copy` (a flip mask) is one of the object's live mirror copies.
    pub fn has_copy(&self, copy: u8) -> bool {
        copy != 0 && self.mirror.is_some_and(|m| copy & !m.axis_mask == 0)
    }

    /// Every place the object is drawn, as the flip mask of the copy (0 for the
    /// object itself, first) and the matrix that places the mesh there.
    pub fn placements(&self) -> impl Iterator<Item = (u8, Mat4)> + '_ {
        let model = self.matrix();
        let copies = self
            .mirror
            .into_iter()
            .flat_map(move |m| m.copies().into_iter().map(move |c| (c.flip_mask, c.matrix(&model, m.center))));
        std::iter::once((0, model)).chain(copies)
    }

    /// The matrix of one placement: the object's own for `copy` 0, else that
    /// live mirror copy's (the world reflection after the object's matrix). The
    /// object's own if it has no such copy.
    pub fn placed_matrix(&self, copy: u8) -> Mat4 {
        match self.mirror.filter(|_| self.has_copy(copy)) {
            Some(mirror) => MirrorCopy::new(copy).matrix(&self.matrix(), mirror.center),
            None => self.matrix(),
        }
    }
}

impl Scene {
    /// Gives an object live mirror copies across the axes in `axes` (X = 1,
    /// Y = 2, Z = 4) through `center`, replacing any it had; 0 removes them.
    /// False, and nothing changes, if that is what it has already.
    pub fn set_mirror(&mut self, id: u64, axes: u8, center: Vec3) -> bool {
        let wanted = MirrorRepeater::new(axes, center);
        let Some(object) = self.get_mut(id) else { return false };
        if object.mirror == wanted {
            return false;
        }
        object.mirror = wanted;
        // A copy that is gone cannot stay selected.
        if self.side.is_some_and(|(owner, copy)| owner == id && !self.get(id).is_some_and(|o| o.has_copy(copy))) {
            self.side = None;
        }
        true
    }

    /// Turns the live copies of an object into real, independent objects
    /// exactly where they are shown, and removes the live mirror. Each is a
    /// mesh of its own, the object's reflected in its own space (the mask goes
    /// with it), named "<object> Mirror X", and sits just under the object in
    /// the list. The object ends up the one selected. Returns the new objects,
    /// or `None` when it has no mirror.
    pub fn bake_mirror(&mut self, id: u64, slot: &mut Mesh) -> Option<Vec<u64>> {
        let source = self.get(id)?.clone();
        let repeater = source.mirror?;
        let mesh = self.mesh(source.mesh, slot)?;
        let mask = mesh.mask.clone();
        // A copy's faces are the object's own, in the same order, so they are in the same face groups.
        let groups = mesh.group_ids().to_vec();
        // No normals to carry across: a mesh works its own out when it is built.
        let baked = repeater.bake(&mesh.positions, None, &mesh.indices, &source.placement());

        let mut made = Vec::with_capacity(baked.len());
        for bake in baked {
            let (points, corners) = (bake.geometry.positions, bake.geometry.indices);
            let mesh = crate::mesh::groups::built_with(points, corners, Some(mask.clone()), &groups);
            let name = self.unique_name(&format!("{} Mirror {}", source.name, bake.copy.axis_name()));
            let new = self.add(&name, mesh, slot);
            if let Some(object) = self.get_mut(new) {
                let Placement { position, rotation, scale } = bake.placement;
                // What was on screen stays on screen, and what was not stays out of it.
                (object.position, object.rotation, object.scale, object.visible) =
                    (position, rotation, scale, source.visible);
            }
            let row = self.index_of(source.id).map_or(self.objects.len(), |row| row + 1);
            self.move_to(new, row + made.len());
            // Its own space is the object's reflected, and every mirror plane
            // and X, Y or Z radial axis runs along an axis the reflection only
            // negates: the same settings describe the same symmetry.
            let symmetry = self.symmetry(source.id).reflected(bake.copy.signs);
            self.place_symmetry(new, symmetry);
            made.push(new);
        }
        if let Some(object) = self.get_mut(id) {
            object.mirror = None;
        }
        self.select(id, slot);
        Some(made)
    }

    // ---- which side the gizmo is on --------------------------------------------

    /// Which side of the active object the selection is on: the flip mask of
    /// one of its live mirror copies, or 0 for the object itself. The gizmo
    /// sits on that side. It falls back to 0 once that copy is gone, and it
    /// belongs to the object it was chosen for, so another object being
    /// active leaves it behind.
    pub fn selected_side(&self) -> u8 {
        let active = self.active();
        match self.side {
            Some((id, copy)) if id == active.id && active.has_copy(copy) => copy,
            _ => 0,
        }
    }

    /// Puts the gizmo on a side of the active object (see `selected_side`).
    /// Nothing happens for another object, or a copy it does not have.
    pub fn select_side(&mut self, id: u64, copy: u8) {
        if id != self.active || (copy != 0 && !self.active().has_copy(copy)) {
            return;
        }
        self.side = (copy != 0).then_some((id, copy));
    }

    /// The matrix of the side the gizmo is on: where the mesh is drawn there.
    pub fn shown_matrix(&self) -> Mat4 {
        self.active().placed_matrix(self.selected_side())
    }

    /// Where `object` is as the gizmo shows it: its own place, or, when it is
    /// the active object and one of its copies is the side selected, that
    /// copy's. A copy is the same mesh drawn in the object's own axes
    /// reflected, with a proper rotation and the object's own (positive)
    /// scale: what a gizmo can hold.
    pub fn shown_placement(&self, object: &Object) -> Placement {
        let own = object.placement();
        match object.mirror.filter(|_| object.id == self.active && self.selected_side() != 0) {
            Some(mirror) => MirrorCopy::new(self.selected_side()).baked_placement(&own, mirror.center),
            None => own,
        }
    }

    /// The other half of `shown_placement`: the gizmo has put `object` where
    /// `shown` says it is as it shows, and this moves the real object so that
    /// it is. For a copy that is the original reflected back, so dragging the
    /// copy right moves the original left.
    pub fn place_shown(&mut self, id: u64, shown: Placement) {
        let Some(object) = self.get(id) else { return };
        if self.shown_placement(object) == shown {
            // Nothing moved; reflecting a placement twice need not return the bits it started from.
            return;
        }
        let side = self.selected_side();
        let mirror = object.mirror.filter(|_| id == self.active && side != 0);
        let Some(object) = self.get_mut(id) else { return };
        let Placement { position, rotation, scale } = match mirror {
            Some(mirror) => {
                let copy = MirrorCopy::new(side);
                Placement::new(
                    copy.source_position(shown.position, mirror.center),
                    copy.source_rotation(shown.rotation),
                    shown.scale,
                )
            }
            None => shown,
        };
        (object.position, object.rotation, object.scale) = (position, rotation, scale);
    }

    // ---- bounds ------------------------------------------------------------------

    /// World bounds of an object, its live mirror copies and the copies tools
    /// keep of it (an array's): all of what is on screen for it, which is what
    /// framing and the camera's reach go by, as the Unity app's go by every
    /// copy. (`bounds_of` is the object alone, which is what the gizmo is
    /// sized to.)
    pub fn shown_bounds_of(&self, object: &Object, slot: &Mesh) -> Aabb {
        let own = self.bounds_of(object, slot);
        let kept = self.tool_places(object.id);
        let copies = object.mirror.is_some() || !kept.is_empty();
        let Some(mesh) = self.mesh(object.mesh, slot).filter(|_| copies) else { return own };
        let local = mesh.bounds();
        if local.is_empty() {
            return own;
        }
        let others = object.placements().skip(1).map(|(_, model)| model).chain(kept.iter().copied());
        others.fold(own, |all, model| Aabb::union(all, box_through(local, &model)))
    }

    // ---- files ---------------------------------------------------------------------

    /// Puts each mirrored object's live mirror in `details`, under its row in
    /// the object list.
    pub fn keep_mirrors(&self, details: &mut Details) {
        for (row, object) in self.objects.iter().enumerate() {
            if let Some(mirror) = object.mirror {
                details.set_object(row, slot::MIRROR_COPIES, &SavedMirror::of(&mirror));
            }
        }
    }

    /// Gives each object the live mirror `details` kept for its row, if any.
    /// A file from before live mirrors had none to give.
    pub fn restore_mirrors(&mut self, details: &Details) {
        for (row, object) in self.objects.iter_mut().enumerate() {
            object.mirror = saved_mirror(details, row).or(object.mirror);
        }
    }
}

/// The live mirror a file kept for the object in row `row` of its list, if it
/// kept one that reads.
pub fn saved_mirror(details: &Details, row: usize) -> Option<MirrorRepeater> {
    details.get_object::<SavedMirror>(row, slot::MIRROR_COPIES)?.repeater()
}

/// How a live mirror is kept in a `.sclp`: under `mirror_copies` in the
/// object's entry of the JSON section, so older files simply have none and
/// older versions of the app skip it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedMirror {
    /// X = 1, Y = 2, Z = 4; 0 is no mirror.
    pub axes: u8,
    /// The point the planes pass through.
    pub center: [f32; 3],
}

impl Default for SavedMirror {
    fn default() -> Self {
        Self { axes: 0, center: [0.0; 3] }
    }
}

impl SavedMirror {
    pub fn of(mirror: &MirrorRepeater) -> Self {
        Self { axes: mirror.axis_mask, center: mirror.center.to_array() }
    }

    /// The live mirror this describes; `None` for no axes. A centre that is
    /// not a number is taken as the world's origin.
    pub fn repeater(&self) -> Option<MirrorRepeater> {
        let center = Vec3::from_array(self.center);
        MirrorRepeater::new(self.axes, if center.is_finite() { center } else { Vec3::ZERO })
    }
}

#[cfg(test)]
mod tests;
