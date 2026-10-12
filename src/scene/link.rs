//! A mesh from outside in an object's place: what the Blender link does when
//! an object comes back worked on (`app/blender.rs`).
//!
//! The object keeps everything that is the object's: its row, its name, where
//! it stands, its turn and size, its live mirror, its symmetry, its
//! instances. Only the shape is new. As after a join, the new mesh goes in
//! under a new number: the undo step is the object list, which names the old
//! mesh, and the old mesh stays parked until no step shows it, so Undo puts
//! back exactly what was there.

use super::Scene;
use crate::mesh::Mesh;

impl Scene {
    /// Gives the object `id` the mesh `mesh` in place of the one it shows, and
    /// every instance of it with it: they share a shape, and go on sharing
    /// this one. `slot` is `Doc::mesh`. False, and nothing changes, if there
    /// is no such object. The new mesh is at rest as it is, for Reset Shape.
    pub fn replace_mesh_of(&mut self, id: u64, mesh: Mesh, slot: &mut Mesh) -> bool {
        let Some(old) = self.get(id).map(|o| o.mesh) else { return false };
        let in_hand = self.active_mesh() == old;
        let new = self.fresh_id();
        self.parked.insert(new, mesh);
        for o in self.objects.iter_mut().filter(|o| o.mesh == old) {
            o.mesh = new;
        }
        if in_hand {
            self.swap_in(old, new, slot);
            (self.brush, self.stroked) = (None, None);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    #[test]
    fn an_object_takes_a_new_mesh_with_its_instances_and_undo_puts_the_old_one_back() {
        let mut slot = primitives::build(Primitive::Sphere, 1_000, 0.5);
        let ball_points = slot.positions.len();
        let mut scene = Scene::new("Ball");
        let ball = scene.active_id();
        let twin = scene.instance_active();
        let block = scene.add("Block", primitives::build(Primitive::Cube, 500, 0.5), &mut slot);
        scene.get_mut(ball).unwrap().position = Vec3::new(2.0, 0.0, 0.0);
        let before = scene.state();
        let block_points = slot.positions.len();

        // The ball is not the active object: its mesh is one of the parked ones.
        let cone = primitives::build(Primitive::Cone, 800, 0.5);
        let cone_points = cone.positions.len();
        assert!(scene.replace_mesh_of(ball, cone, &mut slot));
        let shown = |scene: &Scene, id: u64, slot: &Mesh| scene.mesh(scene.get(id).unwrap().mesh, slot).unwrap().positions.len();
        assert_eq!(shown(&scene, ball, &slot), cone_points);
        assert_eq!(scene.get(ball).unwrap().mesh, scene.get(twin).unwrap().mesh, "its instance shares the new shape");
        assert_eq!(scene.get(ball).unwrap().position, Vec3::new(2.0, 0.0, 0.0), "and it stands where it stood");
        assert_eq!((scene.active_id(), slot.positions.len()), (block, block_points), "the active object is as it was");

        // The active object: the mesh in hand is the new one.
        let rod = primitives::build(Primitive::Cylinder, 600, 0.5);
        let rod_points = rod.positions.len();
        assert!(scene.replace_mesh_of(block, rod, &mut slot));
        assert_eq!((slot.positions.len(), shown(&scene, block, &slot)), (rod_points, rod_points));

        assert!(!scene.replace_mesh_of(999, primitives::build(Primitive::Cube, 100, 0.5), &mut slot));

        // Undo is the object list as it was, which names the old meshes.
        scene.restore(before, &mut slot);
        assert_eq!((shown(&scene, ball, &slot), shown(&scene, twin, &slot)), (ball_points, ball_points));
        assert_eq!((scene.active_id(), slot.positions.len()), (block, block_points));
    }
}
