//! The transform gizmo on a sphere: arrows that slide the selected sphere,
//! and everything below it, straight along one world axis.
//!
//! As in the Unity app a sphere gets arrows and nothing else. It is round,
//! so a ring would turn it into itself; its size is the Scale mode's (and the
//! wheel's) job; and swinging a branch about its parent joint is the Rotate
//! mode's. A handle that visibly does nothing reads as broken, so none is
//! offered.
//!
//! The arrows are up in Move mode, with a sphere selected and nothing else
//! being dragged: that is the moment a limb needs moving exactly along one
//! axis, while grabbing the sphere itself still moves it freely (see
//! [`SSpheres::claims_pointer`]). The drawing and the picking are the app's
//! gizmo, the same one that sits on objects (app/transform.rs); this file is
//! the spheres' side of it. A drag is the Move mode's own drag with the
//! pointer replaced by the gizmo: the branch goes with the sphere, the
//! reflection follows because it was never a separate sphere, a sphere near
//! the mirror plane sticks to it, and the whole drag is one step in the
//! spheres' own undo history.

use glam::{Quat, Vec3};

use super::input::{self, DragKind};
use super::{EditMode, SSpheres};
use crate::gizmo::{Handles, Placement, Target};

/// Where the gizmo goes: the selected sphere, as the world sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoOffer {
    pub position: Vec3,
    /// The sphere's radius in the world, which sizes the arrows so they
    /// stand clear of it.
    pub radius: f32,
}

impl GizmoOffer {
    /// The offer as the app's gizmo takes it: a point on the world's axes
    /// (a sphere has no turn or size of its own for the gizmo to keep) with
    /// arrows to move it.
    pub fn target(&self) -> (Target, Handles) {
        let placement = Placement { position: self.position, rotation: Quat::IDENTITY, scale: Vec3::ONE };
        (
            Target { placement, radius: self.radius, turns: false, scales: false },
            Handles { moves: true, turns: false, scales: false },
        )
    }
}

impl SSpheres {
    /// What the gizmo should sit on right now, if anything. Asked every
    /// frame, so the answer follows the tool's state: leave Move mode,
    /// deselect, delete the sphere, undo it away, switch to the solid skin or
    /// start another drag, and the arrows go.
    pub fn gizmo(&self) -> Option<GizmoOffer> {
        if self.mode != EditMode::Move || self.preview || self.is_dragging() {
            return None;
        }
        let node = self.rig.get(self.selected?)?;
        Some(GizmoOffer { position: self.frame.to_world(node.position), radius: node.radius * self.frame.scale })
    }

    /// A press landed on an arrow: opens the drag, before anything has moved
    /// so the undo step goes back to this. False (and nothing opened) when
    /// there is no sphere to drag or the pointer is busy with something else.
    pub fn begin_gizmo_drag(&mut self) -> bool {
        if self.is_dragging() || self.gauge.is_some() {
            return false;
        }
        let Some(node) = self.selected.filter(|&s| self.rig.is_alive(s)) else { return false };
        self.drag = input::Drag::default();
        self.begin_edit("Move Sphere");
        // The sphere and everything below it, so a chain keeps its shape.
        self.capture(node, true);
        self.drag.node = Some(node);
        self.drag.kind = DragKind::Gizmo;
        true
    }

    /// The arrow has put the sphere at `world` (where its centre belongs, in
    /// the world). Worked out from the press, not from the last call, so
    /// dragging back to the start puts every sphere back exactly. Ignored
    /// when no gizmo drag is open, which is how a drag cancelled with Esc
    /// stays cancelled.
    pub fn gizmo_drag_to(&mut self, world: Vec3) {
        if self.drag.kind != DragKind::Gizmo {
            return;
        }
        let Some((_, radius)) = self.drag_node() else { return self.end_drag() };
        let Some(&start) = self.drag.start_positions.first() else { return self.end_drag() };
        let target = self.snap_if_symmetric(self.frame.to_rig(world), radius);
        let delta = target - start;
        // The first sphere captured is the dragged one: it lands exactly on
        // the target (which matters on the mirror plane, where it must be
        // exactly central), and the rest keep their distance from it.
        self.set_captured(|slot, at| if slot == 0 { target } else { at + delta });
    }

    /// The button came up: closes the drag as one undo step. A press on an
    /// arrow that never moved leaves none.
    pub fn end_gizmo_drag(&mut self) {
        if self.drag.kind == DragKind::Gizmo {
            self.end_drag();
        }
    }
}
