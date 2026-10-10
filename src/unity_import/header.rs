//! The JSON section of a `.sculpt`, as the Unity app's classes wrote it.
//!
//! One struct per C# class of `SculptSaveData.cs` (and the settings classes
//! it holds), with the class's own field names and the values each field
//! starts with there. Those starting values matter: the Unity app read its
//! files with JsonUtility, which leaves a field the file lacks as the class
//! made it, so that is what a file from before a field existed means. This
//! reader does the same, and goes one further: a field of the wrong kind (a
//! word where a number belongs) is passed over rather than costing the file.
//!
//! Everything in here is still in the Unity app's axes. Nothing is mapped
//! until `convert` in the parent module.

use glam::Vec3;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::io;

/// A `Vector3` as JsonUtility writes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Xyz {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Xyz {
    pub(super) const ONE: Xyz = Xyz { x: 1.0, y: 1.0, z: 1.0 };

    pub(super) fn vec3(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

/// A `Quaternion`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Xyzw {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Xyzw {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }
    }
}

/// A `Color`, in the display values the Unity panels showed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for Rgba {
    fn default() -> Self {
        Self { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }
    }
}

impl Rgba {
    pub(super) const fn grey(v: f32) -> Rgba {
        Rgba { r: v, g: v, b: v, a: 1.0 }
    }

    pub(super) fn rgb(self, fallback: [f32; 3]) -> [f32; 3] {
        let c = [self.r, self.g, self.b];
        if c.iter().all(|v| v.is_finite()) { c.map(|v| v.clamp(0.0, 1.0)) } else { fallback }
    }
}

/// One entry of the file's object list: `SculptSaveData.ObjectEntry`. The
/// fields and what they start as are the C# class's, which is what a file
/// from before a field existed reads back as.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ObjectEntry {
    pub name: String,
    pub position: Xyz,
    pub rotation: Xyzw,
    pub scale: Xyz,
    pub visible: bool,
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub mirror_z: bool,
    pub show_mirror_planes: bool,
    pub radial: bool,
    /// 0 in a file from before radial symmetry: "absent", not "off".
    pub radial_count: i32,
    pub radial_axis: i32,
    pub radial_custom_axis: Xyz,
    pub vertex_count: i64,
    pub triangle_index_count: i64,
    pub has_mask: bool,
    pub has_hidden: bool,
    /// 0 the object's own space, 1 the world's.
    pub symmetry_space: i32,
    /// Live mirror copies: X = 1, Y = 2, Z = 4, and the point the planes pass through.
    pub mirror_repeater_axes: i32,
    pub mirror_repeater_center: Xyz,
    pub instances: Vec<InstanceEntry>,
    pub array: ArrayEntry,
}

impl Default for ObjectEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            position: Xyz::default(),
            rotation: Xyzw::default(),
            scale: Xyz::ONE,
            visible: true,
            mirror_x: false,
            mirror_y: false,
            mirror_z: false,
            show_mirror_planes: true,
            radial: false,
            radial_count: 0,
            radial_axis: 0,
            radial_custom_axis: Xyz::default(),
            vertex_count: 0,
            triangle_index_count: 0,
            has_mask: false,
            has_hidden: false,
            symmetry_space: 0,
            mirror_repeater_axes: 0,
            mirror_repeater_center: Xyz::default(),
            instances: Vec::new(),
            array: ArrayEntry::default(),
        }
    }
}

/// A linked copy of an object: a second placement of its mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InstanceEntry {
    pub name: String,
    pub position: Xyz,
    pub rotation: Xyzw,
    pub scale: Xyz,
    pub visible: bool,
}

impl Default for InstanceEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            position: Xyz::default(),
            rotation: Xyzw::default(),
            scale: Xyz::ONE,
            visible: true,
        }
    }
}

/// A live array of an object. `kind` 0 is none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ArrayEntry {
    pub kind: i32,
    pub count: i32,
    pub end_scale: f32,
    pub mid_scale: f32,
    pub closed: bool,
    pub follow: bool,
    pub space_by_size: bool,
    pub axis: i32,
    pub sweep: f32,
    /// Handle points, as offsets from the object's position.
    pub points: Vec<Xyz>,
}

impl Default for ArrayEntry {
    fn default() -> Self {
        Self {
            kind: 0,
            count: 0,
            end_scale: 1.0,
            mid_scale: 1.0,
            closed: false,
            follow: true,
            space_by_size: true,
            axis: 1,
            sweep: 360.0,
            points: Vec::new(),
        }
    }
}

/// `SculptMaterialController.Settings`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Material {
    pub base_color: Rgba,
    pub metallic: f32,
    pub smoothness: f32,
    pub normal_strength: f32,
    pub normal_noise_scale: f32,
    pub flat_shading: bool,
    pub screen_cavity_enabled: bool,
    pub cavity_ridge: f32,
    pub cavity_valley: f32,
    pub matcap_enabled: bool,
    /// The picture's file name, without its folder or extension.
    pub matcap_name: String,
    pub matcap_intensity: f32,
    pub matcap_tint_strength: f32,

    pub lure_enabled: bool,
    pub lure_preset_id: String,
    pub lure_flake_size: f32,
    pub lure_flake_amount: f32,
    pub lure_sparkle: f32,
    pub lure_translucency: f32,
    pub lure_gloss: f32,
    pub lure_reference_size: f32,

    pub metal_enabled: bool,
    pub metal_preset_id: String,
    pub metal_exposure: f32,
    pub metal_edge_wear: f32,
    pub metal_wash: f32,
    pub metal_detail: f32,
    pub metal_pattern_size: f32,
    pub metal_gloss: f32,
    pub metal_pattern_seed: f32,
    pub metal_reference_size: f32,

    pub clay_enabled: bool,
    pub clay_preset_id: String,
    pub clay_gloss: f32,
    pub clay_wetness: f32,
    pub clay_subsurface: f32,
    pub clay_recess: f32,
    pub clay_detail: f32,
    pub clay_grain: f32,
    pub clay_reference_size: f32,

    pub wood_enabled: bool,
    pub wood_preset_id: String,
    pub wood_grain_axis: i32,
    pub wood_grain: f32,
    pub wood_grain_scale: f32,
    pub wood_edge_wear: f32,
    pub wood_patina: f32,
    pub wood_detail: f32,
    pub wood_sheen: f32,
    pub wood_cracks: f32,
    pub wood_grain_seed: f32,
    pub wood_reference_size: f32,

    pub glow_enabled: bool,
    pub glow_preset_id: String,
    pub glow_intensity: f32,
    pub glow_coverage: f32,
    pub glow_rim: f32,
    pub glow_pattern_size: f32,
    pub glow_speed: f32,
    pub glow_pulse: f32,
    pub glow_dim: f32,
    pub glow_hue_shift: f32,
    pub glow_reference_size: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: Rgba::grey(0.5),
            metallic: 0.0,
            smoothness: 0.4,
            normal_strength: 0.0,
            normal_noise_scale: 60.0,
            flat_shading: false,
            screen_cavity_enabled: true,
            cavity_ridge: 1.0,
            cavity_valley: 1.0,
            matcap_enabled: false,
            matcap_name: String::new(),
            matcap_intensity: 1.0,
            matcap_tint_strength: 0.0,
            lure_enabled: false,
            lure_preset_id: String::new(),
            lure_flake_size: 1.0,
            lure_flake_amount: 1.0,
            lure_sparkle: 1.0,
            lure_translucency: 1.0,
            lure_gloss: 0.8,
            lure_reference_size: 0.0,
            metal_enabled: false,
            metal_preset_id: String::new(),
            metal_exposure: 1.0,
            metal_edge_wear: 1.0,
            metal_wash: 1.0,
            metal_detail: 1.0,
            metal_pattern_size: 1.0,
            metal_gloss: 1.0,
            metal_pattern_seed: 0.0,
            metal_reference_size: 0.0,
            clay_enabled: false,
            clay_preset_id: String::new(),
            clay_gloss: 1.0,
            clay_wetness: 1.0,
            clay_subsurface: 1.0,
            clay_recess: 1.0,
            clay_detail: 1.0,
            clay_grain: 1.0,
            clay_reference_size: 0.0,
            wood_enabled: false,
            wood_preset_id: String::new(),
            wood_grain_axis: 0,
            wood_grain: 1.0,
            wood_grain_scale: 1.0,
            wood_edge_wear: 1.0,
            wood_patina: 1.0,
            wood_detail: 1.0,
            wood_sheen: 1.0,
            wood_cracks: 1.0,
            wood_grain_seed: 0.0,
            wood_reference_size: 0.0,
            glow_enabled: false,
            glow_preset_id: String::new(),
            glow_intensity: 1.0,
            glow_coverage: 1.0,
            glow_rim: 1.0,
            glow_pattern_size: 1.0,
            glow_speed: 1.0,
            glow_pulse: 1.0,
            glow_dim: 1.0,
            glow_hue_shift: 0.0,
            glow_reference_size: 0.0,
        }
    }
}

/// `SculptSaveData.EnvironmentSettings`: lighting, background, HDRI, effects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Environment {
    pub lighting_preset: String,
    pub lighting_five_point: bool,
    pub lighting_brightness: f32,
    pub lighting_rotation: f32,
    pub lighting_follow_camera: bool,
    pub lighting_world_yaw: f32,
    pub lighting_shadows: bool,

    /// 0 flat, 1 gradient, 2 the HDRI.
    pub background_mode: i32,
    /// The flat colour, or the bottom of the gradient.
    pub background_color_a: Rgba,
    /// The top of the gradient.
    pub background_color_b: Rgba,
    pub gradient_bias: f32,

    pub hdri_enabled: bool,
    pub hdri_path: String,
    pub hdri_rotation: f32,
    pub hdri_exposure: f32,
    pub hdri_ambient_intensity: f32,
    pub hdri_reflection_intensity: f32,

    /// Whether the scene that saved the file had effects at all; without
    /// them the numbers below are zeros that mean nothing.
    pub post_available: bool,
    pub bloom_enabled: bool,
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    pub vignette_enabled: bool,
    pub vignette_intensity: f32,
    pub vignette_smoothness: f32,
    pub dof_enabled: bool,
    pub dof_focus_distance: f32,
    pub dof_aperture: f32,
    pub color_adjustments_enabled: bool,
    pub saturation: f32,
    pub contrast: f32,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            lighting_preset: String::new(),
            lighting_five_point: false,
            lighting_brightness: 1.0,
            lighting_rotation: 0.0,
            lighting_follow_camera: true,
            lighting_world_yaw: 0.0,
            lighting_shadows: true,
            background_mode: 0,
            background_color_a: Rgba::grey(0.0),
            background_color_b: Rgba::grey(0.5),
            gradient_bias: 1.0,
            hdri_enabled: false,
            hdri_path: String::new(),
            hdri_rotation: 0.0,
            hdri_exposure: 1.0,
            hdri_ambient_intensity: 1.0,
            hdri_reflection_intensity: 1.0,
            post_available: false,
            bloom_enabled: false,
            bloom_intensity: 0.0,
            bloom_threshold: 0.0,
            vignette_enabled: false,
            vignette_intensity: 0.0,
            vignette_smoothness: 0.0,
            dof_enabled: false,
            dof_focus_distance: 0.0,
            dof_aperture: 0.0,
            color_adjustments_enabled: false,
            saturation: 0.0,
            contrast: 0.0,
        }
    }
}

/// `CameraOrbitController.SavedView`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// False in a file from before the view was kept.
    pub valid: bool,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub pivot: Xyz,
    pub orthographic: bool,
}

/// One custom falloff curve of the brush block.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FalloffCurve {
    /// Which brush, by the Unity app's numbering (see [`Brush::current_brush`]).
    pub brush: i32,
    pub points: Vec<Xy>,
}

/// A `Vector2`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Xy {
    pub x: f32,
    pub y: f32,
}

/// `SculptController.Settings`: every brush setting the Unity app kept with
/// a scene. Here brush tuning is the user's own (it lives in the settings
/// file and no file that is opened changes it), so of all this only dynamic
/// topology is used (see [`SavedDynamicTopology`]); the rest is read so that
/// it can be.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Brush {
    pub brush_strength: f32,
    /// Brush size on the model, in world units.
    pub brush_radius: f32,
    pub world_space_brush_size: bool,
    /// Brush size on screen, in pixels.
    pub brush_screen_radius: f32,
    /// 0 Move, 1 Clay, 2 Smooth, 3 Crease, 4 Inflate, 5 Flatten, 6 Pose,
    /// 7 Standard, 8 Layer, 9 Snake Hook. The per-brush lists below are in
    /// the same order.
    pub current_brush: i32,
    pub is_positive: bool,
    pub accumulate: bool,
    pub accumulate_strength: f32,
    pub front_facing_only: bool,
    pub build_up_on_hold: bool,
    pub surface_relax: f32,
    pub pose_rigidity: f32,
    pub pose_segments: i32,
    pub clay_height_factor: f32,
    pub clay_tip_roundness: f32,
    pub clay_edge_softness: f32,
    pub clay_pressure_radius_influence: f32,
    pub clay_pressure_softness_influence: f32,
    pub use_alpha: bool,
    pub alpha_type: i32,
    pub alpha_rotation: f32,
    pub alpha_scale: f32,
    pub invert_alpha: bool,
    pub crease_pinch: f32,
    pub crease_depth_factor: f32,
    pub mask_hardness: f32,
    pub pressure_floor: f32,
    pub pressure_curve: f32,
    pub remesh_resolution: i32,
    pub quad_remesh_target: i32,
    pub dynamic_topology: bool,
    /// 0 in a file from before dynamic topology: the default.
    pub dynamic_topology_detail: f32,
    pub use_burst_jobs: bool,
    pub show_wireframe_gizmo: bool,
    pub mask_paint_mode: bool,
    pub per_brush_strength: Vec<f32>,
    pub falloff_curves: Vec<FalloffCurve>,
    pub per_brush_focal_shift: Vec<f32>,
    pub per_brush_polarity: Vec<bool>,
    pub per_brush_accumulate: Vec<bool>,
    pub per_brush_accumulate_strength: Vec<f32>,
    pub per_brush_front_facing_only: Vec<bool>,
}

/// The JSON section of a `.sculpt`, as the file has it: Unity's axes, Unity's names.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// When it was saved and by which Unity; for a person, nothing reads them.
    pub saved_utc: String,
    pub unity_version: String,
    pub brush: Brush,
    pub material: Material,
    pub environment: Environment,
    pub camera: View,
    pub objects: Vec<ObjectEntry>,
    /// Which object was the selected one; -1 for none.
    pub primary_selection_index: i64,
}

/// One part of the JSON, with whatever it lacks (or has wrong) taken from
/// the C# class's own starting values, as JsonUtility did.
fn section<T: Default + Serialize + DeserializeOwned>(root: &Map<String, Value>, key: &str) -> T {
    root.get(key).and_then(|value| io::read_over(&T::default(), value)).unwrap_or_default()
}

impl Header {
    /// Reads the JSON. Only what the arrays' layout hangs on is strict: an
    /// object entry must say how many vertices and corners it has. Anything
    /// else that is missing or malformed falls back to what Unity starts it as.
    pub(super) fn parse(json: &[u8]) -> Option<Header> {
        let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(json) else { return None };
        let text = |key: &str| root.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        let mut objects = Vec::new();
        if let Some(entries) = root.get("objects") {
            for entry in entries.as_array()? {
                let count = |key: &str| entry.get(key).and_then(Value::as_i64);
                let flag = |key: &str| match entry.get(key) {
                    None | Some(Value::Null) => Some(false),
                    Some(value) => value.as_bool(),
                };
                let mut object: ObjectEntry = io::read_over(&ObjectEntry::default(), entry)?;
                object.vertex_count = count("vertexCount")?;
                object.triangle_index_count = count("triangleIndexCount")?;
                object.has_mask = flag("hasMask")?;
                object.has_hidden = flag("hasHidden")?;
                objects.push(object);
            }
        }
        Some(Header {
            saved_utc: text("savedUtc"),
            unity_version: text("unityVersion"),
            brush: section(&root, "brush"),
            material: section(&root, "material"),
            environment: section(&root, "environment"),
            camera: section(&root, "camera"),
            objects,
            primary_selection_index: root.get("primarySelectionIndex").and_then(Value::as_i64).unwrap_or(-1),
        })
    }
}
