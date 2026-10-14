//! Custom materials: a finish function of the user's own, written in WGSL.
//!
//! In the `materials/shaders` folder beside the settings, each material is
//! two files with one name, its id (`hammered_copper.wgsl` and
//! `hammered_copper.json`):
//!
//! - the `.wgsl` file defines `fn finish_<id>(s: Surface, params:
//!   FinishParams) -> MaterialSample`, with any helpers it needs (best named
//!   starting with the id, so two materials' helpers cannot clash). It is
//!   compiled after the app's own shader, so `Surface`, `FinishParams`,
//!   `MaterialSample`, `material()`, `value_noise`, `fbm` and the rest are
//!   there to use (see the README).
//! - the `.json` sidecar names the material and lists its sliders: each
//!   slider's kind (colour, number, toggle), range, default, and where its
//!   value goes in `params` (`"at": [vec4 0-15, lane 0-3]`; a colour fills
//!   three lanes from there, in linear colour). A number with `"scale":
//!   "size"` is a fraction of the model's size, as the built-in finishes'
//!   pattern sizes are.
//!
//! **Nothing reaches the graphics card unchecked.** A shader the card
//! refuses closes the app (wgpu treats an uncaught error as fatal), so the
//! source is put through naga first, the compiler wgpu itself uses: parsed,
//! validated, its function's signature checked, and its use of screen-space
//! derivatives noted ([`check`]). Only a shader that passes goes to the card,
//! and even then inside an error scope (render/paint.rs). A material whose
//! shader fails shows as its plain swatch colour, and the status line says
//! which line of the file is wrong.
//!
//! **Hot reload.** The files are looked at about once a second
//! ([`Shaders::poll`]); a changed one is read and checked again, and the
//! palette and the library pick the new version up.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use super::Value;
use crate::look::{FinishParams, linear};

/// The folder's name, inside the materials folder.
pub const FOLDER: &str = "shaders";
/// No shader or sidecar comes near this; anything bigger is not read.
const MAX_BYTES: u64 = 1 << 20;
/// How often the files are looked at for changes.
const POLL: Duration = Duration::from_secs(1);

/// The app's own shader, which a custom material is compiled after.
pub fn prelude() -> String {
    concat!(include_str!("../render/shader.wgsl"), include_str!("../render/sculpt_pbr.wgsl")).to_owned()
}

/// What a slider is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    Colour,
    #[default]
    Number,
    Toggle,
}

/// One slider of a custom material, as its sidecar lists it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Param {
    pub name: String,
    pub kind: ParamKind,
    pub min: f32,
    pub max: f32,
    pub default: Value,
    /// Where the value goes: which of the sixteen vec4s, and which lane of it.
    pub at: [u8; 2],
    /// `"size"`: the value is a fraction of the model's size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl Default for Param {
    fn default() -> Self {
        Param {
            name: String::new(),
            kind: ParamKind::Number,
            min: 0.0,
            max: 1.0,
            default: Value::Number(0.0),
            at: [0, 0],
            scale: None,
            description: String::new(),
        }
    }
}

impl Param {
    /// `value` as this slider takes it: of its kind and in its range; the
    /// default for anything else.
    pub fn fitted(&self, value: Option<&Value>) -> Value {
        let (lo, hi) = (self.min.min(self.max), self.max.max(self.min));
        let number = |x: f32| if x.is_finite() { x.clamp(lo, hi) } else { lo };
        let default = match (&self.kind, &self.default) {
            (ParamKind::Toggle, Value::Toggle(b)) => Value::Toggle(*b),
            (ParamKind::Toggle, Value::Number(x)) => Value::Toggle(*x > 0.5),
            (ParamKind::Colour, Value::Colour(c)) => Value::Colour(c.map(|c| c.clamp(0.0, 1.0))),
            (ParamKind::Colour, _) => Value::Colour([0.5; 3]),
            (ParamKind::Number, Value::Number(x)) => Value::Number(number(*x)),
            (ParamKind::Number, _) | (ParamKind::Toggle, _) => match self.kind {
                ParamKind::Toggle => Value::Toggle(false),
                _ => Value::Number(lo),
            },
        };
        match (self.kind, value) {
            (ParamKind::Toggle, Some(Value::Toggle(b))) => Value::Toggle(*b),
            (ParamKind::Number, Some(Value::Number(x))) => Value::Number(number(*x)),
            (ParamKind::Colour, Some(Value::Colour(c))) => Value::Colour(c.map(|c| c.clamp(0.0, 1.0))),
            _ => default,
        }
    }
}

/// A custom material's sidecar.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Sidecar {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The finish function, if not `finish_<id>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// What else of the surface the shader reads: `"curvature"` (the mesh's
    /// recess and ridge, in `s.curv`) and `"back_depth"` (the back faces
    /// behind the pixel, in `s.back_depth`). Each costs a pass, so only a
    /// shader that asks gets them.
    pub needs: Vec<String>,
    /// sRGB 0 to 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swatch: Option<[f32; 3]>,
    pub params: Vec<Param>,
}

impl Sidecar {
    /// Reads a sidecar's text as forgivingly as a material file.
    pub fn parse(text: &str) -> Result<Sidecar, String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("the sidecar is not JSON ({e})"))?;
        crate::io::read_over(&Sidecar::default(), &value).ok_or_else(|| "the sidecar could not be read".to_owned())
    }

    pub fn needs(&self, what: &str) -> bool {
        self.needs.iter().any(|n| n.trim().eq_ignore_ascii_case(what))
    }

    /// The finish function's name for the material `id`.
    pub fn function_of(&self, id: &str) -> String {
        self.function.clone().filter(|f| !f.trim().is_empty()).unwrap_or_else(|| format!("finish_{id}"))
    }

    /// Every slider at its default.
    pub fn defaults(&self) -> BTreeMap<String, Value> {
        self.params.iter().map(|p| (p.name.clone(), p.fitted(None))).collect()
    }
}

/// `values` packed where `sidecar` says, with the patterns at `size`: each
/// number in its lane (times `size` if it is a size), each toggle as 1 or 0,
/// each colour in linear across three lanes. A slider whose place is off the
/// end of the sixteen vec4s is left out.
pub fn pack(sidecar: &Sidecar, values: &BTreeMap<String, Value>, size: f32) -> FinishParams {
    let mut out: FinishParams = bytemuck::Zeroable::zeroed();
    for param in &sidecar.params {
        let [slot, lane] = [param.at[0] as usize, param.at[1] as usize];
        let sized = param.scale.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("size"));
        let lanes: Vec<f32> = match param.fitted(values.get(&param.name)) {
            Value::Number(x) => vec![if sized { x * size } else { x }],
            Value::Toggle(b) => vec![if b { 1.0 } else { 0.0 }],
            Value::Colour(c) => linear(c).to_vec(),
        };
        if slot >= 16 || lane + lanes.len() > 4 {
            continue;
        }
        out.v[slot][lane..lane + lanes.len()].copy_from_slice(&lanes);
    }
    out
}

/// What is wrong with a shader, in words for the status line.
#[derive(Clone, PartialEq, Debug)]
pub struct Problem {
    /// The line of the user's file, from 1, when the compiler names one.
    pub line: Option<u32>,
    pub message: String,
}

impl Problem {
    fn new(line: Option<u32>, message: impl Into<String>) -> Problem {
        Problem { line, message: message.into() }
    }

    /// "line 12: expected ';'", or the message alone.
    pub fn words(&self) -> String {
        match self.line {
            Some(line) => format!("line {line}: {}", self.message),
            None => self.message.clone(),
        }
    }
}

/// What checking a shader found that does not stop it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Checked {
    pub warnings: Vec<String>,
}

/// The user's `id` as one that can be part of a WGSL function name: letters,
/// digits and underscores, starting with a letter. `None` if it is not one.
pub fn clean_id(id: &str) -> Option<String> {
    let id = id.trim();
    let ok = id.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && id.len() <= 48;
    ok.then(|| id.to_owned())
}

/// The names of the functions the app's own shader defines.
fn prelude_functions() -> &'static std::collections::HashSet<String> {
    static NAMES: std::sync::OnceLock<std::collections::HashSet<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let module = naga::front::wgsl::parse_str(&prelude()).expect("the app's own shader parses");
        module.functions.iter().filter_map(|(_, f)| f.name.clone()).collect()
    })
}

/// Checks the custom shader `source` for the material `id` the way the
/// graphics card would, without the card: compiled after the app's own
/// shader by naga, validated, and its finish function `function` looked
/// for with the signature the painted shader calls. Its use of screen-space
/// derivatives (`dpdx`, `dpdy`, `fwidth`, `textureSample` without a level)
/// is a warning: a painted material is shaded inside a branch chosen pixel
/// by pixel, where those give garbage. Line numbers are the user's file's.
pub fn check(source: &str, function: &str) -> Result<Checked, Problem> {
    let prelude = prelude();
    let before = prelude.lines().count() as u32;
    let whole = format!("{prelude}\n{source}");
    let line_of = |loc: Option<naga::SourceLocation>| {
        loc.map(|l| l.line_number).filter(|&l| l > before + 1).map(|l| l - before - 1)
    };
    let module = naga::front::wgsl::parse_str(&whole)
        .map_err(|e| Problem::new(line_of(e.location(&whole)), e.message().to_owned()))?;
    let mut validator = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default());
    validator.validate(&module).map_err(|e| {
        let detail = e.spans().next().map(|(_, label)| label.clone()).filter(|l| !l.is_empty());
        let message = match detail {
            Some(label) => format!("{} ({label})", e.as_inner()),
            None => e.as_inner().to_string(),
        };
        Problem::new(line_of(e.location(&whole)), message)
    })?;
    let found = module.functions.iter().find(|(_, f)| f.name.as_deref() == Some(function));
    let Some((handle, f)) = found else {
        return Err(Problem::new(None, format!("there is no function called {function}")));
    };
    let type_name = |ty: naga::Handle<naga::Type>| module.types[ty].name.clone().unwrap_or_default();
    let arguments: Vec<String> = f.arguments.iter().map(|a| type_name(a.ty)).collect();
    let result = f.result.as_ref().map(|r| type_name(r.ty)).unwrap_or_default();
    if arguments != ["Surface", "FinishParams"] || result != "MaterialSample" {
        return Err(Problem::new(
            None,
            format!("{function} must be fn {function}(s: Surface, params: FinishParams) -> MaterialSample"),
        ));
    }
    let mut checked = Checked::default();
    let own = prelude_functions();
    for used in reachable(&module, handle) {
        let f = &module.functions[used];
        let name = f.name.clone().unwrap_or_default();
        if own.contains(&name) && name != function {
            continue;
        }
        if let Some(what) = derivative_in(f) {
            checked.warnings.push(format!(
                "{name} uses {what}, which gives wrong results on painted materials: use s.footprint (one pixel's size on the model) instead"
            ));
        }
    }
    Ok(checked)
}

/// The functions `from` calls, itself included, however deep.
pub fn reachable(module: &naga::Module, from: naga::Handle<naga::Function>) -> Vec<naga::Handle<naga::Function>> {
    let mut seen = vec![from];
    let mut at = 0;
    while at < seen.len() {
        let mut calls = Vec::new();
        calls_in(&module.functions[seen[at]].body, &mut calls);
        for c in calls {
            if !seen.contains(&c) {
                seen.push(c);
            }
        }
        at += 1;
    }
    seen
}

fn calls_in(block: &naga::Block, out: &mut Vec<naga::Handle<naga::Function>>) {
    use naga::Statement as S;
    for statement in block.iter() {
        match statement {
            S::Call { function, .. } => out.push(*function),
            S::Block(b) => calls_in(b, out),
            S::If { accept, reject, .. } => {
                calls_in(accept, out);
                calls_in(reject, out);
            }
            S::Switch { cases, .. } => cases.iter().for_each(|c| calls_in(&c.body, out)),
            S::Loop { body, continuing, .. } => {
                calls_in(body, out);
                calls_in(continuing, out);
            }
            _ => {}
        }
    }
}

/// The first screen-space derivative `f` takes, in words, if it takes one:
/// `dpdx` and the like, or a texture sample that picks its own level.
pub fn derivative_in(f: &naga::Function) -> Option<&'static str> {
    f.expressions.iter().find_map(|(_, e)| match e {
        naga::Expression::Derivative { axis, .. } => Some(match axis {
            naga::DerivativeAxis::X => "dpdx",
            naga::DerivativeAxis::Y => "dpdy",
            naga::DerivativeAxis::Width => "fwidth",
        }),
        naga::Expression::ImageSample { level: naga::SampleLevel::Auto | naga::SampleLevel::Bias(_), .. } => {
            Some("textureSample")
        }
        _ => None,
    })
}

/// One custom shader of the folder, as last read.
#[derive(Clone, Debug)]
pub struct Shader {
    pub id: String,
    pub wgsl_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub source: String,
    pub sidecar_text: String,
    pub sidecar: Sidecar,
    /// What checking it found: fine (with any warnings), or why not.
    pub state: Result<Checked, Problem>,
    /// The files' times when they were read.
    stamp: (Option<SystemTime>, Option<SystemTime>),
}

impl Shader {
    /// A shader made from text, not files: a scene's copy, or a test's.
    pub fn from_text(id: &str, source: &str, sidecar_text: &str) -> Shader {
        let (sidecar, sidecar_problem) = match Sidecar::parse(sidecar_text) {
            Ok(s) => (s, None),
            Err(why) => (Sidecar::default(), Some(why)),
        };
        let state = match (clean_id(id), sidecar_problem) {
            (None, _) => Err(Problem::new(None, format!("\"{id}\" cannot be a material's id: use letters, digits and _"))),
            (_, Some(why)) => Err(Problem::new(None, why)),
            (Some(id), None) => check(source, &sidecar.function_of(&id)),
        };
        Shader {
            id: id.to_owned(),
            wgsl_path: PathBuf::new(),
            sidecar_path: PathBuf::new(),
            source: source.to_owned(),
            sidecar_text: sidecar_text.to_owned(),
            sidecar,
            state,
            stamp: (None, None),
        }
    }

    /// The material's name: the sidecar's, or the id.
    pub fn name(&self) -> String {
        let name = self.sidecar.name.trim();
        if name.is_empty() { self.id.replace('_', " ") } else { name.to_owned() }
    }

    pub fn ok(&self) -> bool {
        self.state.is_ok()
    }

    /// The finish function's name.
    pub fn function(&self) -> String {
        self.sidecar.function_of(&self.id)
    }

    /// What the status line says when it does not compile.
    pub fn failure(&self) -> Option<String> {
        let problem = self.state.as_ref().err()?;
        Some(format!(
            "Custom material \"{}\" did not compile ({}). It shows as plain colour until the file is fixed.",
            self.name(),
            problem.words()
        ))
    }

    /// A number that changes whenever the source or the sidecar does: part of
    /// the painted pipeline's key and the thumbnail's.
    pub fn hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.id.hash(&mut h);
        self.source.hash(&mut h);
        self.sidecar_text.hash(&mut h);
        h.finish()
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn read_small(path: &Path) -> Result<String, String> {
    let shown = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let size = std::fs::metadata(path).map_err(|e| format!("could not read {shown}: {e}"))?.len();
    if size > MAX_BYTES {
        return Err(format!("{shown} is far too big to be a material's file"));
    }
    std::fs::read_to_string(path).map_err(|e| format!("could not read {shown}: {e}"))
}

/// Reads the shader `id` from its two files.
fn read_shader(id: &str, wgsl: &Path, sidecar: &Path) -> Shader {
    let stamp = (mtime(wgsl), mtime(sidecar));
    let source = read_small(wgsl);
    let sidecar_text = read_small(sidecar).unwrap_or_else(|_| "{}".to_owned());
    let mut shader = match &source {
        Ok(source) => Shader::from_text(id, source, &sidecar_text),
        Err(_) => Shader::from_text(id, "", &sidecar_text),
    };
    if let Err(why) = source {
        shader.state = Err(Problem::new(None, why));
    }
    shader.wgsl_path = wgsl.to_owned();
    shader.sidecar_path = sidecar.to_owned();
    shader.stamp = stamp;
    shader
}

/// Every custom shader there is: the folder's, and the copies the scene's
/// palette carries for shaders this computer does not have.
#[derive(Default)]
pub struct Shaders {
    dir: Option<PathBuf>,
    list: Vec<Shader>,
    /// Shaders known only from a scene file, by id.
    carried: Vec<Shader>,
    next_poll: Option<Instant>,
}

impl Shaders {
    /// The shaders of the materials folder `materials` (none if `None`).
    pub fn new(materials: Option<&Path>) -> Shaders {
        Shaders { dir: materials.map(|m| m.join(FOLDER)), ..Shaders::default() }
    }

    pub fn folder(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    pub fn list(&self) -> &[Shader] {
        &self.list
    }

    /// The shader `id`: the folder's if it has one, else a scene's copy.
    pub fn get(&self, id: &str) -> Option<&Shader> {
        self.list.iter().chain(&self.carried).find(|s| s.id == id)
    }

    /// The shader `id` if it is there and compiles.
    pub fn usable(&self, id: &str) -> Option<&Shader> {
        self.get(id).filter(|s| s.ok())
    }

    /// Takes in a scene's copy of a shader, for when the folder has none of
    /// that id. A copy that is already known, as it is, costs nothing.
    pub fn carry(&mut self, id: &str, source: &super::Source) {
        if self.list.iter().any(|s| s.id == id) {
            return;
        }
        let same = |s: &Shader| s.id == id && s.source == source.wgsl && s.sidecar_text == source.sidecar;
        if self.carried.iter().any(same) {
            return;
        }
        self.carried.retain(|s| s.id != id);
        self.carried.push(Shader::from_text(id, &source.wgsl, &source.sidecar));
    }

    /// Reads the folder afresh: every `<id>.wgsl` with its `<id>.json`.
    /// Gives back what there is to say (files that were skipped).
    pub fn scan(&mut self) -> Vec<String> {
        self.list.clear();
        let Some(dir) = self.dir.clone() else { return Vec::new() };
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        paths.retain(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wgsl")));
        paths.sort();
        let mut notes = Vec::new();
        for wgsl in paths {
            let stem = wgsl.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(id) = clean_id(&stem) else {
                notes.push(format!(
                    "Skipped the custom material file {stem}.wgsl: its name can only have letters, digits and _ in it."
                ));
                continue;
            };
            self.list.push(read_shader(&id, &wgsl, &wgsl.with_extension("json")));
        }
        notes
    }

    /// About once a second: reads again any shader whose files changed, and
    /// the folder if shaders came or went. Gives back the ids that changed.
    pub fn poll(&mut self, now: Instant) -> Vec<String> {
        if self.next_poll.is_some_and(|at| now < at) {
            return Vec::new();
        }
        self.next_poll = Some(now + POLL);
        self.reread_changed()
    }

    /// Reads again every shader whose files' times differ from when they
    /// were read (and the folder, if its `.wgsl` files are not the ones
    /// known). Gives back the ids that changed.
    pub fn reread_changed(&mut self) -> Vec<String> {
        let mut changed = Vec::new();
        for shader in &mut self.list {
            let now = (mtime(&shader.wgsl_path), mtime(&shader.sidecar_path));
            if now != shader.stamp {
                *shader = read_shader(&shader.id, &shader.wgsl_path.clone(), &shader.sidecar_path.clone());
                changed.push(shader.id.clone());
            }
        }
        let Some(dir) = &self.dir else { return changed };
        let on_disk: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wgsl")))
                    .filter_map(|p| p.file_stem().and_then(|s| clean_id(&s.to_string_lossy())))
                    .collect()
            })
            .unwrap_or_default();
        let known: Vec<String> = self.list.iter().map(|s| s.id.clone()).collect();
        let same = on_disk.len() == known.len() && on_disk.iter().all(|id| known.contains(id));
        if !same {
            let before = known;
            self.scan();
            for s in &self.list {
                if !before.contains(&s.id) && !changed.contains(&s.id) {
                    changed.push(s.id.clone());
                }
            }
            for id in before {
                if !self.list.iter().any(|s| s.id == id) && !changed.contains(&id) {
                    changed.push(id);
                }
            }
        }
        changed
    }
}

/// The example custom material that ships with the app (see the README).
pub const EXAMPLE_ID: &str = "hammered_copper";
pub const EXAMPLE_WGSL: &str = include_str!("../../assets/materials/example/hammered_copper.wgsl");
pub const EXAMPLE_SIDECAR: &str = include_str!("../../assets/materials/example/hammered_copper.json");

/// Makes the shaders folder `dir` with the example in it, the first time
/// there is no such folder: something to start from, and to look at. A
/// folder that is there is the user's and is left exactly as it is.
pub fn install_example(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let write = |name: String, text: &str| std::fs::write(dir.join(name), text).map_err(|e| e.to_string());
    write(format!("{EXAMPLE_ID}.wgsl"), EXAMPLE_WGSL)?;
    write(format!("{EXAMPLE_ID}.json"), EXAMPLE_SIDECAR)
}
