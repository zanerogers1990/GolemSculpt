//! Symmetry of whole objects: the settings an object is sculpted under, and
//! live mirror copies of it (Nomad's Mirror).
//!
//! A live copy is not a second mesh kept in step. It is the SAME mesh drawn
//! again through a reflected transform, so there is nothing to sync and nothing
//! that can drift: every edit to the mesh is already an edit to every copy, and
//! a topology change is just a new mesh the copies pick up next frame. This
//! module is the arithmetic behind that: where each copy is drawn, how moving a
//! copy moves the original, and what baking the copies into real geometry
//! produces. Creating, drawing and picking the copies is the app's side.

use glam::{Mat4, Quat, Vec3};
use sculpt_geom::join::join;
use sculpt_geom::mirror::{Reflected, reflect_geometry, reflect_point, reflect_rotation};
use sculpt_geom::{Aabb, SymmetryGroup, SymmetryOp, TriMesh};

use crate::weld;

/// Where an object sits in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Placement {
    pub const IDENTITY: Placement = Placement { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    pub fn new(position: Vec3, rotation: Quat, scale: Vec3) -> Placement {
        Placement { position, rotation, scale }
    }

    /// Local to world.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

impl Default for Placement {
    fn default() -> Placement {
        Placement::IDENTITY
    }
}

/// The axis radial symmetry repeats around: a local axis through the object's
/// origin. Stored by value in save files, so new members go on the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RadialAxis {
    X,
    /// The axis a lathed object is turned around, and the usual "up" of a bust
    /// or a vase.
    #[default]
    Y,
    Z,
    Custom,
}

/// Where an object's symmetry lives (Nomad's Local/World): through the
/// object's own origin and along its own axes, moving with it, or fixed at the
/// world origin along the world axes, with the object free to move through it.
/// Stored by value in save files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SymmetrySpace {
    #[default]
    Local,
    World,
}

/// The sculpting symmetry of one object: up to three axes of mirroring, plus
/// N-fold radial symmetry around an axis. Any combination can be on at once
/// (X and Y mirror a stroke into all four quadrants; radial repeats it at N
/// evenly spaced angles, combined with the mirrors when both are on).
///
/// Mirroring is per object: each object reflects through its own origin, so
/// two objects cannot share one setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SymmetrySettings {
    /// Mirror across the planes perpendicular to X, Y and Z.
    pub mirror: [bool; 3],
    pub radial: bool,
    /// Repeats around the radial axis; used clamped to what `SymmetryGroup`
    /// allows.
    pub radial_count: u32,
    pub radial_axis: RadialAxis,
    /// The direction used when `radial_axis` is `Custom`. Stored as given; a
    /// zero vector falls back to Y when used.
    pub radial_custom_axis: Vec3,
    pub space: SymmetrySpace,
}

impl Default for SymmetrySettings {
    fn default() -> SymmetrySettings {
        SymmetrySettings {
            mirror: [false; 3],
            radial: false,
            radial_count: 8,
            radial_axis: RadialAxis::Y,
            radial_custom_axis: Vec3::Y,
            space: SymmetrySpace::Local,
        }
    }
}

impl SymmetrySettings {
    /// No symmetry at all: one stroke, no repeats.
    pub fn is_off(&self) -> bool {
        self.mirror == [false; 3] && !self.radial
    }

    /// The radial axis as a unit direction.
    pub fn radial_axis_vector(&self) -> Vec3 {
        match self.radial_axis {
            RadialAxis::X => Vec3::X,
            RadialAxis::Y => Vec3::Y,
            RadialAxis::Z => Vec3::Z,
            RadialAxis::Custom if self.radial_custom_axis.length_squared() > 1e-12 => self.radial_custom_axis.normalize(),
            RadialAxis::Custom => Vec3::Y,
        }
    }

    /// Rotations about the radial axis a stroke is repeated at: 1 when radial
    /// symmetry is off.
    pub fn radial_repeats(&self) -> u32 {
        let (fewest, most) = (SymmetryGroup::MIN_RADIAL_COUNT, SymmetryGroup::MAX_RADIAL_COUNT);
        if self.radial { self.radial_count.clamp(fewest, most) } else { 1 }
    }

    /// The symmetry about the origin and axes of whichever space it lives in:
    /// the enabled mirror reflections times the radial rotations, identity
    /// first. For a `Local` symmetry this is already in the object's own
    /// space.
    pub fn group(&self) -> SymmetryGroup {
        let [x, y, z] = self.mirror;
        SymmetryGroup::build(x, y, z, self.radial_repeats(), self.radial_axis_vector())
    }

    /// Every symmetry op a stroke on an object placed at `object` is applied
    /// under, in the object's LOCAL space, identity first. Mapping a local
    /// point, delta or normal through op `k` gives its `k`-th symmetric copy.
    ///
    /// A `World` symmetry is the same group built about the world origin and
    /// carried into the object's frame, so it changes whenever the object
    /// moves. Building a group is cheap but not free and brushes ask once per
    /// dab: keep the result until a setting or (in World space) the placement
    /// changes.
    pub fn group_for(&self, object: &Placement) -> SymmetryGroup {
        let group = self.group();
        if self.space == SymmetrySpace::Local || group.len() == 1 {
            return group;
        }
        group.in_object_frame(object.rotation, object.position, &object.matrix().inverse())
    }

    /// These settings for a copy of the object whose geometry was reflected by
    /// `signs` (all +1 for a plain clone). Mirror flags and an X, Y or Z radial
    /// axis describe the same planes and axis either way, since a reflection
    /// only negates along them; a custom axis is reflected with the geometry.
    pub fn reflected(&self, signs: Vec3) -> SymmetrySettings {
        SymmetrySettings { radial_custom_axis: self.radial_custom_axis * signs, ..*self }
    }
}

/// Symmetry guides are sized off the object's bounds, padded so the plane
/// visibly extends past the silhouette instead of clipping it.
pub const GUIDE_PADDING: f32 = 1.4;

/// Full width of a Local-space mirror plane or radial guide, in the object's
/// local units, for a mesh whose local bounds are `local_bounds`.
///
/// `shared_extent` is the half-size of anything else sharing the plane (a rig
/// grown well past the mesh it started from, say), already in local units, or
/// 0: sizing to the mesh alone leaves the plane a small card floating inside a
/// much larger rig.
pub fn local_guide_size(local_bounds: &Aabb, shared_extent: f32) -> f32 {
    let mesh_extent = if local_bounds.is_empty() { 0.0 } else { (local_bounds.size() * 0.5).max_element() };
    let extent = mesh_extent.max(shared_extent);
    if extent <= 0.0 { 2.0 } else { extent.max(0.01) * 2.0 * GUIDE_PADDING }
}

/// Full width of a World-space guide, in world units: it sits at the world
/// origin and has to reach past the object wherever the object has been moved
/// to.
pub fn world_guide_size(world_bounds: &Aabb) -> f32 {
    if world_bounds.is_empty() {
        return 2.0;
    }
    let extent = (world_bounds.size() * 0.5).max_element();
    (world_bounds.center().length() + extent).max(0.01) * 2.0 * GUIDE_PADDING
}

/// The radial guide as line segments (pairs of indices into the points): the
/// axis end to end, a ring around it, and one spoke per repeat, so both the
/// axis and where the sectors fall are visible. Unit size around +Y; rotate +Y
/// onto the radial axis and scale by half the guide size to place it.
pub fn radial_guide_lines(repeats: u32) -> (Vec<Vec3>, Vec<u32>) {
    const RING_SEGMENTS: u32 = 64;
    let around = |k: u32, of: u32| {
        let angle = std::f32::consts::TAU * k as f32 / of as f32;
        Vec3::new(angle.cos(), 0.0, angle.sin())
    };
    let mut points = vec![Vec3::NEG_Y, Vec3::Y];
    let mut lines = vec![0, 1];

    let ring = points.len() as u32;
    for k in 0..RING_SEGMENTS {
        points.push(around(k, RING_SEGMENTS));
        lines.extend([ring + k, ring + (k + 1) % RING_SEGMENTS]);
    }

    let centre = points.len() as u32;
    points.push(Vec3::ZERO);
    let repeats = repeats.max(1);
    for k in 0..repeats {
        points.push(around(k, repeats));
        lines.extend([centre, centre + 1 + k]);
    }
    (points, lines)
}

/// The world matrix of the copy of an object under `op`, a symmetry of the
/// WORLD about `center`: the world map applied after the object's own matrix.
///
/// When `op.reflects()` the matrix has a negative determinant: the copy is the
/// same mesh drawn inside out, so it must be drawn with its winding reversed
/// (front faces clockwise) or it will be culled the wrong way round.
pub fn copy_matrix(op: &SymmetryOp, center: Vec3, source: &Mat4) -> Mat4 {
    Mat4::from_translation(center) * op.to_mat4() * Mat4::from_translation(-center) * *source
}

/// The world matrices of an object's copies under every element of `group`
/// after the identity (the object itself), in the group's order.
pub fn copy_matrices(group: &SymmetryGroup, center: Vec3, source: &Mat4) -> Vec<Mat4> {
    group.iter().skip(1).map(|op| copy_matrix(op, center, source)).collect()
}

/// One live mirror copy: the object reflected across some of the world's
/// coordinate planes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MirrorCopy {
    /// Bit 0 flips X, bit 1 Y, bit 2 Z.
    pub flip_mask: u8,
    /// -1 on each axis this copy is reflected across, +1 on the rest.
    pub signs: Vec3,
}

impl MirrorCopy {
    /// The copy across the axes in `flip_mask` (X = 1, Y = 2, Z = 4).
    pub fn new(flip_mask: u8) -> MirrorCopy {
        let flip_mask = flip_mask & 7;
        MirrorCopy { flip_mask, signs: SymmetryGroup::sign_of_flip_mask(flip_mask as u32) }
    }

    /// "X", "XZ" and so on.
    pub fn axis_name(&self) -> String {
        MirrorRepeater::axis_name_of(self.flip_mask)
    }

    /// Whether the copy is mirror-reversed (an odd number of axes), and so has
    /// to be drawn with its winding reversed.
    pub fn reflects(&self) -> bool {
        self.flip_mask.count_ones() % 2 == 1
    }

    /// This copy as a symmetry of the world about the repeater's centre.
    pub fn op(&self) -> SymmetryOp {
        SymmetryOp::from_sign(self.signs)
    }

    /// Where the live copy is drawn: the original's mesh, at the reflected
    /// position, with the reflected rotation, and a NEGATIVE scale on the
    /// mirrored axes. The negative scale is the whole trick: the copy draws the
    /// original's local geometry reflected.
    pub fn placement(&self, source: &Placement, center: Vec3) -> Placement {
        Placement { scale: source.scale * self.signs, ..self.baked_placement(source, center) }
    }

    /// The same placement as one matrix: the world reflection after the
    /// object's own matrix.
    pub fn matrix(&self, source: &Mat4, center: Vec3) -> Mat4 {
        copy_matrix(&self.op(), center, source)
    }

    /// Where a BAKED copy sits: one whose own local geometry has been
    /// reflected by `signs`. It keeps the original's positive scale, which
    /// renders identically to the live copy's reflected transform.
    pub fn baked_placement(&self, source: &Placement, center: Vec3) -> Placement {
        Placement {
            position: reflect_point(source.position, center, self.signs),
            rotation: reflect_rotation(source.rotation, self.signs),
            scale: source.scale,
        }
    }

    /// Where the ORIGINAL has to be for this copy to be at `copy_position`.
    /// A copy has no transform of its own to write: dragging it moves the
    /// original, reflected back, so dragging the copy right moves the original
    /// left. Reflection is its own inverse, so the same map serves both ways.
    pub fn source_position(&self, copy_position: Vec3, center: Vec3) -> Vec3 {
        reflect_point(copy_position, center, self.signs)
    }

    /// How the ORIGINAL has to be turned for this copy to be turned by
    /// `copy_rotation` (as reported by `baked_placement`, the proper rotation a
    /// gizmo shows).
    pub fn source_rotation(&self, copy_rotation: Quat) -> Quat {
        reflect_rotation(copy_rotation, self.signs)
    }
}

/// A live mirror baked into a real mesh of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct BakedCopy {
    pub copy: MirrorCopy,
    /// The object's arrays reflected in its local space, winding and normals
    /// included. Vertex `i` is still vertex `i`, so a mask carries across
    /// index for index.
    pub geometry: Reflected,
    /// Where the new object goes to sit exactly where the live copy was shown.
    pub placement: Placement,
}

/// What baking an object together with its copies gives.
#[derive(Clone, Debug, PartialEq)]
pub struct BakedTogether {
    /// One mesh in the original's local space.
    pub mesh: TriMesh,
    /// For each vertex of `mesh`, the vertex of the original it came from.
    pub source: Vec<u32>,
    /// And which copy: 0 for the original itself, `k` for the `k`-th copy.
    pub copy: Vec<u32>,
}

impl BakedTogether {
    /// A per-vertex array of the original (a mask), carried onto the baked
    /// mesh: every copy of a vertex gets that vertex's value.
    pub fn carry<T: Copy + Send + Sync>(&self, values: &[T]) -> Vec<T> {
        weld::carry(&self.source, values)
    }
}

/// Bakes an object and its copies under `ops` (symmetries of the WORLD about
/// `center`, the identity not among them) into ONE mesh in the object's local
/// space, welding the vertices where a copy meets the original or another
/// copy closer than `weld_distance`.
///
/// Where the object's rim lies on a mirror plane the halves come out joined
/// along it. This is a join, not a boolean: where a copy passes THROUGH the
/// original the two surfaces simply interpenetrate, and anyone who wants one
/// skin out of that remeshes afterwards.
///
/// Fails only for an object squashed flat by its own scale, whose copies
/// cannot be brought back into its local space.
pub fn bake_together(
    positions: &[Vec3],
    indices: &[u32],
    source: &Mat4,
    ops: &[SymmetryOp],
    center: Vec3,
    weld_distance: f32,
) -> Result<BakedTogether, String> {
    let volume = source.determinant().abs();
    if volume.is_nan() || volume <= 1e-30 {
        let why = "This object is scaled to nothing along an axis, so its mirror copies cannot be baked into it";
        return Err(why.to_string());
    }
    let to_local = source.inverse();
    let mesh = TriMesh::new(positions.to_vec(), indices.to_vec());
    let mut parts = vec![(&mesh, Mat4::IDENTITY)];
    parts.extend(ops.iter().map(|op| (&mesh, to_local * copy_matrix(op, center, source))));
    let joined = join(&parts).mesh;

    let n = positions.len().max(1) as u32;
    let origin = |joined_vertex: u32| (joined_vertex % n, joined_vertex / n);
    let welded = weld::weld_coincident(&joined.positions, &joined.indices, weld_distance);
    let (mesh, kept): (TriMesh, Vec<u32>) = match welded {
        Some(welded) => (welded.mesh, welded.source),
        None => {
            let all = (0..joined.vertex_count() as u32).collect();
            (joined, all)
        }
    };
    let (source, copy): (Vec<u32>, Vec<u32>) = kept.into_iter().map(origin).unzip();
    Ok(BakedTogether { mesh, source, copy })
}

/// Nomad's Mirror on one object: live mirrored copies across world-space
/// planes through `center`. One copy per non-empty combination of the enabled
/// axes (X and Z make three: X, Z and XZ).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MirrorRepeater {
    /// X = 1, Y = 2, Z = 4.
    pub axis_mask: u8,
    /// The point every mirror plane passes through; the world origin unless
    /// set otherwise.
    pub center: Vec3,
}

impl MirrorRepeater {
    /// `None` when no axis is enabled: there is nothing to repeat.
    pub fn new(axis_mask: u8, center: Vec3) -> Option<MirrorRepeater> {
        let axis_mask = axis_mask & 7;
        (axis_mask != 0).then_some(MirrorRepeater { axis_mask, center })
    }

    pub fn axis_name_of(mask: u8) -> String {
        ["X", "Y", "Z"].iter().enumerate().filter(|(bit, _)| mask >> bit & 1 != 0).map(|(_, name)| *name).collect()
    }

    /// The copies, in flip-mask order.
    pub fn copies(&self) -> Vec<MirrorCopy> {
        (1..8u8).filter(|m| m & !self.axis_mask == 0).map(MirrorCopy::new).collect()
    }

    pub fn copy_count(&self) -> usize {
        (1usize << (self.axis_mask & 7).count_ones()) - 1
    }

    /// Turns every copy into real geometry of its own, exactly where it is
    /// shown: the object's arrays reflected in local space (normals carried
    /// across, not recalculated, so duplicated seam vertices do not turn into
    /// hard creases) and the placement that puts them there.
    pub fn bake(
        &self,
        positions: &[Vec3],
        normals: Option<&[Vec3]>,
        indices: &[u32],
        source: &Placement,
    ) -> Vec<BakedCopy> {
        self.copies()
            .into_iter()
            .map(|copy| BakedCopy {
                copy,
                geometry: reflect_geometry(positions, normals, indices, copy.signs),
                placement: copy.baked_placement(source, self.center),
            })
            .collect()
    }

    /// Bakes the object and all its copies into one welded mesh in the
    /// object's local space; see `bake_together`.
    pub fn bake_together(
        &self,
        positions: &[Vec3],
        indices: &[u32],
        source: &Mat4,
        weld_distance: f32,
    ) -> Result<BakedTogether, String> {
        let ops: Vec<SymmetryOp> = self.copies().iter().map(MirrorCopy::op).collect();
        bake_together(positions, indices, source, &ops, self.center, weld_distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{duplicate_positions, edge_census, icosphere, octa_sphere};
    use glam::EulerRot;
    use sculpt_geom::adjacency::connected_components;

    /// The Unity tests' object: off to one side, turned, and scaled.
    fn placed() -> Placement {
        let turn = Quat::from_euler(EulerRot::YXZ, 30f32.to_radians(), 10f32.to_radians(), -20f32.to_radians());
        Placement::new(Vec3::new(0.7, 0.2, -0.3), turn, Vec3::splat(0.8))
    }

    fn settings(x: bool, y: bool, z: bool, radial: u32) -> SymmetrySettings {
        SymmetrySettings {
            mirror: [x, y, z],
            radial: radial > 1,
            radial_count: radial.max(2),
            radial_axis: RadialAxis::Y,
            ..Default::default()
        }
    }

    #[test]
    fn local_space_ops_have_no_offset() {
        let group = settings(true, false, false, 1).group_for(&placed());
        assert_eq!(group.len(), 2);
        assert!(!group[1].has_offset());
        assert!(group[1].diagonal);
        let p = Vec3::new(0.1, 0.2, 0.3);
        assert_eq!(group[1].apply_point(p), group[1].apply(p), "a local op's point map is its linear map, exactly");
    }

    #[test]
    fn world_space_ops_reflect_through_the_world_origin() {
        let (mesh, _) = icosphere(2);
        let object = placed();
        let (to_world, rotation) = (object.matrix(), object.rotation);
        let cases = [
            (true, false, false, 1),
            (false, true, false, 1),
            (true, false, true, 1),
            (false, false, false, 6),
            (true, false, false, 4),
        ];
        for (x, y, z, radial) in cases {
            let mut symmetry = settings(x, y, z, radial);
            symmetry.space = SymmetrySpace::World;
            let local = symmetry.group_for(&object);
            let world = SymmetryGroup::build(x, y, z, radial, Vec3::Y);
            assert_eq!(local.len(), world.len());
            assert_eq!(local.is_group(), world.is_group());

            for k in 0..local.len() {
                for &v in mesh.positions.iter().step_by(17) {
                    // The op in local space must land where the world op sends the world point.
                    let expected = world[k].apply_point(to_world.transform_point3(v));
                    let actual = to_world.transform_point3(local[k].apply_point(v));
                    assert!(expected.distance(actual) < 1e-5, "op {k}: {expected} vs {actual}");

                    let normal = v.normalize();
                    let expected = world[k].apply(rotation * normal);
                    let actual = rotation * local[k].apply(normal);
                    assert!(expected.distance(actual) < 1e-5, "op {k} normal");

                    assert!(local[k].apply_inverse_point(local[k].apply_point(v)).distance(v) < 1e-5, "op {k} inverse");
                }
            }
        }
    }

    #[test]
    fn world_space_at_the_origin_matches_local_exactly() {
        let mut symmetry = settings(true, false, true, 1);
        let local = symmetry.group_for(&Placement::IDENTITY);
        symmetry.space = SymmetrySpace::World;
        let world = symmetry.group_for(&Placement::IDENTITY);
        assert_eq!(local.len(), 4);
        for k in 0..local.len() {
            assert_eq!(world[k], local[k], "op {k}");
        }
    }

    #[test]
    fn world_symmetry_follows_the_object() {
        let mut symmetry = settings(true, false, false, 1);
        symmetry.space = SymmetrySpace::World;
        let mut object = placed();
        let before = symmetry.group_for(&object)[1];
        object.position += Vec3::new(0.25, 0.0, 0.0);
        let after = symmetry.group_for(&object)[1];
        assert_ne!(after.offset, before.offset, "a moved object must get a rebuilt world group");
    }

    #[test]
    fn settings_clamp_the_repeats_and_pick_the_axis() {
        let mut symmetry = SymmetrySettings::default();
        assert!(symmetry.is_off());
        assert_eq!(symmetry.radial_repeats(), 1, "radial off: one stroke");
        assert_eq!(symmetry.group().len(), 1);

        symmetry.radial = true;
        for (asked, used) in [(0, 2), (1, 2), (2, 2), (8, 8), (32, 32), (33, 32), (u32::MAX, 32)] {
            symmetry.radial_count = asked;
            assert_eq!(symmetry.radial_repeats(), used, "asked for {asked}");
            assert_eq!(symmetry.group().len(), used as usize);
        }
        assert!(!symmetry.is_off());

        symmetry.radial_axis = RadialAxis::Custom;
        symmetry.radial_custom_axis = Vec3::new(0.0, 0.0, 3.0);
        assert_eq!(symmetry.radial_axis_vector(), Vec3::Z);
        symmetry.radial_custom_axis = Vec3::ZERO;
        assert_eq!(symmetry.radial_axis_vector(), Vec3::Y, "a zero custom axis falls back to Y");
        symmetry.radial_axis = RadialAxis::X;
        assert_eq!(symmetry.radial_axis_vector(), Vec3::X);

        // A copy reflected across X keeps every setting but a custom axis, which is reflected too.
        symmetry.radial_custom_axis = Vec3::new(1.0, 2.0, 3.0);
        let copy = symmetry.reflected(Vec3::new(-1.0, 1.0, 1.0));
        assert_eq!(copy.radial_custom_axis, Vec3::new(-1.0, 2.0, 3.0));
        assert_eq!(SymmetrySettings { radial_custom_axis: symmetry.radial_custom_axis, ..copy }, symmetry);
    }

    #[test]
    fn copies_are_the_world_reflection_of_the_object() {
        let (mesh, _) = icosphere(2);
        let repeater = MirrorRepeater::new(1 | 4, Vec3::ZERO).unwrap();
        let copies = repeater.copies();
        assert_eq!(copies.len(), 3, "X, Z and XZ");
        assert_eq!(repeater.copy_count(), 3);
        assert_eq!(copies.iter().map(MirrorCopy::axis_name).collect::<Vec<_>>(), ["X", "Z", "XZ"]);
        assert_eq!(copies.iter().map(MirrorCopy::reflects).collect::<Vec<_>>(), [true, true, false]);

        let object = placed();
        let to_world = object.matrix();
        for copy in &copies {
            let drawn = copy.placement(&object, repeater.center).matrix();
            let as_matrix = copy.matrix(&to_world, repeater.center);
            assert_eq!(drawn.determinant() < 0.0, copy.reflects());
            for &v in mesh.positions.iter().step_by(13) {
                let expected = to_world.transform_point3(v) * copy.signs;
                assert!(expected.distance(drawn.transform_point3(v)) < 1e-5, "copy {}", copy.axis_name());
                assert!(expected.distance(as_matrix.transform_point3(v)) < 1e-5, "copy {} as a matrix", copy.axis_name());
            }
        }
    }

    #[test]
    fn copies_follow_a_moved_object_and_a_moved_centre() {
        let (mesh, _) = icosphere(1);
        let center = Vec3::new(0.4, -1.0, 2.0);
        let repeater = MirrorRepeater::new(1, center).unwrap();
        let copy = repeater.copies()[0];
        let turn = Quat::from_euler(EulerRot::YXZ, 1.2, 0.0, 0.09);
        let object = Placement::new(Vec3::new(1.5, -0.4, 0.2), turn, Vec3::new(0.5, 1.0, 2.0));
        let drawn = copy.placement(&object, center).matrix();
        for &v in &mesh.positions {
            let world = object.matrix().transform_point3(v);
            let expected = center + (world - center) * copy.signs;
            assert!(expected.distance(drawn.transform_point3(v)) < 1e-5);
        }
    }

    #[test]
    fn gizmo_on_the_copy_moves_the_object_mirrored() {
        let repeater = MirrorRepeater::new(1, Vec3::ZERO).unwrap();
        let copy = repeater.copies()[0];
        let mut object = placed();
        let shown = copy.baked_placement(&object, repeater.center);
        assert!(shown.position.distance(copy.placement(&object, repeater.center).position) < 1e-6);
        assert!(shown.scale.x > 0.0, "reported as the object's positive scale");

        // Moving the copy left moves the object right.
        let start = object.position;
        object.position = copy.source_position(shown.position + Vec3::new(-0.5, 0.1, 0.0), repeater.center);
        assert!(object.position.distance(start + Vec3::new(0.5, 0.1, 0.0)) < 1e-5);

        // Turning the copy turns the object so that the copy shows the turn asked for.
        let spun = Quat::from_axis_angle(Vec3::Y, 25f32.to_radians()) * shown.rotation;
        object.rotation = copy.source_rotation(spun);
        let now = copy.baked_placement(&object, repeater.center).rotation;
        assert!(now.angle_between(spun) < 1e-3);
    }

    #[test]
    fn no_axes_means_no_repeater() {
        assert_eq!(MirrorRepeater::new(0, Vec3::ZERO), None);
        assert_eq!(MirrorRepeater::new(8, Vec3::ZERO), None, "only the low three bits name axes");
        let all = MirrorRepeater::new(7, Vec3::ZERO).unwrap();
        assert_eq!(all.copy_count(), 7);
        assert_eq!(all.copies().last().unwrap().axis_name(), "XYZ");
        assert_eq!(MirrorRepeater::new(2, Vec3::ZERO).unwrap().copy_count(), 1);
    }

    /// A baked copy is real geometry that sits exactly where the live copy was drawn, wound
    /// outward, with the original's positive scale.
    #[test]
    fn baked_copies_sit_where_the_live_copies_were_shown() {
        let (mesh, _) = icosphere(2);
        let normals: Vec<Vec3> = mesh.positions.iter().map(|p| p.normalize()).collect();
        let repeater = MirrorRepeater::new(1 | 2, Vec3::new(0.1, 0.0, 0.0)).unwrap();
        let object = placed();
        let baked = repeater.bake(&mesh.positions, Some(&normals), &mesh.indices, &object);
        assert_eq!(baked.len(), 3);
        for bake in &baked {
            assert_eq!(bake.placement.scale, object.scale);
            let live = bake.copy.placement(&object, repeater.center).matrix();
            let own = bake.placement.matrix();
            for (i, &v) in mesh.positions.iter().enumerate() {
                assert!(live.transform_point3(v).distance(own.transform_point3(bake.geometry.positions[i])) < 1e-5);
            }
            let rebuilt = TriMesh::new(bake.geometry.positions.clone(), bake.geometry.indices.clone());
            assert!(rebuilt.signed_volume() > 0.0, "copy {} came out inside out", bake.copy.axis_name());
            assert!((rebuilt.signed_volume() - mesh.signed_volume()).abs() < 1e-6);
            let baked_normals = bake.geometry.normals.as_ref().unwrap();
            assert!(baked_normals.iter().zip(&normals).all(|(b, n)| *b == *n * bake.copy.signs));
        }
    }

    /// Half a sphere whose rim lies on the mirror plane, baked with its copy: one closed sphere,
    /// the seam shared rather than doubled, the mask mirrored along with it.
    #[test]
    fn half_a_sphere_baked_with_its_mirror_copy_is_one_closed_sphere() {
        let full = octa_sphere(10, 1.0);
        let mut slot = vec![u32::MAX; full.vertex_count()];
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        for t in 0..full.triangle_count() {
            if full.triangle(t).iter().map(|p| p.x).sum::<f32>() <= 0.0 {
                continue;
            }
            for &i in &full.indices[t * 3..t * 3 + 3] {
                if slot[i as usize] == u32::MAX {
                    slot[i as usize] = positions.len() as u32;
                    positions.push(full.positions[i as usize]);
                }
                indices.push(slot[i as usize]);
            }
        }
        let mask: Vec<f32> = positions.iter().map(|p| p.y).collect();
        let repeater = MirrorRepeater::new(1, Vec3::ZERO).unwrap();

        let baked = repeater.bake_together(&positions, &indices, &Mat4::IDENTITY, 1e-5).unwrap();
        assert_eq!(baked.mesh.vertex_count(), full.vertex_count());
        assert_eq!(baked.mesh.triangle_count(), full.triangle_count());
        assert_eq!(edge_census(&baked.mesh), (0, 0));
        assert_eq!(duplicate_positions(&baked.mesh.positions), 0);
        assert!((baked.mesh.signed_volume() - full.signed_volume()).abs() < 1e-9);
        let carried = baked.carry(&mask);
        for (k, p) in baked.mesh.positions.iter().enumerate() {
            assert_eq!(carried[k], p.y, "the mask follows the surface onto the copy");
            assert_eq!(p.x < 0.0, baked.copy[k] == 1, "vertex {k} is filed under the wrong copy");
            let signs = if baked.copy[k] == 1 { Vec3::new(-1.0, 1.0, 1.0) } else { Vec3::ONE };
            assert_eq!(positions[baked.source[k] as usize] * signs, *p);
        }

        // The same object moved and turned, with the plane moved to match: the result, being in
        // the object's own space, is the same sphere.
        let object = placed();
        let to_world = object.matrix();
        let plane_point = to_world.transform_point3(Vec3::ZERO);
        let plane_normal = to_world.transform_vector3(Vec3::X);
        let across = SymmetryOp::reflection(plane_normal);
        let tilted = bake_together(&positions, &indices, &to_world, &[across], plane_point, 1e-4).unwrap();
        assert_eq!(tilted.mesh.vertex_count(), full.vertex_count());
        assert_eq!(edge_census(&tilted.mesh), (0, 0));
        assert!((tilted.mesh.signed_volume() - full.signed_volume()).abs() < 1e-4);
    }

    /// A quarter of a sphere under four-fold radial symmetry about Y: three copies, and baked
    /// together they close up into the whole sphere.
    #[test]
    fn radial_copies_of_a_quarter_close_into_a_sphere() {
        let full = octa_sphere(8, 1.0);
        let mut slot = vec![u32::MAX; full.vertex_count()];
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        for t in 0..full.triangle_count() {
            let centroid = full.triangle(t).iter().sum::<Vec3>();
            if centroid.x <= 0.0 || centroid.z <= 0.0 {
                continue;
            }
            for &i in &full.indices[t * 3..t * 3 + 3] {
                if slot[i as usize] == u32::MAX {
                    slot[i as usize] = positions.len() as u32;
                    positions.push(full.positions[i as usize]);
                }
                indices.push(slot[i as usize]);
            }
        }
        let group = settings(false, false, false, 4).group();
        let matrices = copy_matrices(&group, Vec3::ZERO, &Mat4::IDENTITY);
        assert_eq!(matrices.len(), 3);
        assert!(matrices.iter().all(|m| m.determinant() > 0.0), "rotations do not turn anything inside out");
        // Each copy is the quarter turned a further quarter turn about Y.
        let p = Vec3::new(0.6, 0.3, 0.2);
        let turned: Vec<Vec3> = matrices.iter().map(|m| m.transform_point3(p)).collect();
        assert!(turned.iter().all(|q| (q.y, (q.x * q.x + q.z * q.z)) == (p.y, p.x * p.x + p.z * p.z)));
        assert!(turned[1].distance(Vec3::new(-0.6, 0.3, -0.2)) < 1e-6, "the second copy is the half turn");

        let baked = bake_together(&positions, &indices, &Mat4::IDENTITY, &group.ops()[1..], Vec3::ZERO, 1e-5).unwrap();
        assert_eq!(baked.mesh.vertex_count(), full.vertex_count());
        assert_eq!(baked.mesh.triangle_count(), full.triangle_count());
        assert_eq!(edge_census(&baked.mesh), (0, 0));
        assert!((baked.mesh.signed_volume() - full.signed_volume()).abs() < 1e-9);
        assert_eq!(baked.copy.iter().max(), Some(&3));
    }

    /// Radial symmetry with a mirror: the dihedral group. Copies that do not touch stay separate
    /// pieces, each the right way out, mirrored ones included.
    #[test]
    fn dihedral_copies_of_a_separate_object_are_separate_and_outward() {
        let (ball, _) = icosphere(1);
        let object = Placement::new(Vec3::new(2.0, 0.3, 0.7), Quat::from_rotation_z(0.4), Vec3::splat(0.5));
        let group = settings(true, false, false, 5).group();
        assert_eq!(group.len(), 10);
        let center = Vec3::new(0.0, 1.0, 0.0);

        let matrices = copy_matrices(&group, center, &object.matrix());
        let mirrored = matrices.iter().filter(|m| m.determinant() < 0.0).count();
        assert_eq!((matrices.len(), mirrored), (9, 5));

        let copies = &group.ops()[1..];
        let baked = bake_together(&ball.positions, &ball.indices, &object.matrix(), copies, center, 1e-5).unwrap();
        assert_eq!(baked.mesh.triangle_count(), 10 * ball.triangle_count());
        assert_eq!(connected_components(baked.mesh.vertex_count(), &baked.mesh.indices).count, 10);
        assert_eq!(edge_census(&baked.mesh), (0, 0));
        assert!((baked.mesh.signed_volume() - 10.0 * ball.signed_volume()).abs() < 1e-4);
        // Every copy's centre is as far from the axis through `center` as the object's own.
        let world = |k: usize| object.matrix().transform_point3(baked.mesh.positions[k]) - center;
        let radius = |p: Vec3| (p.x * p.x + p.z * p.z).sqrt();
        assert!((0..baked.mesh.vertex_count()).all(|k| radius(world(k)) > 1.4 && radius(world(k)) < 2.9));
    }

    #[test]
    fn a_flattened_object_cannot_be_baked() {
        let (ball, _) = icosphere(1);
        let flat = Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0));
        let repeater = MirrorRepeater::new(1, Vec3::ZERO).unwrap();
        let error = repeater.bake_together(&ball.positions, &ball.indices, &flat, 1e-5).unwrap_err();
        assert!(error.starts_with("This object is scaled to nothing"), "{error}");
    }

    #[test]
    fn guides_cover_the_object_with_room_to_spare() {
        let bounds = Aabb::new(Vec3::new(-1.0, -3.0, -0.5), Vec3::new(1.0, 3.0, 0.5));
        assert!((local_guide_size(&bounds, 0.0) - 3.0 * 2.0 * GUIDE_PADDING).abs() < 1e-6);
        // A larger rig sharing the plane wins.
        assert!((local_guide_size(&bounds, 10.0) - 10.0 * 2.0 * GUIDE_PADDING).abs() < 1e-5);
        assert_eq!(local_guide_size(&Aabb::EMPTY, 0.0), 2.0);
        // A world guide sits at the origin and has to reach an object that has been moved away.
        let moved = Aabb::new(Vec3::new(9.0, -1.0, -1.0), Vec3::new(11.0, 1.0, 1.0));
        assert!((world_guide_size(&moved) - 11.0 * 2.0 * GUIDE_PADDING).abs() < 1e-5);
        assert_eq!(world_guide_size(&Aabb::EMPTY), 2.0);

        for repeats in [0u32, 1, 2, 7, 32] {
            let (points, lines) = radial_guide_lines(repeats);
            let spokes = repeats.max(1) as usize;
            assert_eq!(points.len(), 2 + 64 + 1 + spokes);
            assert_eq!(lines.len(), 2 * (1 + 64 + spokes));
            assert!(lines.iter().all(|&i| (i as usize) < points.len()));
            assert!(points[2..].iter().all(|p| p.y == 0.0 && p.length() < 1.0 + 1e-6));
        }
    }
}
