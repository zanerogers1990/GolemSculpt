//! The symmetry each object is sculpted under: its mirror axes, its radial
//! symmetry, and whether those are the object's own or the world's.
//!
//! Every object has its own, as in the Unity app (its `MirrorController`):
//! selecting another object brings up that object's symmetry, so a figure
//! whose face is mirrored across X and whose body across Z keeps both. The
//! Symmetry card, the X key, the guides, every brush and every tool that
//! follows the brushes go by the selected object's.
//!
//! What an object starts with is the Unity app's rule too:
//!
//! - A copy starts with what it was copied from: a clone, an instance, an
//!   instance made unique, a live mirror made into real objects (whose custom
//!   radial axis is reflected with their geometry), and a piece a tool cuts
//!   from an object or grows on it (Split's part, a tube kept separate).
//! - Anything else starts with none: a new shape, an imported model, an
//!   SSphere skin, a lathed object (unless it asks for radial symmetry), a
//!   mask extract.
//! - What stays the same object keeps what it has: the one a join or a
//!   boolean leaves, and an object that is remeshed, trimmed or merged into.
//!
//! **Instances share it.** In the Unity app an instance was a second placement
//! of its source and had no settings of its own, so the source's were the
//! instance's. Here the objects that show one mesh are equals, so setting the
//! symmetry of one sets it for all of them ([`Scene::set_symmetry`]).
//!
//! **Not an undo step.** Ticking an axis was never on the Unity app's undo
//! history and no undo put one back. That is why the settings are kept beside
//! the object list, by object, and not in [`Object`]: the list is what an undo
//! step restores whole, and it would take every change made to the settings
//! since back with it. An object that undo brings back finds its settings as
//! it left them.
//!
//! **Local and World** (Nomad's names, and the Unity app's). A Local symmetry
//! goes through the object's own origin along its own axes and moves and
//! turns with it. A World one goes through the world's origin along the
//! world's axes and stays put while the object moves through it: the same
//! list of copies built about the world's origin, carried into the mesh's
//! space through where the selected object stands when the stroke begins
//! (`SymmetryGroup::in_object_frame`). Moving or turning the object in
//! between strokes moves the planes through the mesh, never in the world.
//! Where the plane or axis passes is exact for any placement; its tilt is
//! exact for an object scaled evenly, or unevenly along axes that line up with
//! the world's, and for any other uneven scale the copies are as true as a
//! rigid mirror image of a stretched mesh can be (the Unity app's limit too).
//! It pairs with the live mirror of the Objects card: parts placed either
//! side of the world's planes sculpt symmetrically.

use glam::Vec3;
use sculpt_geom::SymmetryGroup;
use sculpt_symmetry::Placement;
use serde::{Deserialize, Serialize};

use super::{Object, Scene};
use crate::io::{Details, Mirror, slot};
use crate::sculpt::{Radial, RadialAxis, Settings};

/// Where an object's symmetry sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Space {
    /// Through the object's own origin, along its own axes.
    #[default]
    Local,
    /// Through the world's origin, along the world's axes.
    World,
}

impl Space {
    pub const ALL: [Space; 2] = [Space::Local, Space::World];

    pub fn name(self) -> &'static str {
        match self {
            Space::Local => "Local",
            Space::World => "World",
        }
    }
}

/// One object's symmetry settings: what the Symmetry card shows while that
/// object is selected.
///
/// Kept in `.sclp` files under each object (`io::slot::SYMMETRY`), so a field
/// added here needs a value to start from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Symmetry {
    /// The mirror axes strokes are repeated across: X, Y, Z.
    pub mirror: [bool; 3],
    /// Strokes repeated round an axis, on top of the mirror axes.
    pub radial: Radial,
    pub space: Space,
}

impl Symmetry {
    /// One stroke, no copies.
    pub fn is_off(&self) -> bool {
        self.mirror == [false; 3] && !self.radial.on
    }

    /// Where the object stands, when that is what places the symmetry: the
    /// planes and the axis are the world's. `None` for a symmetry of the
    /// object's own.
    pub fn world(&self, object: &Object) -> Option<Placement> {
        (self.space == Space::World).then(|| object.placement())
    }

    /// Every copy a stroke on `object` is made as, the stroke itself first,
    /// in the space of the object's mesh.
    pub fn group_for(&self, object: &Object) -> SymmetryGroup {
        let mut settings = Settings::default();
        settings.set_symmetry(self.mirror, self.radial, self.world(object));
        settings.symmetry_group()
    }

    /// The mirror planes a rebuilt mesh can be made exactly symmetric across
    /// (a remesh, a boolean): the ticked ones, while they are the object's
    /// own, which is where the lattice a mesh is rebuilt on is centred. The
    /// world's planes cut the object anywhere, so there it is none.
    pub fn exact_mirrors(&self) -> [bool; 3] {
        if self.space == Space::Local { self.mirror } else { [false; 3] }
    }

    /// What the Symmetry card says in its header: "X + Z + Radial 8", with
    /// "(World)" after it when that is where it sits, or "Off".
    pub fn summary(&self) -> String {
        let mut settings = Settings::default();
        settings.set_symmetry(self.mirror, self.radial, None);
        let summary = settings.symmetry_summary();
        if self.space == Space::World && !self.is_off() { format!("{summary} (World)") } else { summary }
    }

    /// These settings for a copy of the object whose geometry was reflected
    /// by `signs` (each +1 or -1). The mirror axes and an X, Y or Z radial
    /// axis are the same planes and the same line either way, since a
    /// reflection only negates along them; a custom axis is a direction and
    /// is reflected with the geometry.
    pub fn reflected(self, signs: Vec3) -> Symmetry {
        Symmetry { radial: Radial { custom_axis: self.radial.custom_axis * signs, ..self.radial }, ..self }
    }

    /// These settings with everything in range: what a file's numbers are
    /// passed through before they are used.
    pub fn sanitized(self) -> Symmetry {
        Symmetry { radial: self.radial.sanitized(), ..self }
    }

    /// For scripted screenshots: the words of the environment variable
    /// `SCULPT_DEMO_BRUSH` that are about symmetry (the rest are the brushes',
    /// see `Settings::demo_setup`). `radial=6` is radial symmetry with that
    /// many repeats, `axis=z` the axis it goes round (x, y or z), `mirror=off`
    /// takes the mirror axes off and `space=world` fixes it all in the world.
    pub fn demo_setup(&mut self) {
        let spec = std::env::var("SCULPT_DEMO_BRUSH").unwrap_or_default();
        for word in spec.split(',').map(str::trim) {
            match word.split_once('=') {
                Some(("radial", count)) => {
                    let count = count.parse().unwrap_or(self.radial.count);
                    self.radial = Radial { on: true, count, ..self.radial }.sanitized();
                }
                Some(("axis", name)) => {
                    let axis = RadialAxis::ALL.into_iter().find(|a| a.name().eq_ignore_ascii_case(name));
                    self.radial.axis = axis.unwrap_or(self.radial.axis);
                }
                Some(("mirror", "off")) => self.mirror = [false; 3],
                Some(("space", name)) => {
                    let space = Space::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name));
                    self.space = space.unwrap_or(self.space);
                }
                _ => {}
            }
        }
    }
}

/// What the viewport needs to draw the selected object's guides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guides {
    /// Which mirror planes to draw: X, Y, Z.
    pub mirror: [bool; 3],
    /// The radial guide, as `Radial::guide` gives it.
    pub radial: Option<([f32; 4], [f32; 4])>,
    /// The object's own space into the space the planes and the axis are in:
    /// nothing to do for its own, its placement for the world's.
    pub space: glam::Mat4,
}

impl Scene {
    /// The symmetry object `id` is sculpted under. An object that was never
    /// given one has none.
    pub fn symmetry(&self, id: u64) -> Symmetry {
        self.symmetry.get(&id).copied().unwrap_or_default()
    }

    /// The selected object's: what the Symmetry card shows and strokes use.
    pub fn active_symmetry(&self) -> Symmetry {
        self.symmetry(self.active_id())
    }

    /// Sets the symmetry of object `id`, and of every other object that shows
    /// the same mesh: instances share it (see the top of this file). Not an
    /// undo step.
    pub fn set_symmetry(&mut self, id: u64, symmetry: Symmetry) {
        let Some(mesh) = self.get(id).map(|o| o.mesh) else { return };
        let sharing: Vec<u64> = self.objects.iter().filter(|o| o.mesh == mesh).map(|o| o.id).collect();
        for id in sharing {
            self.symmetry.insert(id, symmetry);
        }
    }

    /// Changes the selected object's symmetry.
    pub fn edit_symmetry(&mut self, edit: impl FnOnce(&mut Symmetry)) {
        let mut symmetry = self.active_symmetry();
        edit(&mut symmetry);
        self.set_symmetry(self.active_id(), symmetry);
    }

    /// `to` starts with the symmetry `from` has: it is a copy of it, an
    /// instance of it, or a piece of it. Only `to` is set; an instance already
    /// shares its mesh with `from`, and anything else is an object of its own.
    pub fn inherit_symmetry(&mut self, from: u64, to: u64) {
        self.symmetry.insert(to, self.symmetry(from));
    }

    /// Gives one object a symmetry of its own as it is brought into the
    /// scene, leaving any object it shares a mesh with alone: what a file
    /// says each of its objects had.
    pub(crate) fn place_symmetry(&mut self, id: u64, symmetry: Symmetry) {
        self.symmetry.insert(id, symmetry);
    }

    /// Hands the sculptor the selected object's symmetry, which is the one a
    /// stroke is made under whichever placement of the mesh the pointer is on
    /// (the object, an instance of it, a live mirror copy). Called before
    /// every stroke, so no stroke can be made under the symmetry of the
    /// object that was selected before.
    pub fn hand_symmetry_to(&self, settings: &mut Settings) {
        let active = self.active();
        let symmetry = self.symmetry(active.id);
        settings.set_symmetry(symmetry.mirror, symmetry.radial, symmetry.world(active));
    }

    /// The selected object's guides.
    pub fn guides(&self) -> Guides {
        let active = self.active();
        let symmetry = self.symmetry(active.id);
        let space = if symmetry.space == Space::World { active.matrix() } else { glam::Mat4::IDENTITY };
        Guides { mirror: symmetry.mirror, radial: symmetry.radial.guide(), space }
    }

    // ---- files -----------------------------------------------------------------

    /// Puts every object's symmetry in `details`, under its row in the object
    /// list. Every row gets one, set or not, so that a file from this version
    /// never falls back on the one setting older files had (see
    /// [`saved_symmetry`]).
    pub fn keep_symmetry(&self, details: &mut Details) {
        for (row, object) in self.objects.iter().enumerate() {
            details.set_object(row, slot::SYMMETRY, &self.symmetry(object.id));
        }
    }

    /// Gives each object the symmetry `details` kept for its row.
    pub fn restore_symmetry(&mut self, details: &Details) {
        let ids: Vec<u64> = self.objects.iter().map(|o| o.id).collect();
        for (row, id) in ids.into_iter().enumerate() {
            if let Some(symmetry) = saved_symmetry(details, row) {
                self.place_symmetry(id, symmetry);
            }
        }
    }
}

/// The symmetry a file kept for the object in row `row` of its list.
///
/// A file from before symmetry belonged to objects has one setting for the
/// whole scene (`slot::MIRROR`), in whichever object was selected's own space.
/// That was every object's setting in turn, so every object of such a file
/// gets it, as a Local symmetry. `None` for a file that kept neither.
pub fn saved_symmetry(details: &Details, row: usize) -> Option<Symmetry> {
    if let Some(own) = details.get_object_over(row, slot::SYMMETRY, &Symmetry::default()) {
        return Some(own.sanitized());
    }
    let scene: Mirror = details.get_over(slot::MIRROR, &Mirror::default())?;
    Some(Symmetry { mirror: [scene.x, scene.y, scene.z], radial: scene.radial.sanitized(), space: Space::Local })
}

#[cfg(test)]
mod tests;
