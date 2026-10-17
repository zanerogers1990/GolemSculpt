//! What the app remembers between runs: interface size, tooltips, which
//! cards are open, which float and where, which panels are folded away,
//! fullscreen, the measurement unit, every brush's tuning and the recent
//! files. One small JSON file, `settings.json`, in the user's config folder
//! (`%APPDATA%\Sculpt` on Windows). The Unity app kept the same things in
//! PlayerPrefs.
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

use crate::sculpt::alpha::AlphaSettings;
use crate::sculpt::dyntopo::{self, DynTopo};
use crate::sculpt::{Brush, BrushSettings, FalloffCurve, FlattenMode, PoseMode, PoseSettings, Settings, SmoothMode};

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
        let Some(file) = self.settings_file() else { return Prefs::fresh() };
        match std::fs::read_to_string(&file) {
            Ok(text) => Prefs::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Prefs::fresh(),
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
    pub stay_in_group: bool,
    /// The distance between dabs, and how far each is scattered off the
    /// stroke's path, as fractions of the brush's radius.
    pub spacing: f32,
    pub scatter: f32,
    /// How much smaller than the brush a dab may come out, at random.
    pub size_jitter: f32,
    /// How much of its size, and of its strength, the brush loses at the
    /// pen's lightest touch.
    pub pressure_size: f32,
    pub pressure_strength: f32,
    /// For a brush that smooths: what it evens out, and how much of the
    /// shrinkage it gives back.
    pub smooth_mode: SmoothMode,
    pub keep_volume: f32,
    /// For a brush that flattens: which side of its plane it works on, and
    /// whether the plane is fixed where the stroke begins.
    pub flatten_mode: FlattenMode,
    pub lock_plane: bool,
}

impl Default for BrushTuning {
    fn default() -> Self {
        // A brush with nothing of its own about it (Move, the first, has
        // a Strength of its own).
        let base = Settings::default().per_brush[Brush::Smooth.index()];
        Self::of(&base)
    }
}

impl BrushTuning {
    pub(crate) fn of(b: &crate::sculpt::BrushSettings) -> Self {
        Self {
            strength: b.strength,
            accumulate: b.accumulate,
            build_up: b.build_up,
            positive: b.positive,
            focal_shift: b.focal_shift,
            front_facing: b.front_facing,
            stay_in_group: b.stay_in_group,
            spacing: b.spacing,
            scatter: b.scatter,
            size_jitter: b.size_jitter,
            pressure_size: b.pressure_size,
            pressure_strength: b.pressure_strength,
            smooth_mode: b.smooth_mode,
            keep_volume: b.keep_volume,
            flatten_mode: b.flatten_mode,
            lock_plane: b.lock_plane,
        }
    }
}

/// One brush's alpha: `sculpt::alpha::AlphaSettings`, as the file keeps it.
/// The picture goes by its name in the alpha list; a brush with none is not
/// in the file at all.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlphaKept {
    pub name: String,
    pub rotation: f32,
    pub scale: f32,
    pub invert: bool,
    pub tile: bool,
    pub follow: bool,
    pub drag: bool,
    pub random_rotation: f32,
}

impl Default for AlphaKept {
    fn default() -> Self {
        Self::of(&AlphaSettings::default())
    }
}

impl AlphaKept {
    pub(crate) fn of(a: &AlphaSettings) -> Self {
        Self {
            name: a.image.map(|image| image.name.clone()).unwrap_or_default(),
            rotation: a.rotation,
            scale: a.scale,
            invert: a.invert,
            tile: a.tile,
            follow: a.follow,
            drag: a.drag,
            random_rotation: a.random_rotation,
        }
    }

    /// The live setting. A picture that is no longer in the alpha folder
    /// leaves the brush with its plain round tip.
    pub(crate) fn restored(&self) -> AlphaSettings {
        let d = AlphaSettings::default();
        let (scale, random) = (AlphaSettings::SCALE, AlphaSettings::RANDOM_ROTATION);
        AlphaSettings {
            image: crate::sculpt::alpha::find(&self.name),
            rotation: fit(self.rotation, 0.0, 360.0, d.rotation),
            scale: fit(self.scale, *scale.start(), *scale.end(), d.scale),
            invert: self.invert,
            tile: self.tile,
            follow: self.follow,
            drag: self.drag,
            random_rotation: fit(self.random_rotation, *random.start(), *random.end(), d.random_rotation),
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
    /// The alphas, by brush name. A brush that is not here has none.
    pub alpha: BTreeMap<String, AlphaKept>,
    /// Move: grab only what is connected to the point under the pointer.
    pub move_connected: bool,
    /// Move's Strength: how much of the pen's travel the surface follows.
    /// Kept here, and read from here, because the number under Move in
    /// `brushes` means nothing in a file from before Move had a Strength:
    /// the slider was there and did nothing, so it holds whatever it was
    /// left at. Such a file has no `move_strength`, and Move starts at its
    /// own.
    pub move_strength: f32,
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
    /// The Light-Touch Floor of versions before each brush had a
    /// Pressure -> Strength of its own: the share of its strength every
    /// brush kept at the pen's lightest touch. This version never writes
    /// it. A file that has it is one of theirs, and has nothing to say of
    /// its own about what pressure does to each brush, so the floor is
    /// carried over into every brush (see `apply`).
    pub pressure_floor: Option<f32>,
    pub pressure_curve: f32,
    pub lazy_mouse: bool,
    pub lazy_radius: f32,
    pub lazy_strength: f32,
    pub build_up_on_hold: bool,
    /// The Paint brush's colour, swatches, mode and channels. A file from
    /// before there was one gives the brush as it starts.
    pub paint: crate::sculpt::paint::PaintKept,
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
            alpha: Brush::ALL
                .iter()
                .filter(|b| s.alpha[b.index()].image.is_some())
                .map(|b| (b.name().to_owned(), AlphaKept::of(&s.alpha[b.index()])))
                .collect(),
            move_connected: s.move_connected,
            move_strength: s.per_brush[Brush::Move.index()].strength,
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
            pressure_floor: None,
            pressure_curve: s.pressure_curve,
            lazy_mouse: s.lazy_mouse,
            lazy_radius: s.lazy_radius,
            lazy_strength: s.lazy_strength,
            build_up_on_hold: s.build_up_on_hold,
            paint: crate::sculpt::paint::PaintKept::of(&s.paint),
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
            s.alpha[brush.index()] = self.alpha.get(brush.name()).map(AlphaKept::restored).unwrap_or_default();
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
            slot.stay_in_group = kept.stay_in_group;
            let within = |value: f32, range: std::ops::RangeInclusive<f32>, was: f32| {
                fit(value, *range.start(), *range.end(), was)
            };
            slot.spacing = within(kept.spacing, BrushSettings::SPACING, was.spacing);
            slot.scatter = within(kept.scatter, BrushSettings::SCATTER, was.scatter);
            slot.size_jitter = within(kept.size_jitter, BrushSettings::SIZE_JITTER, was.size_jitter);
            slot.pressure_size = within(kept.pressure_size, BrushSettings::PRESSURE, was.pressure_size);
            slot.pressure_strength = within(kept.pressure_strength, BrushSettings::PRESSURE, was.pressure_strength);
            (slot.smooth_mode, slot.flatten_mode, slot.lock_plane) = (kept.smooth_mode, kept.flatten_mode, kept.lock_plane);
            slot.keep_volume = fit(kept.keep_volume, 0.0, 1.0, was.keep_volume);
        }
        // A file from before brushes had their own: its one floor, in the
        // range its slider had, becomes every brush's Pressure -> Strength.
        if let Some(floor) = self.pressure_floor.filter(|floor| floor.is_finite()) {
            for slot in &mut s.per_brush {
                slot.pressure_strength = 1.0 - floor.clamp(0.0, 0.5);
            }
        }
        let was = d.per_brush[Brush::Move.index()].strength;
        s.per_brush[Brush::Move.index()].strength = fit(self.move_strength, 0.01, 1.0, was);
        s.mask_hardness = fit(self.mask_hardness, 0.0, 1.0, d.mask_hardness);
        s.clay_depth = fit(self.clay_depth, 0.1, 1.5, d.clay_depth);
        s.clay_softness = fit(self.clay_softness, 0.05, 1.0, d.clay_softness);
        s.clay_relax = fit(self.clay_relax, 0.0, 1.0, d.clay_relax);
        s.crease_pinch = fit(self.crease_pinch, 0.0, 1.0, d.crease_pinch);
        s.crease_depth = fit(self.crease_depth, 0.05, 1.0, d.crease_depth);
        s.flatten_offset = fit(self.flatten_offset, -0.5, 0.5, d.flatten_offset);
        s.layer_height = fit(self.layer_height, 0.02, 1.0, d.layer_height);
        s.pressure_curve = fit(self.pressure_curve, 0.5, 3.0, d.pressure_curve);
        s.lazy_mouse = self.lazy_mouse;
        s.lazy_radius = fit(self.lazy_radius, 1.0, 150.0, d.lazy_radius);
        s.lazy_strength = fit(self.lazy_strength, 0.05, 1.0, d.lazy_strength);
        s.build_up_on_hold = self.build_up_on_hold;
        self.paint.apply(&mut s.paint);
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
    /// The Sculpt panel (left) and the Scene panel (right) folded away to a
    /// tab at the edge of the screen.
    pub left_panel_folded: bool,
    pub right_panel_folded: bool,
    /// The tab each panel was left on, by name. A name that is no tab's
    /// gives that panel's first.
    pub sculpt_tab: String,
    pub scene_tab: String,
    /// The interface's colours, by the theme's name (see
    /// `sculpt_toolkit::palette`). A name that is no theme's gives the
    /// built-in one.
    pub theme: String,
    /// Cards pulled out of their panel to float over the viewport, by name:
    /// the left and top of each one's window, in the interface's points. A
    /// card that is not here is in its panel (see `sculpt_toolkit::floating`).
    pub floating_cards: BTreeMap<String, [f32; 2]>,
    /// The fourteen base brushes' settings, and what every brush shares.
    pub brush: BrushPrefs,
    /// The brush library: which brush is in hand, which one Shift uses, the
    /// pins and the shipped brushes deleted. The user's own brushes are
    /// files of their own (see `brush_files.rs`).
    pub library: crate::brush_files::Kept,
    /// Voxels along the longest side for a voxel remesh.
    pub remesh_resolution: f32,
    /// Quads a quad remesh aims for.
    pub quad_target: f32,
    /// Triangles Decimate brings an object down to.
    pub decimate_target: f32,
    /// The Subdivision card's Flat tick: Subdivide cuts faces and moves nothing.
    pub subdivide_flat: bool,
    /// Triangle count of a new shape (Starting Detail).
    pub starting_detail: f32,
    /// Dynamic topology: whether brushes add detail as they go, and the
    /// longest edge they leave, as a fraction of the brush radius.
    pub dynamic_topology: bool,
    pub dynamic_topology_detail: f32,
    /// Sculpts recently saved or opened, newest first (see `recent.rs`).
    pub recent: Vec<PathBuf>,
    /// How hard the renderer works: shadow map size and the like (see
    /// `render/quality.rs`).
    pub render_quality: crate::render::quality::Quality,
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
            left_panel_folded: opts.folded[0],
            right_panel_folded: opts.folded[1],
            sculpt_tab: opts.tabs.sculpt.name().to_owned(),
            scene_tab: opts.tabs.scene.name().to_owned(),
            theme: crate::ui::theme::THEMES[opts.theme].name.to_owned(),
            floating_cards: BTreeMap::new(),
            brush: BrushPrefs::default(),
            library: crate::brush_files::Kept::default(),
            remesh_resolution: opts.remesh_resolution,
            quad_target: opts.quad_target,
            decimate_target: opts.decimate_target,
            subdivide_flat: opts.subdivide_flat,
            starting_detail: opts.new_shape_tris,
            dynamic_topology: dyntopo.enabled,
            dynamic_topology_detail: dyntopo.detail,
            recent: Vec::new(),
            render_quality: opts.quality,
        }
    }
}

impl Prefs {
    /// The settings of a first run, with no settings file yet: the built-in
    /// ones, with the brush a fresh app starts with in hand. (Not
    /// `Default`: that also fills in what an older file lacks, and a file
    /// from before there was a library keeps the brush it was left on.)
    pub fn fresh() -> Prefs {
        let mut prefs = Prefs::default();
        prefs.library.selected = crate::sculpt::library::shipped::IN_HAND.to_owned();
        prefs
    }

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

    /// The theme's place in `ui::theme::THEMES`.
    pub fn theme(&self) -> usize {
        sculpt_toolkit::palette::named(&self.theme).unwrap_or(0)
    }

    /// The tab each panel was left on.
    pub fn tabs(&self) -> crate::ui::tabs::Showing {
        crate::ui::tabs::Showing::named(&self.sculpt_tab, &self.scene_tab)
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

    pub fn decimate_target(&self) -> f32 {
        let range = crate::decimate::TARGET_RANGE;
        fit(self.decimate_target, *range.start(), *range.end(), crate::doc::Options::default().decimate_target).round()
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

    /// Opens the card titled `title` the next time it is drawn: what a
    /// button that sends the sculptor to another card does.
    pub fn open(ctx: &egui::Context, title: &str) {
        notes(ctx, |n| {
            n.waiting.insert(title.to_owned(), true);
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
        prefs.right_panel_folded = true;
        prefs.floating_cards.insert("Brushes".into(), [640.0, 120.0]);
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
        // No file at all is a first run: Clay Strips is in hand.
        let (home, dir) = settings_in("missing");
        assert_eq!(home.load(), Prefs::fresh());
        assert_eq!(Prefs::fresh().library.selected, "Clay Strips");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), "{ this is not json").unwrap();
        assert_eq!(home.load(), Prefs::default());
        std::fs::write(dir.join(FILE_NAME), "[1, 2, 3]").unwrap();
        assert_eq!(home.load(), Prefs::default());
        assert_eq!(Home::nowhere().load(), Prefs::fresh());
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

    /// Which panels are folded away: a file from before there was such a
    /// thing starts with both showing, one side is kept without the other,
    /// and a value that is not yes or no costs only itself.
    #[test]
    fn folded_panels_are_remembered_and_old_files_start_with_both_showing() {
        let mut older = serde_json::to_value(Prefs::default()).unwrap();
        let fields = older.as_object_mut().unwrap();
        assert!(fields.remove("left_panel_folded").is_some() && fields.remove("right_panel_folded").is_some());
        fields.insert("tooltips".into(), false.into());
        let prefs = Prefs::parse(&older.to_string());
        assert!(!prefs.left_panel_folded && !prefs.right_panel_folded && !prefs.tooltips);

        let (home, _) = settings_in("folded");
        let folded = Prefs { left_panel_folded: true, ..Prefs::default() };
        home.save(&folded).unwrap();
        assert_eq!(home.load(), folded);
        assert!(home.load().left_panel_folded && !home.load().right_panel_folded);

        let odd = r#"{ "left_panel_folded": "yes", "right_panel_folded": true, "interface_size": 1.5 }"#;
        let prefs = Prefs::parse(odd);
        assert!(!prefs.left_panel_folded, "a word is not yes or no");
        assert!(prefs.right_panel_folded && prefs.interface_size == 1.5, "the settings beside it are still read");
        let odd = r#"{ "left_panel_folded": 1, "right_panel_folded": null, "tooltips": false }"#;
        let prefs = Prefs::parse(odd);
        assert!(!prefs.left_panel_folded && !prefs.right_panel_folded && !prefs.tooltips);
    }

    /// The theme and the tab each panel is on: a file from before there
    /// were such things starts in the built-in theme on each panel's first
    /// tab, both are kept by name, and a name that is nobody's (or is not a
    /// name at all) costs only itself.
    #[test]
    fn the_theme_and_the_tabs_are_remembered_by_name_and_old_files_start_as_before() {
        use crate::ui::tabs::{Scene, Sculpt, Showing};
        let mut older = serde_json::to_value(Prefs::default()).unwrap();
        let fields = older.as_object_mut().unwrap();
        assert!(["theme", "sculpt_tab", "scene_tab"].iter().all(|field| fields.remove(*field).is_some()));
        fields.insert("tooltips".into(), false.into());
        let prefs = Prefs::parse(&older.to_string());
        assert_eq!((prefs.theme(), prefs.tabs(), prefs.tooltips), (0, Showing::default(), false));
        assert_eq!(Prefs::default().theme, "Midnight", "the built-in theme, by the name on its tile");

        let (home, _) = settings_in("theme");
        let chosen = Prefs { theme: "Clay".into(), sculpt_tab: "Mesh".into(), scene_tab: "Look".into(), ..Prefs::default() };
        home.save(&chosen).unwrap();
        let kept = home.load();
        assert_eq!(kept, chosen);
        assert_eq!(crate::ui::theme::THEMES[kept.theme()].name, "Clay");
        assert_eq!(kept.tabs(), Showing { sculpt: Sculpt::Mesh, scene: Scene::Look });

        let odd = r#"{ "theme": "Neon", "sculpt_tab": 3, "scene_tab": "settings", "interface_size": 0.75 }"#;
        let prefs = Prefs::parse(odd);
        assert_eq!(prefs.theme(), 0, "a theme this version does not have gives the built-in one");
        assert_eq!(prefs.tabs(), Showing { sculpt: Sculpt::Brush, scene: Scene::Settings });
        assert_eq!(prefs.interface_size(), 0.75, "the settings beside them are still read");
    }

    /// Which cards float, and where. A file from before cards could float
    /// has none; each entry that is not two numbers costs only itself; and a
    /// place that is numbers but nowhere near a screen is not handed to the
    /// interface, so that card comes back in its panel.
    #[test]
    fn floating_cards_are_remembered_and_nonsense_comes_back_docked() {
        let mut older = serde_json::to_value(Prefs::default()).unwrap();
        assert!(older.as_object_mut().unwrap().remove("floating_cards").is_some());
        older["interface_size"] = 1.25.into();
        let prefs = Prefs::parse(&older.to_string());
        assert!(prefs.floating_cards.is_empty() && prefs.interface_size == 1.25);

        let (home, _) = settings_in("floating");
        let mut floating = Prefs::default();
        floating.floating_cards.insert("Brushes".into(), [320.5, 96.0]);
        floating.floating_cards.insert("Measure & Size".into(), [-12.0, 2000.0]);
        home.save(&floating).unwrap();
        assert_eq!(home.load(), floating);

        let odd = r#"{
            "floating_cards": {
                "Brushes": [300, 200],
                "Masking": "over there",
                "Remesh": [1, 2, 3],
                "File": [10],
                "Objects": { "x": 1, "y": 2 },
                "Material": [null, 4],
                "Lighting": [1e30, 5],
                "Stroke": [true, 6]
            },
            "tooltips": false
        }"#;
        let prefs = Prefs::parse(odd);
        assert!(!prefs.tooltips, "the settings beside it are still read");
        let read: Vec<&str> = prefs.floating_cards.keys().map(String::as_str).collect();
        assert_eq!(read, ["Brushes", "Lighting"], "only the entries that are two numbers");
        assert_eq!(prefs.floating_cards["Brushes"], [300.0, 200.0]);
        // And what the interface is handed leaves out the place that is nowhere.
        let ctx = egui::Context::default();
        sculpt_toolkit::floating::restore(&ctx, &prefs.floating_cards);
        assert_eq!(sculpt_toolkit::floating::snapshot(&ctx), BTreeMap::from([("Brushes".to_owned(), [300.0, 200.0])]));
        // The whole map being something else costs nothing but itself either.
        let prefs = Prefs::parse(r#"{ "floating_cards": [1, 2], "right_panel_folded": true }"#);
        assert!(prefs.floating_cards.is_empty() && prefs.right_panel_folded);
    }

    /// A settings file written by this version, read the way the version
    /// before cards could float and panels could fold read it: the fields it
    /// has never heard of are stepped over and everything else is kept.
    #[test]
    fn a_version_without_floating_cards_reads_this_versions_file() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        #[serde(default)]
        struct Before {
            interface_size: f32,
            tooltips: bool,
            cards: BTreeMap<String, bool>,
            brush: BrushPrefs,
            recent: Vec<PathBuf>,
        }
        impl Default for Before {
            fn default() -> Self {
                let now = Prefs::default();
                Before { interface_size: 1.0, tooltips: true, cards: BTreeMap::new(), brush: now.brush, recent: Vec::new() }
            }
        }
        let mut now = Prefs::default();
        now.interface_size = 1.5;
        now.tooltips = false;
        now.left_panel_folded = true;
        now.floating_cards.insert("Brushes".into(), [640.0, 120.0]);
        now.cards.insert("Stroke".into(), true);
        now.brush.radius_px = 91.0;
        let written: serde_json::Value = serde_json::from_str(&serde_json::to_string_pretty(&now).unwrap()).unwrap();
        assert!(written["floating_cards"]["Brushes"].is_array() && written["left_panel_folded"] == true);
        // `Prefs::parse` is this same call, and was before these fields.
        let read = crate::io::read_over(&Before::default(), &written).expect("the older version reads it");
        assert_eq!((read.interface_size, read.tooltips, read.brush.radius_px), (1.5, false, 91.0));
        assert_eq!(read.cards, now.cards);
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
        // Nor about the brushes added after it.
        for later in ["Pose", "Pinch", "Nudge", "Erase Layer", "Paint"] {
            assert!(brush["brushes"].as_object_mut().unwrap().remove(later).is_some());
        }
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
        for later in [Brush::Pose, Brush::Pinch, Brush::Nudge, Brush::EraseLayer] {
            let (now, first_run) = (settings.per_brush[later.index()], Settings::default().per_brush[later.index()]);
            assert_eq!(BrushTuning::of(&now), BrushTuning::of(&first_run), "{later:?}");
        }
        // Nudge starts following half the pen's travel, as Move does: not at the 10% of a brush with none of its own.
        assert_eq!(settings.per_brush[Brush::Nudge.index()].strength, 0.5);
        // And Erase Layer as a whole eraser, which the pen's pressure makes gentle.
        assert_eq!(settings.per_brush[Brush::EraseLayer.index()].strength, 1.0);

        // And what this version writes names the Pose brush and its settings.
        settings.brush = Brush::Pose;
        settings.pose.joints = 3;
        let written = serde_json::to_value(BrushPrefs::of(&settings)).unwrap();
        assert_eq!(written["brush"], "Pose");
        assert_eq!(written["pose_joints"], 3);
        assert!(written["brushes"]["Pose"].is_object());
    }

    /// A settings file from before Move's Strength did anything: the number
    /// it has for it was never felt, so Move starts at its own. What is set
    /// from then on is kept.
    #[test]
    fn a_move_strength_from_before_it_did_anything_is_not_kept() {
        let own = Settings::default().per_brush[Brush::Move.index()].strength;
        let mut older = serde_json::to_value(Prefs::default()).unwrap();
        let brush = older["brush"].as_object_mut().unwrap();
        assert!(brush.remove("move_strength").is_some(), "move_strength is in the file");
        brush["brushes"]["Move"]["strength"] = 0.1.into();
        brush["brushes"]["Clay"]["strength"] = 0.2.into();
        let mut settings = Settings::default();
        Prefs::parse(&older.to_string()).brush.apply(&mut settings);
        assert_eq!(settings.per_brush[Brush::Move.index()].strength, own);
        assert_eq!(settings.per_brush[Brush::Clay.index()].strength, 0.2, "the other brushes keep theirs");

        settings.per_brush[Brush::Move.index()].strength = 0.3;
        let kept = serde_json::to_string(&BrushPrefs::of(&settings)).unwrap();
        let mut back = Settings::default();
        serde_json::from_str::<BrushPrefs>(&kept).unwrap().apply(&mut back);
        assert_eq!(back.per_brush[Brush::Move.index()].strength, 0.3);
        // A brush the file says nothing of starts as any brush does, not as Move.
        assert_eq!(BrushTuning::default().strength, Settings::default().per_brush[Brush::Clay.index()].strength);
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

    /// Each brush's own stroke settings (spacing, scatter, size jitter and
    /// what the pen's pressure does to its size and strength) are kept by
    /// brush, come back inside what their sliders allow, and a file that
    /// says nothing of them leaves every brush as it starts.
    #[test]
    fn each_brushs_stroke_settings_are_kept_and_brought_into_range() {
        let stroke = |b: &BrushSettings| (b.spacing, b.scatter, b.size_jitter, b.pressure_size, b.pressure_strength);
        let mut settings = Settings::default();
        let clay = &mut settings.per_brush[Brush::Clay.index()];
        (clay.spacing, clay.scatter, clay.size_jitter, clay.pressure_size, clay.pressure_strength) = (1.5, 0.4, 0.3, 0.7, 0.25);
        let written = serde_json::to_string(&BrushPrefs::of(&settings)).unwrap();
        let mut back = Settings::default();
        serde_json::from_str::<BrushPrefs>(&written).unwrap().apply(&mut back);
        assert_eq!(stroke(&back.per_brush[Brush::Clay.index()]), (1.5, 0.4, 0.3, 0.7, 0.25));
        for brush in Brush::ALL.into_iter().filter(|&b| b != Brush::Clay) {
            assert_eq!(stroke(&back.per_brush[brush.index()]), (brush.default_spacing(), 0.0, 0.0, 0.0, 0.88), "{brush:?}");
        }

        // A file from before any of this: every brush as it starts, Crease
        // with its own closer spacing.
        let mut older = serde_json::to_value(Prefs::default()).unwrap();
        for (_, tuning) in older["brush"]["brushes"].as_object_mut().unwrap() {
            for key in ["spacing", "scatter", "size_jitter", "pressure_size", "pressure_strength"] {
                assert!(tuning.as_object_mut().unwrap().remove(key).is_some(), "{key} is in the file");
            }
        }
        assert!(older["brush"].as_object_mut().unwrap().remove("pressure_floor").is_some());
        older["brush"]["brushes"]["Clay"]["strength"] = 0.2.into();
        let mut old = Settings::default();
        old.per_brush[Brush::Crease.index()].spacing = 2.0;
        Prefs::parse(&older.to_string()).brush.apply(&mut old);
        for brush in Brush::ALL {
            assert_eq!(stroke(&old.per_brush[brush.index()]), (brush.default_spacing(), 0.0, 0.0, 0.0, 0.88), "{brush:?}");
        }
        assert_eq!(old.per_brush[Brush::Clay.index()].strength, 0.2, "what the file does say is kept");

        // Numbers no slider could have set, at the lowest end and the highest.
        let mut prefs = Prefs::default();
        let tuning = prefs.brush.brushes.get_mut("Layer").unwrap();
        (tuning.spacing, tuning.scatter, tuning.size_jitter) = (0.0, 99.0, 5.0);
        (tuning.pressure_size, tuning.pressure_strength) = (-1.0, f32::NAN);
        let tuning = prefs.brush.brushes.get_mut("Crease").unwrap();
        (tuning.spacing, tuning.scatter, tuning.size_jitter) = (f32::INFINITY, -3.0, f32::NAN);
        (tuning.pressure_size, tuning.pressure_strength) = (4.0, 4.0);
        prefs.brush.apply(&mut back);
        assert_eq!(stroke(&back.per_brush[Brush::Layer.index()]), (0.02, 2.0, 0.9, 0.0, 0.88), "never no spacing: dabs are laid by it");
        assert_eq!(stroke(&back.per_brush[Brush::Crease.index()]), (0.1, 0.0, 0.0, 1.0, 1.0));
    }

    /// Versions before each brush had a Pressure -> Strength of its own
    /// kept one Light-Touch Floor for all of them. Such a file's floor is
    /// carried into every brush on its first load; this version writes none,
    /// so what is set per brush afterwards is what is read back; and the
    /// older version still reads a file this one wrote.
    #[test]
    fn an_older_files_light_touch_floor_carries_over_into_every_brush() {
        let mut older = serde_json::to_value(Prefs::default()).unwrap();
        for (_, tuning) in older["brush"]["brushes"].as_object_mut().unwrap() {
            tuning.as_object_mut().unwrap().remove("pressure_strength");
        }
        assert!(older["brush"]["pressure_floor"].is_null(), "this version writes no floor");
        older["brush"]["pressure_floor"] = 0.3.into();
        let mut settings = Settings::default();
        Prefs::parse(&older.to_string()).brush.apply(&mut settings);
        for brush in Brush::ALL {
            let kept = settings.per_brush[brush.index()].pressure_strength;
            assert!((kept - 0.7).abs() < 1e-6, "{brush:?}: {kept}");
        }
        // The floor every such file had unless it was changed is today's feel.
        older["brush"]["pressure_floor"] = 0.12.into();
        Prefs::parse(&older.to_string()).brush.apply(&mut settings);
        assert!(settings.per_brush.iter().all(|b| (b.pressure_strength - 0.88).abs() < 1e-6));
        // Its slider went from 0 to 0.5.
        for (floor, strength) in [(-2.0, 1.0), (0.0, 1.0), (0.5, 0.5), (9.0, 0.5)] {
            older["brush"]["pressure_floor"] = floor.into();
            Prefs::parse(&older.to_string()).brush.apply(&mut settings);
            assert!(settings.per_brush.iter().all(|b| b.pressure_strength == strength), "a floor of {floor}");
        }
        // A floor that is not a number costs only itself.
        older["brush"]["pressure_floor"] = "low".into();
        older["brush"]["radius_px"] = 77.0.into();
        let mut odd = Settings::default();
        Prefs::parse(&older.to_string()).brush.apply(&mut odd);
        assert_eq!((odd.radius_px, odd.per_brush[0].pressure_strength), (77.0, 0.88));

        // Saved from here on, the floor is gone and each brush has its own.
        settings.per_brush[Brush::Clay.index()].pressure_strength = 0.4;
        let written = serde_json::to_value(BrushPrefs::of(&settings)).unwrap();
        assert!(written["pressure_floor"].is_null());
        let mut back = Settings::default();
        Prefs::parse(&serde_json::json!({ "brush": written }).to_string()).brush.apply(&mut back);
        assert_eq!(back.per_brush[Brush::Clay.index()].pressure_strength, 0.4);
        assert_eq!(back.per_brush[Brush::Layer.index()].pressure_strength, 0.5);

        // The version before reads that file: its one floor stays as it starts.
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        #[serde(default)]
        struct Before {
            radius_px: f32,
            pressure_floor: f32,
            pressure_curve: f32,
        }
        impl Default for Before {
            fn default() -> Self {
                Before { radius_px: 60.0, pressure_floor: 0.12, pressure_curve: 1.6 }
            }
        }
        settings.radius_px = 91.0;
        let written = serde_json::to_value(BrushPrefs::of(&settings)).unwrap();
        let read = crate::io::read_over(&Before::default(), &written).expect("the older version reads it");
        assert_eq!(read, Before { radius_px: 91.0, pressure_floor: 0.12, pressure_curve: 1.6 });
    }

    /// The options of the Smooth and Flatten kernels are each brush's own:
    /// kept by name, left as they start by a file that says nothing of them,
    /// and a mode this version has never heard of costs only itself.
    #[test]
    fn each_brushs_kernel_options_are_kept_by_name() {
        let options = |b: &BrushSettings| (b.smooth_mode, b.keep_volume, b.flatten_mode, b.lock_plane);
        let first_run = (SmoothMode::All, 0.0, FlattenMode::Both, false);
        let mut settings = Settings::default();
        let smooth = &mut settings.per_brush[Brush::Smooth.index()];
        (smooth.smooth_mode, smooth.keep_volume) = (SmoothMode::Peaks, 0.75);
        let flatten = &mut settings.per_brush[Brush::Flatten.index()];
        (flatten.flatten_mode, flatten.lock_plane) = (FlattenMode::Scrape, true);
        let written = serde_json::to_value(BrushPrefs::of(&settings)).unwrap();
        assert_eq!(written["brushes"]["Smooth"]["smooth_mode"], "peaks");
        assert_eq!(written["brushes"]["Flatten"]["flatten_mode"], "scrape");
        let mut back = Settings::default();
        Prefs::parse(&serde_json::json!({ "brush": written }).to_string()).brush.apply(&mut back);
        assert_eq!(options(&back.per_brush[Brush::Smooth.index()]), (SmoothMode::Peaks, 0.75, FlattenMode::Both, false));
        assert_eq!(options(&back.per_brush[Brush::Flatten.index()]), (SmoothMode::All, 0.0, FlattenMode::Scrape, true));
        assert_eq!(options(&back.per_brush[Brush::Clay.index()]), first_run);

        // A file from before: every brush as it starts.
        let mut older = serde_json::to_value(Prefs::default()).unwrap();
        for (_, tuning) in older["brush"]["brushes"].as_object_mut().unwrap() {
            for key in ["smooth_mode", "keep_volume", "flatten_mode", "lock_plane"] {
                assert!(tuning.as_object_mut().unwrap().remove(key).is_some(), "{key} is in the file");
            }
        }
        older["brush"]["brushes"]["Smooth"]["strength"] = 0.4.into();
        Prefs::parse(&older.to_string()).brush.apply(&mut back);
        assert!(back.per_brush.iter().all(|b| options(b) == first_run));
        assert_eq!(back.per_brush[Brush::Smooth.index()].strength, 0.4);

        // A mode from some later version, and numbers no slider could set.
        let odd = serde_json::json!({ "brush": { "brushes": {
            "Smooth": { "smooth_mode": "polish", "keep_volume": 7.0, "strength": 0.6 },
            "Flatten": { "flatten_mode": 3, "lock_plane": "yes", "keep_volume": -1.0, "strength": 0.7 },
            "Layer": { "smooth_mode": "RELAX", "keep_volume": "lots" },
        } } });
        let mut back = Settings::default();
        Prefs::parse(&odd.to_string()).brush.apply(&mut back);
        let (smooth, flatten) = (back.per_brush[Brush::Smooth.index()], back.per_brush[Brush::Flatten.index()]);
        assert_eq!((options(&smooth), smooth.strength), ((SmoothMode::All, 1.0, FlattenMode::Both, false), 0.6));
        assert_eq!((options(&flatten), flatten.strength), ((SmoothMode::All, 0.0, FlattenMode::Both, false), 0.7));
        assert_eq!(options(&back.per_brush[Brush::Layer.index()]), first_run, "names are kept in lower case");
    }
}
