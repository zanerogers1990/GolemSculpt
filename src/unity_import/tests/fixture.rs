//! Writes `.sculpt` files the way the Unity app did, for the tests.
//!
//! This follows `SceneSerializer.Save` byte for byte: the magic, the version
//! (2 only when an object has a hidden block), the JSON's length, the JSON
//! in the order JsonUtility wrote its fields, then each object's vertices,
//! normals, triangles, mask (only if anything is masked) and hidden flags
//! (only if it has any). Everything in here is in the Unity app's own axes
//! and winding; nothing has been through `axes`.

use std::path::PathBuf;

use glam::{Quat, Vec3};

/// A number as JsonUtility writes a float: the double nearest it, in full.
pub fn float(value: f32) -> String {
    format!("{:?}", value as f64)
}

pub fn xyz(v: Vec3) -> String {
    format!("{{\"x\":{},\"y\":{},\"z\":{}}}", float(v.x), float(v.y), float(v.z))
}

pub fn xyzw(q: Quat) -> String {
    format!("{{\"x\":{},\"y\":{},\"z\":{},\"w\":{}}}", float(q.x), float(q.y), float(q.z), float(q.w))
}

pub fn rgba(c: [f32; 4]) -> String {
    format!("{{\"r\":{},\"g\":{},\"b\":{},\"a\":{}}}", float(c[0]), float(c[1]), float(c[2]), float(c[3]))
}

pub fn text(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

/// One block of the JSON: its fields in order, each already JSON text.
#[derive(Clone)]
pub struct Block(Vec<(String, String)>);

impl Block {
    pub fn new(fields: &[(&str, String)]) -> Block {
        Block(fields.iter().map(|(key, value)| ((*key).to_owned(), value.clone())).collect())
    }

    /// Changes a field, or adds it at the end.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> &mut Block {
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(field) => field.1 = value,
            None => self.0.push((key.to_owned(), value)),
        }
        self
    }

    /// Takes a field out, as a build from before it existed would not write it.
    pub fn remove(&mut self, key: &str) -> &mut Block {
        self.0.retain(|(k, _)| k != key);
        self
    }

    pub fn json(&self) -> String {
        let fields: Vec<String> = self.0.iter().map(|(key, value)| format!("{}:{value}", text(key))).collect();
        format!("{{{}}}", fields.join(","))
    }
}

/// A linked copy of an object.
#[derive(Clone)]
pub struct Instance {
    pub name: String,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub visible: bool,
}

/// A live array on an object.
#[derive(Clone)]
pub struct Array {
    pub kind: i32,
    pub count: i32,
    pub end_scale: f32,
    pub mid_scale: f32,
    pub closed: bool,
    pub follow: bool,
    pub space_by_size: bool,
    pub axis: i32,
    pub sweep: f32,
    pub points: Vec<Vec3>,
}

impl Array {
    /// A straight run of `count` from the object to `end`.
    pub fn line(count: i32, end: Vec3) -> Array {
        Array {
            kind: 1,
            count,
            end_scale: 1.0,
            mid_scale: 1.0,
            closed: false,
            follow: true,
            space_by_size: true,
            axis: 1,
            sweep: 360.0,
            points: vec![Vec3::ZERO, end],
        }
    }

    fn json(&self) -> String {
        let points: Vec<String> = self.points.iter().map(|p| xyz(*p)).collect();
        Block::new(&[
            ("kind", self.kind.to_string()),
            ("count", self.count.to_string()),
            ("endScale", float(self.end_scale)),
            ("midScale", float(self.mid_scale)),
            ("closed", self.closed.to_string()),
            ("follow", self.follow.to_string()),
            ("spaceBySize", self.space_by_size.to_string()),
            ("axis", self.axis.to_string()),
            ("sweep", float(self.sweep)),
            ("points", format!("[{}]", points.join(","))),
        ])
        .json()
    }

    fn none() -> Array {
        Array { kind: 0, count: 0, points: Vec::new(), ..Array::line(0, Vec3::ZERO) }
    }
}

/// One object as the Unity app held it.
#[derive(Clone)]
pub struct Object {
    pub name: String,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub visible: bool,
    pub mirror: [bool; 3],
    pub show_planes: bool,
    pub radial: bool,
    pub radial_count: i32,
    pub radial_axis: i32,
    pub radial_custom_axis: Vec3,
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<i32>,
    /// The object's mask as it was in memory; only written if any of it is set.
    pub mask: Option<Vec<f32>>,
    /// One flag per triangle, when part of the object was hidden.
    pub hidden: Option<Vec<bool>>,
    pub symmetry_space: i32,
    pub repeater_axes: i32,
    pub repeater_center: Vec3,
    pub instances: Vec<Instance>,
    pub array: Option<Array>,
}

impl Object {
    /// An object at the origin with nothing switched on.
    pub fn new(name: &str, vertices: Vec<Vec3>, triangles: Vec<i32>) -> Object {
        Object {
            name: name.to_owned(),
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            visible: true,
            mirror: [false; 3],
            show_planes: true,
            radial: false,
            radial_count: 8,
            radial_axis: 1,
            radial_custom_axis: Vec3::Y,
            vertices,
            triangles,
            mask: None,
            hidden: None,
            symmetry_space: 0,
            repeater_axes: 0,
            repeater_center: Vec3::ZERO,
            instances: Vec::new(),
            array: None,
        }
    }

    pub fn placed(mut self, position: Vec3, rotation: Quat, scale: Vec3) -> Object {
        (self.position, self.rotation, self.scale) = (position, rotation, scale);
        self
    }

    /// A point of the object's own space in Unity's world, by Unity's rule:
    /// scale, then rotation, then position.
    pub fn world(&self, local: Vec3) -> Vec3 {
        self.position + self.rotation * (self.scale * local)
    }

    fn has_mask(&self) -> bool {
        self.mask.as_ref().is_some_and(|mask| mask.len() == self.vertices.len() && mask.iter().any(|&m| m > 0.0))
    }

    fn has_hidden(&self) -> bool {
        self.hidden.as_ref().is_some_and(|hidden| hidden.len() == self.triangles.len() / 3)
    }

    fn entry(&self, era: Era) -> Block {
        let instances: Vec<String> = self
            .instances
            .iter()
            .map(|i| {
                Block::new(&[
                    ("name", text(&i.name)),
                    ("position", xyz(i.position)),
                    ("rotation", xyzw(i.rotation)),
                    ("scale", xyz(i.scale)),
                    ("visible", i.visible.to_string()),
                ])
                .json()
            })
            .collect();
        let mut entry = Block::new(&[
            ("name", text(&self.name)),
            ("position", xyz(self.position)),
            ("rotation", xyzw(self.rotation)),
            ("scale", xyz(self.scale)),
            ("visible", self.visible.to_string()),
            ("mirrorX", self.mirror[0].to_string()),
            ("mirrorY", self.mirror[1].to_string()),
            ("mirrorZ", self.mirror[2].to_string()),
            ("showMirrorPlanes", self.show_planes.to_string()),
            ("radial", self.radial.to_string()),
            ("radialCount", self.radial_count.to_string()),
            ("radialAxis", self.radial_axis.to_string()),
            ("radialCustomAxis", xyz(self.radial_custom_axis)),
            ("vertexCount", self.vertices.len().to_string()),
            ("triangleIndexCount", self.triangles.len().to_string()),
            ("hasMask", self.has_mask().to_string()),
            ("hasHidden", self.has_hidden().to_string()),
            ("symmetrySpace", self.symmetry_space.to_string()),
            ("mirrorRepeaterAxes", self.repeater_axes.to_string()),
            ("mirrorRepeaterCenter", xyz(self.repeater_center)),
            ("instances", format!("[{}]", instances.join(","))),
            ("array", self.array.clone().unwrap_or_else(Array::none).json()),
        ]);
        if era == Era::BeforeHiding {
            // What the first builds wrote: none of the later fields, and the
            // linked-mirror pair that was since retired.
            for later in [
                "radial",
                "radialCount",
                "radialAxis",
                "radialCustomAxis",
                "hasHidden",
                "symmetrySpace",
                "mirrorRepeaterAxes",
                "mirrorRepeaterCenter",
                "instances",
                "array",
            ] {
                entry.remove(later);
            }
            entry
                .set("mirrorLinked", "false")
                .set("mirrorLinkTwin", "0")
                .set("mirrorLinkCenter", xyz(Vec3::ZERO))
                .set("mirrorLinkSigns", xyz(Vec3::ZERO));
        }
        entry
    }
}

/// Which build of the Unity app wrote the file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Era {
    /// Before faces could be hidden: fewer fields per object, always version 1.
    BeforeHiding,
    Current,
}

/// A whole `.sculpt`.
#[derive(Clone)]
pub struct Sculpt {
    pub era: Era,
    pub brush: Block,
    pub material: Block,
    pub environment: Block,
    pub camera: Block,
    pub objects: Vec<Object>,
    pub primary: i32,
}

impl Sculpt {
    /// A scene of `objects` with every setting as the Unity app starts it.
    pub fn new(objects: Vec<Object>) -> Sculpt {
        Sculpt {
            era: Era::Current,
            brush: brush(),
            material: material(),
            environment: environment(),
            camera: camera(),
            objects,
            primary: 0,
        }
    }

    /// The version `Save` stamps: 2 only when some object has a hidden block.
    pub fn version(&self) -> i32 {
        if self.objects.iter().any(Object::has_hidden) { 2 } else { 1 }
    }

    pub fn json(&self) -> String {
        let objects: Vec<String> = self.objects.iter().map(|o| o.entry(self.era).json()).collect();
        Block::new(&[
            ("savedUtc", text("2026-10-09T01:08:51.9033074Z")),
            ("unityVersion", text("6000.3.22f1")),
            ("brush", self.brush.json()),
            ("material", self.material.json()),
            ("environment", self.environment.json()),
            ("camera", self.camera.json()),
            ("objects", format!("[{}]", objects.join(","))),
            ("primarySelectionIndex", self.primary.to_string()),
        ])
        .json()
    }

    /// The file's bytes, with `json` in place of the scene's own (for files
    /// whose description is wrong on purpose).
    pub fn bytes_with(&self, version: i32, json: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"SCLPTSV\0");
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&(json.len() as i32).to_le_bytes());
        out.extend_from_slice(json.as_bytes());
        for object in &self.objects {
            let floats =
                |out: &mut Vec<u8>, values: &[f32]| values.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
            for v in &object.vertices {
                floats(&mut out, &v.to_array());
            }
            // Normals nothing should ever read as positions: not numbers.
            for _ in &object.vertices {
                floats(&mut out, &[f32::NAN; 3]);
            }
            for i in &object.triangles {
                out.extend_from_slice(&i.to_le_bytes());
            }
            if object.has_mask() {
                floats(&mut out, object.mask.as_ref().unwrap());
            }
            if object.has_hidden() {
                out.extend(object.hidden.as_ref().unwrap().iter().map(|&flag| flag as u8));
            }
        }
        out
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.bytes_with(self.version(), &self.json())
    }

    /// Writes the file under the temp folder and returns where.
    pub fn write(&self, name: &str) -> PathBuf {
        write(name, &self.bytes())
    }
}

/// A file of `bytes` under the temp folder. Tests run side by side, so every
/// test gives its files names of its own.
pub fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join("sculpt_unity_import_tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// The brush block of a scene saved straight after the Unity app starts.
pub fn brush() -> Block {
    let ten = |value: &str| format!("[{}]", vec![value; 10].join(","));
    Block::new(&[
        ("brushStrength", float(0.1)),
        ("brushRadius", float(0.5)),
        ("worldSpaceBrushSize", "false".into()),
        ("brushScreenRadius", float(60.0)),
        ("currentBrush", "1".into()),
        ("isPositive", "true".into()),
        ("accumulate", "true".into()),
        ("accumulateStrength", float(1.0)),
        ("frontFacingOnly", "false".into()),
        ("buildUpOnHold", "false".into()),
        ("surfaceRelax", float(0.35)),
        ("poseRigidity", float(0.6)),
        ("poseSegments", "4".into()),
        ("clayHeightFactor", float(0.3)),
        ("clayTipRoundness", float(1.0)),
        ("clayEdgeSoftness", float(0.6)),
        ("clayPressureRadiusInfluence", float(0.3)),
        ("clayPressureSoftnessInfluence", float(0.5)),
        ("useAlpha", "false".into()),
        ("alphaType", "0".into()),
        ("alphaRotation", float(0.0)),
        ("alphaScale", float(1.0)),
        ("invertAlpha", "false".into()),
        ("creasePinch", float(0.6)),
        ("creaseDepthFactor", float(0.35)),
        ("maskHardness", float(0.5)),
        ("pressureFloor", float(0.12)),
        ("pressureCurve", float(1.6)),
        ("remeshResolution", "128".into()),
        ("quadRemeshTarget", "5000".into()),
        ("dynamicTopology", "false".into()),
        ("dynamicTopologyDetail", float(0.15)),
        ("useBurstJobs", "true".into()),
        ("showWireframeGizmo", "false".into()),
        ("maskPaintMode", "false".into()),
        ("perBrushStrength", ten(&float(0.1))),
        ("falloffCurves", "[]".into()),
        ("perBrushFocalShift", ten(&float(0.0))),
        ("perBrushPolarity", ten("true")),
        ("perBrushAccumulate", ten("false")),
        ("perBrushAccumulateStrength", ten(&float(1.0))),
        ("perBrushFrontFacingOnly", ten("false")),
    ])
}

/// The material block: a plain grey lit surface, matcap off.
pub fn material() -> Block {
    let mut block = Block::new(&[
        ("baseColor", rgba([0.65, 0.65, 0.68, 1.0])),
        ("metallic", float(0.0)),
        ("smoothness", float(0.4)),
        ("normalStrength", float(0.0)),
        ("normalNoiseScale", float(60.0)),
        ("flatShading", "false".into()),
        ("screenCavityEnabled", "true".into()),
        ("cavityRidge", float(1.0)),
        ("cavityValley", float(1.0)),
        ("matcapEnabled", "false".into()),
        ("matcapName", text("")),
        ("matcapIntensity", float(1.0)),
        ("matcapTintStrength", float(0.0)),
    ]);
    let finish = |block: &mut Block, name: &str, preset: &str, sliders: &[&str]| {
        block.set(&format!("{name}Enabled"), "false").set(&format!("{name}PresetId"), text(preset));
        for slider in sliders {
            block.set(&format!("{name}{slider}"), float(1.0));
        }
        block.set(&format!("{name}ReferenceSize"), float(0.0));
    };
    finish(&mut block, "lure", "black_blue_flake", &["FlakeSize", "FlakeAmount", "Sparkle", "Translucency", "Gloss"]);
    finish(
        &mut block,
        "metal",
        "rust",
        &["Exposure", "EdgeWear", "Wash", "Detail", "PatternSize", "Gloss", "PatternSeed"],
    );
    finish(&mut block, "clay", "terracotta", &["Gloss", "Wetness", "Subsurface", "Recess", "Detail", "Grain"]);
    block.set("woodGrainAxis", "0");
    finish(
        &mut block,
        "wood",
        "antique_fruitwood",
        &["Grain", "GrainScale", "EdgeWear", "Patina", "Detail", "Sheen", "Cracks", "GrainSeed"],
    );
    finish(
        &mut block,
        "glow",
        "lava",
        &["Intensity", "Coverage", "Rim", "PatternSize", "Speed", "Pulse", "Dim", "HueShift"],
    );
    block
        .set("lureGloss", float(0.8))
        .set("metalPatternSeed", float(0.0))
        .set("woodGrainSeed", float(0.0))
        .set("glowHueShift", float(0.0));
    block
}

/// The environment block: the Studio rig locked to the camera, a flat
/// background, no HDRI, the starting effects.
pub fn environment() -> Block {
    Block::new(&[
        ("lightingPreset", text("studio")),
        ("lightingFivePoint", "false".into()),
        ("lightingBrightness", float(1.0)),
        ("lightingRotation", float(0.0)),
        ("lightingFollowCamera", "true".into()),
        ("lightingWorldYaw", float(0.0)),
        ("lightingShadows", "true".into()),
        ("backgroundMode", "0".into()),
        ("backgroundColorA", rgba([0.10, 0.11, 0.14, 1.0])),
        ("backgroundColorB", rgba([0.35, 0.42, 0.55, 1.0])),
        ("gradientBias", float(1.0)),
        ("hdriEnabled", "false".into()),
        ("hdriPath", text("")),
        ("hdriRotation", float(0.0)),
        ("hdriExposure", float(1.0)),
        ("hdriAmbientIntensity", float(1.0)),
        ("hdriReflectionIntensity", float(1.0)),
        ("postAvailable", "true".into()),
        ("bloomEnabled", "true".into()),
        ("bloomIntensity", float(0.25)),
        ("bloomThreshold", float(1.0)),
        ("vignetteEnabled", "true".into()),
        ("vignetteIntensity", float(0.2)),
        ("vignetteSmoothness", float(0.2)),
        ("dofEnabled", "false".into()),
        ("dofFocusDistance", float(10.0)),
        ("dofAperture", float(5.6)),
        ("colorAdjustmentsEnabled", "false".into()),
        ("saturation", float(0.0)),
        ("contrast", float(0.0)),
    ])
}

/// The camera block: where the Unity app's camera starts, ten units back
/// along -Z, looking along +Z.
pub fn camera() -> Block {
    view(0.0, 0.0, 10.0, Vec3::ZERO, false)
}

pub fn view(yaw: f32, pitch: f32, distance: f32, pivot: Vec3, orthographic: bool) -> Block {
    Block::new(&[
        ("valid", "true".into()),
        ("yaw", float(yaw)),
        ("pitch", float(pitch)),
        ("distance", float(distance)),
        ("pivot", xyz(pivot)),
        ("orthographic", orthographic.to_string()),
    ])
}

// ---------------------------------------------------------------- shapes
//
// All wound so that the cross product of a triangle's first two edges points
// out of the solid, which is how the Unity app's meshes are stored (its
// faces read clockwise from outside, in its left-handed axes).

/// A box between two corners: 8 vertices, 12 triangles.
pub fn cuboid(lo: Vec3, hi: Vec3) -> (Vec<Vec3>, Vec<i32>) {
    let corner = |i: usize| {
        Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        )
    };
    let vertices = (0..8).map(corner).collect();
    // Each face as a quad running counter-clockwise seen from outside in a
    // right-handed reading of the numbers, which is the same thing.
    let quads = [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]];
    let triangles = quads.iter().flat_map(|q| [q[0], q[1], q[2], q[0], q[2], q[3]]).collect();
    (vertices, triangles)
}

/// A tetrahedron with every edge a different length, so it and its mirror
/// image cannot be turned into one another: the shape that shows a mirrored
/// import. Its corner is at `at` and its arms run 1 along X, 2 along Y and 3
/// along Z.
pub fn marker(at: Vec3) -> (Vec<Vec3>, Vec<i32>) {
    let vertices = vec![at, at + Vec3::X, at + Vec3::Y * 2.0, at + Vec3::Z * 3.0];
    (vertices, vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3])
}

/// Six times the volume enclosed, by the numbers alone: positive when the
/// cross product of each triangle's edges points outward.
pub fn six_volumes(vertices: &[Vec3], triangles: &[i32]) -> f64 {
    triangles
        .chunks_exact(3)
        .map(|t| {
            let p = |i: i32| vertices[i as usize].as_dvec3();
            p(t[0]).dot(p(t[1]).cross(p(t[2])))
        })
        .sum()
}
