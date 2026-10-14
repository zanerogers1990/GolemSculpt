//! The point of the active model under the pointer, worked out again only
//! when something it depends on has changed.
//!
//! Every frame asks for it (the brush ring is sized by how far away it is),
//! and most frames come from the pointer moving over the interface or from
//! the interface redrawing, with the pointer, the camera and the model just
//! where they were. The ray says where the pointer is and how the camera and
//! the object are placed; the mesh's id and versions and the renderer's
//! upload count say whether the model is the same one, in the same shape.
//! A change made since the last upload has not reached that count yet, so
//! one still waiting to be sent is asked about directly.

use glam::Vec3;

use super::Running;

/// What the last hover was worked out from, and what it found.
#[derive(Default)]
pub(super) struct Last {
    key: Option<Key>,
    hit: Option<Vec3>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Key {
    origin: Vec3,
    dir: Vec3,
    uid: u64,
    topology: u64,
    visibility: u64,
    uploads: u64,
}

impl Last {
    /// The hit kept for `key`, if it still holds: the same key, and nothing
    /// moved since (`moved`).
    fn kept(&self, key: Key, moved: bool) -> Option<Option<Vec3>> {
        (!moved && self.key == Some(key)).then_some(self.hit)
    }
}

impl Running {
    /// Where the ray from `origin` along `dir` (in the active object's space)
    /// meets the active model.
    pub(super) fn hover_at(&mut self, origin: Vec3, dir: Vec3) -> Option<Vec3> {
        let mesh = &self.doc.mesh;
        let key = Key {
            origin,
            dir,
            uid: mesh.uid,
            topology: mesh.topology_version(),
            visibility: mesh.visibility_version(),
            uploads: self.renderer.uploads(),
        };
        let moved = mesh.moved_since_upload() || self.doc.sculptor.is_active();
        if let Some(hit) = self.input.hover_last.kept(key, moved) {
            return hit;
        }
        let hit = mesh.raycast(origin, dir).map(|h| h.pos);
        self.input.hover_last = Last { key: Some(key), hit };
        hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Key {
        Key { origin: Vec3::new(0.0, 0.0, 5.0), dir: Vec3::NEG_Z, uid: 3, topology: 1, visibility: 1, uploads: 7 }
    }

    #[test]
    fn a_hover_is_kept_only_while_the_ray_and_the_model_are_unchanged() {
        let last = Last { key: Some(key()), hit: Some(Vec3::Z) };
        assert_eq!(last.kept(key(), false), Some(Some(Vec3::Z)));
        // A miss is kept as a miss.
        let missed = Last { key: Some(key()), hit: None };
        assert_eq!(missed.kept(key(), false), Some(None));
        // Moved vertices not yet sent, a pointer a hair away, a camera turned,
        // or a model edited, replaced, split or with faces hidden: worked out again.
        assert_eq!(last.kept(key(), true), None);
        let changed = [
            Key { origin: Vec3::new(1e-6, 0.0, 5.0), ..key() },
            Key { dir: Vec3::new(0.0, 1e-6, -1.0), ..key() },
            Key { uploads: 8, ..key() },
            Key { uid: 4, ..key() },
            Key { topology: 2, ..key() },
            Key { visibility: 2, ..key() },
        ];
        for k in changed {
            assert_eq!(last.kept(k, false), None, "{k:?}");
        }
        assert_eq!(Last::default().kept(key(), false), None);
    }
}
