//! What the app remembers between runs: interface size, tooltips, which
//! cards are open, fullscreen, the measurement unit, every brush's tuning and
//! the recent files. One small JSON file, `settings.json`, in the user's
//! config folder (`%APPDATA%\Sculpt` on Windows). The Unity app kept the same
//! things in PlayerPrefs.
//!
//! The file is forgiving in both directions. A setting it does not mention
//! keeps its built-in value, one this version does not know is ignored, and
//! one that makes no sense (a word where a number belongs) is passed over on
//! its own, so a file from an older or newer version, or one edited by hand,
//! never stops the app starting. Numbers are brought into the range their
//! sliders allow before they are used.
//!
//! Only the app as a person starts it remembers anything. A run given any
//! `--option` (`--screenshot`, the `--demo` family, `--size`...) is a scripted
//! one: it neither reads nor writes the user's file, since a visual check
//! must not depend on how the app was last left, and must not leave its own
//! scripted brush settings behind for the person who uses the app next.
//! `SCULPT_SETTINGS_DIR` points a run at a folder of its own instead (tests
//! and visual checks use it); a `--screenshot` run reads from there but still
//! writes nothing.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use sculpt_geom::LengthUnit;
use serde::{Deserialize, Serialize};

use crate::sculpt::dyntopo::{self, DynTopo};
use crate::sculpt::{Brush, FalloffCurve, PoseMode, PoseSettings, Settings};

const FILE_NAME: &str = "settings.json";

/// Where the settings live for this run, if anywhere.
pub struct Home {
    dir: Option<PathBuf>,
    /// Whether this run may write the settings file.
    writable: bool,
}

impl Home {
    /// Where this run keeps its settings: the folder `SCULPT_SETTINGS_DIR`
    /// names, else nowhere for a scripted run (one started with any
    /// `--option`), else the user's own.
    pub fn for_run(screenshot: bool) -> Home {
        if let Some(dir) = std::env::var_os("SCULPT_SETTINGS_DIR").filter(|d| !d.is_empty()) {
            return Home { dir: Some(PathBuf::from(dir)), writable: !screenshot };
        }
        if screenshot || scripted() {
            return Home::nowhere();
        }
        Home { dir: user_dir(), writable: true }
    }

    /// Where the app as a person starts it keeps its settings, whatever kind
    /// of run is asking: the folder `SCULPT_SETTINGS_DIR` names, else the
    /// user's own. Claude's server (`sculpt.exe --mcp`) looks here for the
    /// running app's bridge key (`bridge_guard.rs`); it is itself started
    /// with an `--option`, so [`Home::for_run`] would send it nowhere.
    pub fn of_the_app() -> Option<PathBuf> {
        std::env::var_os("SCULPT_SETTINGS_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(user_dir)
    }

    /// The folder itself, for what else is kept beside the settings: the
    /// recovery copies, the log (`recovery.rs`) and the Claude bridge's key
    /// (`bridge_guard.rs`).
    pub fn folder(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Whether this run may write in that folder.
    pub fn writable(&self) -> bool {
        self.writable
    }

    /// Remembers nothing and writes nothing.
    pub fn nowhere() -> Home {
        Home { dir: None, writable: false }
    }

    /// A folder of the caller's choosing.
    #[cfg_attr(not(test), allow(dead_code))] // tests keep their settings under the temp folder
    pub fn at(dir: &Path) -> Home {
        Home { dir: Some(dir.to_owned()), writable: true }
    }

    fn settings_file(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(FILE_NAME))
    }

    /// The folder the recent files' pictures are kept in.
    pub fn pictures(&self) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join("thumbnails"))
    }

    /// The settings as they were last saved. Anything that cannot be read
    /// comes back as the built-in value; this never fails.
    pub fn load(&self) -> Prefs {
        let Some(file) = self.settings_file() else { return Prefs::default() };
        match std::fs::read_to_string(&file) {
            Ok(text) => Prefs::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Prefs::default(),
            Err(e) => {
                log::warn!("could not read {}: {e}", file.display());
                Prefs::default()
            }
        }
    }

    /// Writes the settings, whole or not at all (see `io::replace_file`).
    /// Does nothing where this run may not write.
    pub fn save(&self, prefs: &Prefs) -> Result<(), String> {
        let Some(file) = self.settings_file().filter(|_| self.writable) else { return Ok(()) };
        let text = serde_json::to_string_pretty(prefs).map_err(|e| format!("Could not write the settings: {e}"))?;
        crate::io::replace_file(&file, |w| w.write_all(text.as_bytes()))
    }
}

/// A folder for a scripted demo to write its files in, by `name`: under the
/// folder `SCULPT_SETTINGS_DIR` names when the run was given one (in its
/// `scratch` folder), and under the system's temp folder when it was not.
/// Never the user's own settings folder, and never beside their files.
pub fn scratch(name: &str) -> PathBuf {
    let given = std::env::var_os("SCULPT_SETTINGS_DIR").filter(|dir| !dir.is_empty()).map(PathBuf::from);
    scratch_in(given.as_deref(), &std::env::temp_dir(), name)
}

/// [`scratch`], with the folder the run was given and the temp folder passed in.
fn scratch_in(given: Option<&Path>, temp: &Path, name: &str) -> PathBuf {
    match given {
        Some(settings) => settings.join("scratch").join(name),
        None => temp.join(name),
    }
}

/// Whether this run was started with any `--option`: a scripted one, which
/// keeps out of the user's own folder (see the top of this file).
pub fn scripted() -> bool {
    std::env::args().skip(1).any(|arg| is_option(&arg))
}

/// Whether an argument the program was started with is an option. Anything
/// else is a file to open, and that is all the app does with it. This is the
/// one place that says which is which: `main.rs` reads its arguments by it,
/// and the Claude bridge's `launch` counts on a full path never being one
/// (`mcp::launch_file`).
pub fn is_option(arg: &str) -> bool {
    arg.starts_with("--")
}

/// `%APPDATA%\Sculpt`; elsewhere the usual per-user config folder.
fn user_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("APPDATA").map(|d| d.join("Sculpt"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config"))).map(|d| d.join("sculpt"))
    }
}

/// One brush's own tuning: `sculpt::BrushSettings`, as the file keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushTuning {
    pub strength: f32,
    pub accumulate: bool,
    pub build_up: f32,
    /// Adds material; false takes it away.
    pub positive: bool,
    pub focal_shift: f32,
    pub front_facing: bool,
}

impl Default for BrushTuning {
    fn default() -> Self {
        let base = Settings::default().per_brush[0];
        Self::of(&base)
    }
}

impl BrushTuning {
    fn of(b: &crate::sculpt::BrushSettings) -> Self {
        Self {
            strength: b.strength,
            accumulate: b.accumulate,
            build_up: b.build_up,
            positive: b.positive,
            focal_shift: b.focal_shift,
            front_facing: b.front_facing,
        }
    }
}

/// The brush settings worth keeping: a copy of the public fields of
/// `sculpt::Settings`, by name, so that file does not need to know about
/// this one. A field added there is remembered once it is added here, in
/// `of` and in `apply`.
///
/// Left out on purpose: mask painting on or off (the app always starts
/// sculpting), and the mirror axes and radial symmetry (they belong to the
/// sculpt, and are kept in its `.sclp` file).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushPrefs {
    /// The brush in hand, by name.
    pub brush: String,
    /// Brush radius on screen, in pixels; one size shared by every brush.
    pub radius_px: f32,
    /// Whether the brush keeps its size on screen (on) or on the model.
    pub screen_space_size: bool,
    /// Brush radius on the model, for when it does not.
    pub radius_world: f32,
    /// Each brush's own tuning, by brush name.
    pub brushes: BTreeMap<String, BrushTuning>,
    /// The hand-drawn falloff curves, by brush name: the points of each, as
    /// (distance from the centre, weight). A brush that is not here keeps
    /// its own built-in falloff.
    pub falloff: BTreeMap<String, Vec<[f32; 2]>>,
    /// Move: grab only what is connected to the point under the pointer.
    pub move_connected: bool,
    /// Pose: what a drag does, by name (Bend, Resize or Stretch), how many
    /// joints a bend has, how far past the brush ring the first one sits and
    /// how sharply a joint bends.
    pub pose_mode: String,
    pub pose_joints: u32,
    pub pose_offset: f32,
    pub pose_rigidity: f32,
    pub mask_hardness: f32,
    pub clay_depth: f32,
    pub clay_softness: f32,
    pub clay_relax: f32,
    pub crease_pinch: f32,
    pub crease_depth: f32,
    pub flatten_offset: f32,
    pub layer_height: f32,
    pub pressure_floor: f32,
    pub pressure_curve: f32,
    pub lazy_mouse: bool,
    pub lazy_radius: f32,
    pub lazy_strength: f32,
    pub build_up_on_hold: bool,
}

impl Default for BrushPrefs {
    fn default() -> Self {
        Self::of(&Settings::default())
    }
}

/// A number from the file, brought into the range its slider allows; one
/// that is not a number at all gives way to `fallback`.
fn fit(value: f32, low: f32, high: f32, fallback: f32) -> f32 {
    if value.is_finite() { value.clamp(low, high) } else { fallback }
}

impl BrushPrefs {
    pub fn of(s: &Settings) -> Self {
        Self {
            brush: s.brush.name().to_owned(),
            radius_px: s.radius_px,
            screen_space_size: s.screen_space_size,
            radius_world: s.radius_world,
            brushes: Brush::ALL.iter().map(|b| (b.name().to_owned(), BrushTuning::of(&s.per_brush[b.index()]))).collect(),
            falloff: Brush::ALL
                .iter()
                .filter_map(|b| {
                    let curve = s.falloff[b.index()].as_ref()?;
                    Some((b.name().to_owned(), curve.points.iter().map(|p| p.to_array()).collect()))
                })
                .collect(),
            move_connected: s.move_connected,
            pose_mode: s.pose.mode.name().to_owned(),
            pose_joints: s.pose.joints,
            pose_offset: s.pose.offset,
            pose_rigidity: s.pose.rigidity,
            mask_hardness: s.mask_hardness,
            clay_depth: s.clay_depth,
            clay_softness: s.clay_softness,
            clay_relax: s.clay_relax,
            crease_pinch: s.crease_pinch,
            crease_depth: s.crease_depth,
            flatten_offset: s.flatten_offset,
            layer_height: s.layer_height,
            pressure_floor: s.pressure_floor,
            pressure_curve: s.pressure_curve,
            lazy_mouse: s.lazy_mouse,
            lazy_radius: s.lazy_radius,
            lazy_strength: s.lazy_strength,
            build_up_on_hold: s.build_up_on_hold,
        }
    }

    /// Puts the remembered tuning into the live settings. The ranges are the
    /// sliders' own, so nothing a hand-edited file says can reach the brush
    /// code out of range (several of these are divided by).
    pub fn apply(&self, s: &mut Settings) {
        let d = Settings::default();
        if let Some(&brush) = Brush::ALL.iter().find(|b| b.name() == self.brush) {
            s.brush = brush;
        }
        s.radius_px = fit(self.radius_px, 2.0, 2000.0, d.radius_px);
        s.screen_space_size = self.screen_space_size;
        // Any size that is one; the sculptor brings it into the model's range.
        s.radius_world = fit(self.radius_world, 1e-6, 1e6, d.radius_world);
        s.move_connected = self.move_connected;
        if let Some(mode) = PoseMode::ALL.into_iter().find(|mode| mode.name() == self.pose_mode) {
            s.pose.mode = mode;
        }
        s.pose.joints = self.pose_joints.clamp(1, PoseSettings::MAX_JOINTS);
        s.pose.offset = fit(self.pose_offset, 0.0, PoseSettings::MAX_OFFSET, d.pose.offset);
        s.pose.rigidity = fit(self.pose_rigidity, 0.0, 1.0, d.pose.rigidity);
        for &brush in &Brush::ALL {
            // A curve needs two points that are numbers; `normalize` then
            // orders them and pins the ends, as after any edit.
            let points: Vec<glam::Vec2> = self
                .falloff
                .get(brush.name())
                .map(|points| points.iter().map(|&p| glam::Vec2::from_array(p)).filter(|p| p.is_finite()).collect())
                .unwrap_or_default();
            s.falloff[brush.index()] = (points.len() >= 2).then(|| {
                let mut curve = FalloffCurve { points };
                curve.normalize();
                curve
            });
        }
        for &brush in &Brush::ALL {
            let Some(kept) = self.brushes.get(brush.name()) else { continue };
            let (slot, was) = (&mut s.per_brush[brush.index()], d.per_brush[brush.index()]);
            slot.strength = fit(kept.strength, 0.01, 1.0, was.strength);
            slot.accumulate = kept.accumulate;
            slot.build_up = fit(kept.build_up, 0.1, 3.0, was.build_up);
            slot.positive = kept.positive;
            slot.focal_shift = fit(kept.focal_shift, -1.0, 1.0, was.focal_shift);
            slot.front_facing = kept.front_facing;
        }
        s.mask_hardness = fit(self.mask_hardness, 0.0, 1.0, d.mask_hardness);
        s.clay_depth = fit(self.clay_depth, 0.1, 1.5, d.clay_depth);
        s.clay_softness = fit(self.clay_softness, 0.05, 1.0, d.clay_softness);
        s.clay_relax = fit(self.clay_relax, 0.0, 1.0, d.clay_relax);
        s.crease_pinch = fit(self.crease_pinch, 0.0, 1.0, d.crease_pinch);
        s.crease_depth = fit(self.crease_depth, 0.05, 1.0, d.crease_depth);
        s.flatten_offset = fit(self.flatten_offset, -0.5, 0.5, d.flatten_offset);
        s.layer_height = fit(self.layer_height, 0.02, 1.0, d.layer_height);
        s.pressure_floor = fit(self.pressure_floor, 0.0, 0.5, d.pressure_floor);
        s.pressure_curve = fit(self.pressure_curve, 0.5, 3.0, d.pressure_curve);
        s.lazy_mouse = self.lazy_mouse;
        s.lazy_radius = fit(self.lazy_radius, 1.0, 150.0, d.lazy_radius);
        s.lazy_strength = fit(self.lazy_strength, 0.05, 1.0, d.lazy_strength);
        s.build_up_on_hold = self.build_up_on_hold;
    }
}

/// Everything in the settings file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// 1 is 100 %.
    pub interface_size: f32,
    pub tooltips: bool,
    pub fullscreen: bool,
    /// "mm" or "in".
    pub unit: String,
    /// Cards that are not as they start out (open or closed), by title.
    pub cards: BTreeMap<String, bool>,
    pub brush: BrushPrefs,
    /// Voxels along the longest side for a voxel remesh.
    pub remesh_resolution: f32,
    /// Quads a quad remesh aims for.
    pub quad_target: f32,
    /// Triangle count of a new shape (Starting Detail).
    pub starting_detail: f32,
    /// Dynamic topology: whether brushes add detail as they go, and the
    /// longest edge they leave, as a fraction of the brush radius.
    pub dynamic_topology: bool,
    pub dynamic_topology_detail: f32,
    /// Sculpts recently saved or opened, newest first (see `recent.rs`).
    pub recent: Vec<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        let opts = crate::doc::Options::default();
        let dyntopo = DynTopo::default();
        Self {
            interface_size: opts.ui_scale,
            tooltips: opts.tooltips,
            fullscreen: false,
            unit: LengthUnit::default().suffix().to_owned(),
            cards: BTreeMap::new(),
            brush: BrushPrefs::default(),
            remesh_resolution: opts.remesh_resolution,
            quad_target: opts.quad_target,
            starting_detail: opts.new_shape_tris,
            dynamic_topology: dyntopo.enabled,
            dynamic_topology_detail: dyntopo.detail,
            recent: Vec::new(),
        }
    }
}

impl Prefs {
    /// Reads the text of a settings file written by any version of the app.
    pub fn parse(text: &str) -> Prefs {
        let defaults = Prefs::default();
        let Ok(file) = serde_json::from_str::<serde_json::Value>(text) else {
            log::warn!("the settings file is not readable; starting from the built-in settings");
            return defaults;
        };
        crate::io::read_over(&defaults, &file).unwrap_or(defaults)
    }

    pub fn length_unit(&self) -> LengthUnit {
        LengthUnit::ALL.into_iter().find(|u| u.suffix() == self.unit).unwrap_or_default()
    }

    /// The values the app reads from here, brought into range.
    pub fn interface_size(&self) -> f32 {
        fit(self.interface_size, 0.75, 3.0, 1.0)
    }

    pub fn remesh_resolution(&self) -> f32 {
        fit(self.remesh_resolution, 4.0, 1024.0, crate::doc::Options::default().remesh_resolution).round()
    }

    pub fn quad_target(&self) -> f32 {
        let range = crate::quadremesh::TARGET_RANGE;
        fit(self.quad_target, *range.start(), *range.end(), crate::doc::Options::default().quad_target)
    }

    pub fn starting_detail(&self) -> f32 {
        // The Starting Detail slider's own ends: 10^4 to 10^6.6 triangles.
        fit(self.starting_detail, 1.0e4, 10f32.powf(6.6), crate::doc::Options::default().new_shape_tris)
    }

    pub fn dynamic_topology_detail(&self) -> f32 {
        fit(self.dynamic_topology_detail, dyntopo::MIN_DETAIL, dyntopo::MAX_DETAIL, dyntopo::DEFAULT_DETAIL)
    }
}

/// Which cards of the panels are open, remembered between runs.
///
/// A card's open / closed state lives in egui's memory under an id made from
/// the panel it is in, which only the card itself can work out. So the app's
/// `theme::category` reports here as it draws: `before` puts back what was
/// remembered the first time a card is drawn, `after` notes how the card was
/// left. The notes ride in egui's own data store, so no global is needed.
pub mod cards {
    use std::collections::BTreeMap;

    use egui::{Id, Ui};

    #[derive(Clone, Default)]
    struct Notes {
        /// Cards that are not as they start out, by title.
        states: BTreeMap<String, bool>,
        /// Remembered states not yet handed to their cards.
        waiting: BTreeMap<String, bool>,
    }

    fn notes<R>(ctx: &egui::Context, f: impl FnOnce(&mut Notes) -> R) -> R {
        ctx.data_mut(|d| f(d.get_temp_mut_or_default::<Notes>(Id::new("remembered_cards"))))
    }

    /// Hands over what the settings file remembered, once, at start-up.
    pub fn restore(ctx: &egui::Context, remembered: &BTreeMap<String, bool>) {
        notes(ctx, |n| {
            n.states = remembered.clone();
            n.waiting = remembered.clone();
        });
    }

    /// The cards that are not as they start out, for the settings file.
    pub fn snapshot(ctx: &egui::Context) -> BTreeMap<String, bool> {
        notes(ctx, |n| n.states.clone())
    }

    /// Call just before drawing the card titled `title` in `ui`. Returns the
    /// id its state is kept under, for `after`.
    pub fn before(ui: &mut Ui, title: &str) -> Id {
        // The same id `sculpt_toolkit::theme::category` makes.
        let id = ui.make_persistent_id(("category", title));
        if let Some(open) = notes(ui.ctx(), |n| n.waiting.remove(title)) {
            ui.data_mut(|d| d.insert_persisted(id, open));
        }
        id
    }

    /// Call just after drawing it.
    pub fn after(ui: &mut Ui, id: Id, title: &str, default_open: bool) {
        let Some(open) = ui.data_mut(|d| d.get_persisted::<bool>(id)) else { return };
        notes(ui.ctx(), |n| {
            if open == default_open {
                n.states.remove(title);
            } else if n.states.get(title) != Some(&open) {
                n.states.insert(title.to_owned(), open);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings folder under the temp folder, never the user's own.
    fn settings_in(name: &str) -> (Home, PathBuf) {
        let dir = std::env::temp_dir().join("sculpt_prefs_tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        (Home::at(&dir), dir)
    }

    #[test]
    fn a_scripted_demo_writes_under_the_settings_folder_it_was_given_or_else_the_temp_folder() {
        let (given, temp) = (Path::new("D:/build/settings"), Path::new("C:/Temp"));
        assert_eq!(scratch_in(Some(given), temp, "sculpt_files_demo"), given.join("scratch").join("sculpt_files_demo"));
        assert_eq!(scratch_in(None, temp, "sculpt_files_demo"), temp.join("sculpt_files_demo"));
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let (home, dir) = settings_in("round_trip");
        let mut prefs = Prefs::default();
        prefs.interface_size = 1.5;
        prefs.tooltips = false;
        prefs.fullscreen = true;
        prefs.unit = "in".into();
        prefs.cards.insert("Stroke".into(), true);
        prefs.brush.brush = "Clay".into();
        prefs.brush.radius_px = 84.0;
        prefs.brush.brushes.get_mut("Crease").unwrap().strength = 0.75;
        prefs.dynamic_topology = true;
        prefs.dynamic_topology_detail = 0.08;
        prefs.recent = vec![dir.join("a.sclp"), dir.join("b.sclp")];
        home.save(&prefs).unwrap();
        assert_eq!(home.load(), prefs);
        assert_eq!(home.load().length_unit(), LengthUnit::Inches);
        // Nothing is left beside it from the write.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from(FILE_NAME)]);
    }

    #[test]
    fn a_missing_or_unreadable_file_gives_the_built_in_settings() {
        let (home, dir) = settings_in("missing");
        assert_eq!(home.load(), Prefs::default());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), "{ this is not json").unwrap();
        assert_eq!(home.load(), Prefs::default());
        std::fs::write(dir.join(FILE_NAME), "[1, 2, 3]").unwrap();
        assert_eq!(home.load(), Prefs::default());
        assert_eq!(Home::nowhere().load(), Prefs::default());
        // And a run that may not write leaves no file behind.
        let (_, dir) = settings_in("read_only");
        std::fs::create_dir_all(&dir).unwrap();
        let read_only = Home { dir: Some(dir.clone()), writable: false };
        read_only.save(&Prefs::default()).unwrap();
        assert!(!dir.join(FILE_NAME).exists());
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        // Written by some other version: a setting this one has never heard
        // of, most of today's settings missing, one brush with half its
        // fields, and a brush that no longer exists.
        let text = r#"{
            "interface_size": 1.25,
            "some_future_setting": { "anything": [1, 2, 3] },
            "brush": {
                "brush": "Inflate",
                "future_brush_field": 7,
                "brushes": {
                    "Crease": { "strength": 0.9 },
                    "Retired Brush": { "strength": 0.2, "accumulate": true }
                }
            },
            "recent": ["C:\\sculpts\\lure.sclp"]
        }"#;
        let prefs = Prefs::parse(text);
        let defaults = Prefs::default();
        assert_eq!(prefs.interface_size, 1.25);
        assert_eq!(prefs.tooltips, defaults.tooltips, "a missing setting keeps its built-in value");
        assert_eq!(prefs.unit, "mm");
        assert_eq!(prefs.brush.brush, "Inflate");
        assert_eq!(prefs.brush.radius_px, defaults.brush.radius_px);
        let crease = prefs.brush.brushes["Crease"];
        assert_eq!(crease.strength, 0.9);
        // The half-written brush keeps that brush's own defaults for the rest:
        // Crease starts out cutting, not adding.
        assert!(!crease.positive);
        assert_eq!(prefs.brush.brushes["Clay"], defaults.brush.brushes["Clay"]);
        assert_eq!(prefs.recent, vec![PathBuf::from("C:\\sculpts\\lure.sclp")]);

        // Applying it leaves every other brush as it was and ignores the one
        // this version does not have.
        let mut settings = Settings::default();
        prefs.brush.apply(&mut settings);
        assert_eq!(settings.brush, Brush::Inflate);
        assert_eq!(settings.per_brush[Brush::Crease.index()].strength, 0.9);
        assert_eq!(settings.per_brush[Brush::Clay.index()].strength, Settings::default().per_brush[Brush::Clay.index()].strength);
    }

    #[test]
    fn a_setting_of_the_wrong_kind_is_passed_over_alone() {
        let text = r#"{
            "interface_size": "huge",
            "tooltips": false,
            "brush": {
                "radius_px": 90,
                "clay_depth": true,
                "falloff": { "Clay": "sharp", "Crease": [[0, 1], [1, 0]] },
                "brushes": { "Retired Brush": { "strength": "a lot" }, "Layer": { "strength": 0.33 } }
            },
            "cards": { "Stroke": "yes", "Symmetry": false },
            "unit": "furlongs"
        }"#;
        let prefs = Prefs::parse(text);
        assert_eq!(prefs.interface_size, 1.0, "a word is not a size");
        assert!(!prefs.tooltips, "the settings around it are still read");
        // One bad entry among several costs only itself, however deep it is.
        assert_eq!(prefs.cards, BTreeMap::from([("Symmetry".to_owned(), false)]));
        assert_eq!(prefs.brush.radius_px, 90.0);
        assert_eq!(prefs.brush.clay_depth, Prefs::default().brush.clay_depth);
        assert_eq!(prefs.brush.falloff, BTreeMap::from([("Crease".to_owned(), vec![[0.0, 1.0], [1.0, 0.0]])]));
        assert_eq!(prefs.brush.brushes["Layer"].strength, 0.33);
        assert!(!prefs.brush.brushes.contains_key("Retired Brush"));
        assert_eq!(prefs.length_unit(), LengthUnit::Millimetres, "an unknown unit is millimetres");
    }

    #[test]
    fn numbers_out_of_range_are_brought_back_in() {
        let mut prefs = Prefs::default();
        prefs.interface_size = 40.0;
        prefs.remesh_resolution = -3.0;
        prefs.starting_detail = f32::INFINITY;
        prefs.brush.clay_softness = 0.0;
        prefs.brush.radius_px = f32::NAN;
        prefs.brush.brushes.get_mut("Clay").unwrap().strength = 9.0;
        // Dynamic topology starts off, at the Unity app's detail, and a
        // detail no slider could have set is brought back onto the slider.
        assert!(!prefs.dynamic_topology);
        assert_eq!(prefs.dynamic_topology_detail(), dyntopo::DEFAULT_DETAIL);
        prefs.dynamic_topology_detail = 0.0;
        assert_eq!(prefs.dynamic_topology_detail(), dyntopo::MIN_DETAIL);
        prefs.dynamic_topology_detail = f32::NAN;
        assert_eq!(prefs.dynamic_topology_detail(), dyntopo::DEFAULT_DETAIL);
        assert_eq!(prefs.interface_size(), 3.0);
        assert_eq!(prefs.remesh_resolution(), 4.0);
        assert_eq!(prefs.starting_detail(), Prefs::default().starting_detail);
        let mut settings = Settings::default();
        prefs.brush.apply(&mut settings);
        assert_eq!(settings.clay_softness, 0.05, "never zero: the brush divides by it");
        assert_eq!(settings.radius_px, Settings::default().radius_px);
        assert_eq!(settings.per_brush[Brush::Clay.index()].strength, 1.0);
    }

    /// A settings file written before there was a Pose brush: nine brushes,
    /// nothing about Pose. It loads, every brush it names keeps what it says,
    /// and Pose starts as it would on a first run.
    #[test]
    fn a_settings_file_from_before_the_pose_brush_still_loads() {
        let mut older = serde_json::to_value(Prefs::default()).unwrap();
        let brush = older["brush"].as_object_mut().unwrap();
        for key in ["pose_mode", "pose_joints", "pose_offset", "pose_rigidity"] {
            assert!(brush.remove(key).is_some(), "{key} is in the file");
        }
        assert!(brush["brushes"].as_object_mut().unwrap().remove("Pose").is_some());
        brush["brush"] = "Snake Hook".into();
        brush["brushes"]["Snake Hook"]["strength"] = 0.77.into();
        brush["falloff"]["Layer"] = serde_json::json!([[0.0, 1.0], [1.0, 0.0]]);
        assert_eq!(brush["brushes"].as_object().unwrap().len(), 9);

        let prefs = Prefs::parse(&older.to_string());
        let mut settings = Settings::default();
        prefs.brush.apply(&mut settings);
        assert_eq!(settings.brush, Brush::SnakeHook);
        assert_eq!(settings.per_brush[Brush::SnakeHook.index()].strength, 0.77);
        assert!(settings.falloff[Brush::Layer.index()].is_some());
        assert_eq!(settings.pose, PoseSettings::default());
        let pose = (settings.per_brush[Brush::Pose.index()], Settings::default().per_brush[Brush::Pose.index()]);
        assert_eq!(BrushTuning::of(&pose.0), BrushTuning::of(&pose.1));

        // And what this version writes names the Pose brush and its settings.
        settings.brush = Brush::Pose;
        settings.pose.joints = 3;
        let written = serde_json::to_value(BrushPrefs::of(&settings)).unwrap();
        assert_eq!(written["brush"], "Pose");
        assert_eq!(written["pose_joints"], 3);
        assert!(written["brushes"]["Pose"].is_object());
    }

    /// The number of joints has a lowest value as well as a highest (house
    /// rule 9), and a mode this version has not heard of changes nothing.
    #[test]
    fn pose_settings_out_of_range_are_brought_back_in() {
        let mut prefs = Prefs::default();
        let mut settings = Settings::default();
        for (kept, want) in [(0, 1), (1, 1), (8, 8), (9, 8), (u32::MAX, 8)] {
            prefs.brush.pose_joints = kept;
            prefs.brush.apply(&mut settings);
            assert_eq!(settings.pose.joints, want, "{kept} joints kept");
        }
        prefs.brush.pose_mode = "Wobble".into();
        (prefs.brush.pose_offset, prefs.brush.pose_rigidity) = (9.0, f32::NAN);
        settings.pose.mode = PoseMode::Resize;
        prefs.brush.apply(&mut settings);
        assert_eq!(settings.pose.mode, PoseMode::Resize);
        assert_eq!((settings.pose.offset, settings.pose.rigidity), (2.0, 0.6));
    }

    #[test]
    fn brush_settings_survive_a_capture_and_apply() {
        let mut settings = Settings::default();
        settings.brush = Brush::SnakeHook;
        settings.radius_px = 33.0;
        settings.lazy_mouse = true;
        settings.screen_space_size = false;
        settings.radius_world = 0.125;
        settings.move_connected = true;
        settings.pose = PoseSettings { mode: PoseMode::Stretch, joints: 5, offset: 0.75, rigidity: 0.2 };
        settings.per_brush[Brush::Layer.index()].front_facing = true;
        settings.per_brush[Brush::Flatten.index()].focal_shift = -0.4;
        let drawn = FalloffCurve { points: vec![glam::Vec2::new(0.0, 1.0), glam::Vec2::new(0.25, 0.75), glam::Vec2::new(1.0, 0.0)] };
        settings.falloff[Brush::Clay.index()] = Some(drawn.clone());
        let kept = BrushPrefs::of(&settings);
        // Through the file's own text, as it would be between two runs.
        let kept: BrushPrefs = serde_json::from_str(&serde_json::to_string(&kept).unwrap()).unwrap();
        let mut back = Settings::default();
        back.falloff[Brush::Crease.index()] = Some(drawn.clone());
        kept.apply(&mut back);
        assert_eq!(BrushPrefs::of(&back), kept);
        assert_eq!(back.brush, Brush::SnakeHook);
        assert!(back.per_brush[Brush::Layer.index()].front_facing);
        assert_eq!((back.screen_space_size, back.radius_world, back.move_connected), (false, 0.125, true));
        assert_eq!(back.pose, PoseSettings { mode: PoseMode::Stretch, joints: 5, offset: 0.75, rigidity: 0.2 });
        assert_eq!(back.falloff[Brush::Clay.index()], Some(drawn));
        assert!(back.falloff[Brush::Crease.index()].is_none(), "a brush with no curve kept goes back to its own falloff");

        // A curve that is not one (a single point, or not numbers) is not used.
        let mut odd = kept.clone();
        odd.falloff.insert("Clay".into(), vec![[0.5, 0.5]]);
        odd.falloff.insert("Inflate".into(), vec![[f32::NAN, 0.5], [0.2, f32::INFINITY]]);
        odd.falloff.insert("Layer".into(), vec![[2.0, 9.0], [-1.0, 0.5], [0.4, 0.4]]);
        odd.apply(&mut back);
        assert!(back.falloff[Brush::Clay.index()].is_none() && back.falloff[Brush::Inflate.index()].is_none());
        // And one with points out of range is tidied the way an edit is.
        let tidied = back.falloff[Brush::Layer.index()].clone().unwrap();
        assert_eq!((tidied.points[0].x, tidied.points.last().unwrap().x), (0.0, 1.0));
        assert!(tidied.points.iter().all(|p| (0.0..=1.0).contains(&p.x) && (0.0..=1.0).contains(&p.y)));
    }
}
