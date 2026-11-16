//! The web version's objects: each a mesh with a place, a turn and a size of
//! its own, as on the desktop (`src/scene.rs`), kept much smaller.
//!
//! One object at a time is the active one, the one brushes, Remesh and the
//! tools work on. Its mesh lives in the app (`GolemApp::mesh`), where the
//! rest of the web version has always found the model; every other object
//! keeps its own here, with what the renderer draws of it made once when it
//! was set aside (`Drawn`). Choosing another object swaps the two.
//!
//! The desktop's gizmo (`src/gizmo.rs`, compiled in) moves objects through
//! `position`, `rotation` and `scale`, which is why this type is called
//! `Object` and lives at `crate::scene`, where the gizmo looks for it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::{Mat4, Quat, Vec3};
use sculpt_symmetry::{MirrorCopy, MirrorRepeater};

use crate::mesh::Mesh;
use crate::mesh::bvh::Aabb;

pub struct Object {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub position: Vec3,
    pub rotation: Quat,
    /// Size along the object's own axes.
    pub scale: Vec3,
    /// The object's mesh while another is active; `None` for the active one,
    /// whose mesh is the app's.
    pub mesh: Option<Mesh>,
    /// What the renderer draws of it while another is active.
    pub drawn: Option<Arc<Drawn>>,
    /// Its live mirror copies across the world's planes, if it has any: the
    /// same mesh drawn again, reflected (the desktop's Live Mirror).
    pub mirror: Option<MirrorRepeater>,
}

/// What can change of an object without touching its mesh: what an undo
/// step of a move, a hide or a rename keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct Props {
    pub name: String,
    pub visible: bool,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub mirror: Option<MirrorRepeater>,
}

impl Props {
    /// An object shown, in the middle of the scene, upright and its own size.
    pub fn named(name: impl Into<String>) -> Props {
        Props { name: name.into(), visible: true, position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE, mirror: None }
    }
}

impl Object {
    pub fn new(id: u64, name: String, mesh: Option<Mesh>) -> Object {
        let mut object = Object {
            id,
            name,
            visible: true,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            mesh: None,
            drawn: None,
            mirror: None,
        };
        if let Some(mesh) = mesh {
            object.set_aside(mesh);
        }
        object
    }

    /// The object's own space into world space.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }

    pub fn props(&self) -> Props {
        Props {
            name: self.name.clone(),
            visible: self.visible,
            position: self.position,
            rotation: self.rotation,
            scale: self.scale,
            mirror: self.mirror,
        }
    }

    pub fn set_props(&mut self, p: Props) {
        (self.name, self.visible, self.position, self.rotation, self.scale, self.mirror) =
            (p.name, p.visible, p.position, p.rotation, p.scale, p.mirror);
    }

    /// Keeps `mesh` as this object's own, with what is drawn of it.
    pub fn set_aside(&mut self, mesh: Mesh) {
        self.drawn = Some(Arc::new(Drawn::of(&mesh)));
        self.mesh = Some(mesh);
    }

    /// Hands back the object's mesh, to become the app's (it is active now).
    pub fn take_mesh(&mut self) -> Option<Mesh> {
        self.drawn = None;
        self.mesh.take()
    }

    /// Whether `copy` (a flip mask) is one of its live mirror copies.
    pub fn has_copy(&self, copy: u8) -> bool {
        copy != 0 && self.mirror.is_some_and(|m| copy & !m.axis_mask == 0)
    }

    /// Every place it is drawn: the flip mask of the copy (0 for the object
    /// itself, first) and the matrix that puts its mesh there (the desktop's
    /// `Object::placements`).
    pub fn placements(&self) -> Vec<(u8, Mat4)> {
        let model = self.matrix();
        let mut out = vec![(0, model)];
        if let Some(m) = self.mirror {
            out.extend(m.copies().into_iter().map(|c| (c.flip_mask, c.matrix(&model, m.center))));
        }
        out
    }

    /// The matrix of one placement: its own for `copy` 0 (or a copy it does
    /// not have), else that live copy's.
    pub fn placed_matrix(&self, copy: u8) -> Mat4 {
        match self.mirror.filter(|_| self.has_copy(copy)) {
            Some(m) => MirrorCopy::new(copy).matrix(&self.matrix(), m.center),
            None => self.matrix(),
        }
    }

    /// Its box in world space, from `mesh` (its own, or the app's for the
    /// active object): the object alone, which the gizmo is sized to.
    pub fn world_bounds(&self, mesh: &Mesh) -> Aabb {
        box_through(mesh.bounds(), &self.matrix())
    }

    /// Its box in world space with its live mirror copies: all that is on
    /// screen of it, which framing goes by.
    pub fn shown_bounds(&self, mesh: &Mesh) -> Aabb {
        let local = mesh.bounds();
        let mut out = box_through(local, &self.matrix());
        for (_, m) in self.placements().into_iter().skip(1) {
            let b = box_through(local, &m);
            if !b.is_empty() {
                out.min = out.min.min(b.min);
                out.max = out.max.max(b.max);
            }
        }
        out
    }
}

/// The box round `b` once `m` places it.
fn box_through(b: Aabb, m: &Mat4) -> Aabb {
        if b.is_empty() {
            return b;
        }
        let mut out = Aabb { min: Vec3::INFINITY, max: Vec3::NEG_INFINITY };
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { b.min.x } else { b.max.x },
                if i & 2 == 0 { b.min.y } else { b.max.y },
                if i & 4 == 0 { b.min.z } else { b.max.z },
            );
            let p = m.transform_point3(corner);
            out.min = out.min.min(p);
            out.max = out.max.max(p);
        }
        out
}

/// An object's mesh as the renderer draws it while it is not the active
/// one: made once, and handed to every frame by reference.
pub struct Drawn {
    /// New for every one made, so the renderer knows to send it again.
    pub id: u64,
    pub positions: Vec<u8>,
    pub normals: Vec<u8>,
    pub indices: Vec<u8>,
    pub index_count: u32,
    pub vertices: usize,
}

impl Drawn {
    pub fn of(mesh: &Mesh) -> Drawn {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let shown = mesh.visible_indices();
        Drawn {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            positions: bytemuck::cast_slice(&mesh.positions).to_vec(),
            normals: bytemuck::cast_slice(&mesh.normals).to_vec(),
            indices: bytemuck::cast_slice(&shown).to_vec(),
            index_count: shown.len() as u32,
            vertices: mesh.positions.len(),
        }
    }
}

/// `name`, or `name (2)`, `name (3)` ... whichever no object has yet.
pub fn unique_name(objects: &[Object], name: &str) -> String {
    let taken = |n: &str| objects.iter().any(|o| o.name == n);
    if !taken(name) {
        return name.to_owned();
    }
    (2..).map(|i| format!("{name} ({i})")).find(|n| !taken(n)).unwrap()
}
