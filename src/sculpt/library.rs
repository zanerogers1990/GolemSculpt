//! The brush library: every brush there is to pick, each a named record of
//! settings over one of the kernels.
//!
//! In ZBrush and Nomad a brush in the palette is a preset over a base tool:
//! Clay Buildup, Dam Standard and Trim Dynamic are each a kernel with a
//! falloff, an alpha, a spacing, a pressure mapping and a few numbers. Here
//! a brush is a [`Record`] of exactly that, and the library holds any number
//! of them over the fourteen kernels: the fourteen the kernels start as (the
//! base brushes, which keep the number keys and cannot be deleted), the ones
//! that ship with the app (`shipped.rs`) and the user's own.
//!
//! # How a brush is in hand
//!
//! The stroke code reads one working set of settings per kernel
//! (`Settings::per_brush` and the rest), as it did when there were only the
//! twelve. That is left exactly as it was, and with it every stroke test.
//! The library works by swapping: for each kernel one brush is LIVE, its
//! record laid in the kernel's working set, where the panel edits it and
//! strokes read it. Picking another brush of that kernel lifts the live
//! one's settings back into its record and lays the new one's down. So an
//! edit sticks to the brush it was made on with no save button, and picking
//! brush B cannot change brush A: A's settings are in A's record by the
//! time B's are laid, and a brush of another kernel never touches the set.
//!
//! The other way (strokes reading a record straight from a list) would have
//! put the library between every stroke and its settings, for the same
//! result. This way the library is something laid over the settings, and a
//! `Sculptor` nobody ever picks a brush on behaves as it always did.
//!
//! The brush in hand is the live brush of `Settings::brush`'s kernel. What a
//! live brush's settings are is always asked of the working set
//! ([`Library::record`]); an entry's own record is only current while the
//! brush is not live.
//!
//! # Shift
//!
//! Shift smooths with any brush in hand, and one Smooth-kernel brush is
//! marked as the one it uses. When that brush is not the live Smooth,
//! strokes are handed its record apart (`Settings::shift`).

pub mod shipped;

use std::ops::RangeInclusive;

use super::alpha::AlphaSettings;
use super::{Brush, BrushSettings, FalloffCurve, PoseSettings, Settings, ShiftBrush};
pub use shipped::about;

/// The numbers that belong to one kernel alone. A record keeps them all,
/// and only its own kernel's mean anything in it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Numbers {
    pub clay_depth: f32,
    pub clay_softness: f32,
    pub clay_relax: f32,
    pub crease_pinch: f32,
    pub crease_depth: f32,
    pub flatten_offset: f32,
    pub layer_height: f32,
    pub move_connected: bool,
    pub pose: PoseSettings,
}

impl Numbers {
    /// Every kernel's numbers as the working settings have them.
    pub fn of(s: &Settings) -> Numbers {
        Numbers {
            clay_depth: s.clay_depth,
            clay_softness: s.clay_softness,
            clay_relax: s.clay_relax,
            crease_pinch: s.crease_pinch,
            crease_depth: s.crease_depth,
            flatten_offset: s.flatten_offset,
            layer_height: s.layer_height,
            move_connected: s.move_connected,
            pose: s.pose,
        }
    }

    /// These, with the ones `kernel` reads taken from `from`. Every move of
    /// a kernel's numbers between a record and the working settings goes
    /// through here, so a brush never carries off another kernel's.
    pub fn with_own(mut self, kernel: Brush, from: &Numbers) -> Numbers {
        match kernel {
            Brush::Clay => {
                (self.clay_depth, self.clay_softness) = (from.clay_depth, from.clay_softness);
                self.clay_relax = from.clay_relax;
            }
            Brush::Crease => (self.crease_pinch, self.crease_depth) = (from.crease_pinch, from.crease_depth),
            Brush::Flatten => self.flatten_offset = from.flatten_offset,
            Brush::Layer => self.layer_height = from.layer_height,
            Brush::Move => self.move_connected = from.move_connected,
            Brush::Pose => self.pose = from.pose,
            Brush::Smooth | Brush::Inflate | Brush::Standard | Brush::SnakeHook | Brush::Pinch | Brush::Nudge => {}
            Brush::EraseLayer | Brush::Paint => {}
        }
        self
    }

    fn in_range(mut self) -> Numbers {
        let d = Numbers::of(&Settings::default());
        self.clay_depth = fit(self.clay_depth, 0.1..=1.5, d.clay_depth);
        self.clay_softness = fit(self.clay_softness, 0.05..=1.0, d.clay_softness);
        self.clay_relax = fit(self.clay_relax, 0.0..=1.0, d.clay_relax);
        self.crease_pinch = fit(self.crease_pinch, 0.0..=1.0, d.crease_pinch);
        self.crease_depth = fit(self.crease_depth, 0.05..=1.0, d.crease_depth);
        self.flatten_offset = fit(self.flatten_offset, -0.5..=0.5, d.flatten_offset);
        self.layer_height = fit(self.layer_height, 0.02..=1.0, d.layer_height);
        self.pose.joints = self.pose.joints.clamp(1, PoseSettings::MAX_JOINTS);
        self.pose.offset = fit(self.pose.offset, 0.0..=PoseSettings::MAX_OFFSET, d.pose.offset);
        self.pose.rigidity = fit(self.pose.rigidity, 0.0..=1.0, d.pose.rigidity);
        self
    }

    fn lay(&self, s: &mut Settings) {
        (s.clay_depth, s.clay_softness, s.clay_relax) = (self.clay_depth, self.clay_softness, self.clay_relax);
        (s.crease_pinch, s.crease_depth) = (self.crease_pinch, self.crease_depth);
        (s.flatten_offset, s.layer_height) = (self.flatten_offset, self.layer_height);
        (s.move_connected, s.pose) = (self.move_connected, self.pose);
    }
}

/// One brush's settings: everything that shapes its stroke but the size of
/// the ring, which every brush shares, and what holds for every stroke
/// (Lazy Mouse, the pen's curve, symmetry).
#[derive(Clone, PartialEq)]
pub struct Record {
    pub settings: BrushSettings,
    /// Its hand-drawn falloff, or `None` while it keeps its kernel's own.
    pub falloff: Option<FalloffCurve>,
    pub alpha: AlphaSettings,
    pub numbers: Numbers,
}

impl Record {
    /// What `kernel`'s working set holds. `keep` is where the numbers of
    /// the other kernels come from: they are not this brush's to take.
    fn lifted(s: &Settings, kernel: Brush, keep: &Numbers) -> Record {
        let slot = kernel.index();
        Record {
            settings: s.per_brush[slot],
            falloff: s.falloff[slot].clone(),
            alpha: s.alpha[slot],
            numbers: keep.with_own(kernel, &Numbers::of(s)),
        }
    }

    /// Lays the record in `kernel`'s working set.
    fn lay(&self, s: &mut Settings, kernel: Brush) {
        let slot = kernel.index();
        s.per_brush[slot] = self.settings;
        s.falloff[slot] = self.falloff.clone();
        s.alpha[slot] = self.alpha;
        Numbers::of(s).with_own(kernel, &self.numbers).lay(s);
    }

    /// How `kernel`'s base brush starts.
    pub fn base(kernel: Brush) -> Record {
        let s = Settings::default();
        Record::lifted(&s, kernel, &Numbers::of(&s))
    }

    /// The record with every number inside what its slider allows: what a
    /// record read from a file is put through, so that nothing a file says
    /// reaches the brush code out of range (several of these are divided
    /// by). The ranges are the ones the settings file is held to.
    pub fn in_range(mut self) -> Record {
        self.settings = settings_in_range(self.settings);
        let a = &mut self.alpha;
        a.rotation = fit(a.rotation, 0.0..=360.0, 0.0);
        a.scale = fit(a.scale, AlphaSettings::SCALE, 1.0);
        a.random_rotation = fit(a.random_rotation, AlphaSettings::RANDOM_ROTATION, 0.0);
        if let Some(curve) = &mut self.falloff {
            curve.normalize();
        }
        self.numbers = self.numbers.in_range();
        self
    }
}

/// A number brought into `range`; one that is not a number gives way to `or`.
fn fit(value: f32, range: RangeInclusive<f32>, or: f32) -> f32 {
    if value.is_finite() { value.clamp(*range.start(), *range.end()) } else { or }
}

fn settings_in_range(mut b: BrushSettings) -> BrushSettings {
    // A brush with nothing of its own about it, for what is not a number.
    let was = Settings::default().per_brush[Brush::Smooth.index()];
    b.strength = fit(b.strength, 0.01..=1.0, was.strength);
    b.build_up = fit(b.build_up, 0.1..=3.0, was.build_up);
    b.focal_shift = fit(b.focal_shift, -1.0..=1.0, was.focal_shift);
    b.spacing = fit(b.spacing, BrushSettings::SPACING, was.spacing);
    b.scatter = fit(b.scatter, BrushSettings::SCATTER, was.scatter);
    b.size_jitter = fit(b.size_jitter, BrushSettings::SIZE_JITTER, was.size_jitter);
    b.pressure_size = fit(b.pressure_size, BrushSettings::PRESSURE, was.pressure_size);
    b.pressure_strength = fit(b.pressure_strength, BrushSettings::PRESSURE, was.pressure_strength);
    b.keep_volume = fit(b.keep_volume, 0.0..=1.0, was.keep_volume);
    b
}

/// Where a brush came from, which says what may be done with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// One of the fourteen the kernels start as. It has the kernel's name and
    /// number key, and can be neither renamed nor deleted.
    Base,
    /// One that ships with the app. It can be deleted, and Reset puts it
    /// back as it shipped.
    Shipped,
    /// One of the user's own: a duplicate, or one brought in from a file.
    User,
}

/// A brush of the library.
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub kernel: Brush,
    pub origin: Origin,
    /// On the hold-Space wheel. A base brush starts on it or off it as its
    /// kernel says (`Brush::starts_on_wheel`), a shipped one as
    /// `shipped::ON_WHEEL` says; the user's own start off.
    pub pinned: bool,
    /// Its settings, as of when it was last live (see the top of this file).
    record: Record,
    /// What Reset goes back to: how it shipped, or for one of the user's
    /// own, how it was when it was made.
    made: Record,
}

impl Entry {
    /// A brush that ships with the app, as it ships.
    fn shipped(name: &str, kernel: Brush, record: Record) -> Entry {
        let pinned = shipped::ON_WHEEL.contains(&name);
        Entry { name: name.to_owned(), kernel, origin: Origin::Shipped, pinned, made: record.clone(), record }
    }

    /// Whether the brush is on the wheel as the app starts out.
    fn starts_pinned(&self) -> bool {
        match self.origin {
            Origin::Base => self.kernel.starts_on_wheel(),
            Origin::Shipped => shipped::ON_WHEEL.contains(&self.name.as_str()),
            Origin::User => false,
        }
    }

    /// How the brush was when it shipped or was made.
    pub fn made(&self) -> &Record {
        &self.made
    }
}

/// The longest a brush's name may be: what fits beside its icon in the list.
pub const MAX_NAME: usize = 28;

/// Something the panel asks to have done with the library. It is the app
/// that does it (`app/brushes.rs`): the brush folder and the file boxes are
/// the app's, and it has the status line to say what happened on.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    Select(usize),
    Duplicate(usize),
    Rename(usize, String),
    Delete(usize),
    Reset(usize),
    Pin(usize, bool),
    /// Put the wheel back to the brushes it starts with.
    ResetWheel,
    /// Shift smooths with this brush from now on.
    ShiftUses(usize),
    /// Pick a brush file and bring it in. Opens a file box.
    Import,
    /// Pick where to write this brush. Opens a file box.
    Export(usize),
    /// Bring back the shipped brushes that were deleted.
    RestoreShipped,
}

pub struct Library {
    /// The base brushes first, in `Brush::ALL`'s order (so a kernel's base
    /// brush is at the kernel's own index, always), then the shipped, then
    /// the user's.
    entries: Vec<Entry>,
    /// For each kernel, the brush that is live in its working set.
    live: [usize; Brush::COUNT],
    /// The Smooth-kernel brush Shift smooths with.
    shift: usize,
    /// Shipped brushes the user has deleted, by name.
    removed: Vec<String>,
}

impl Default for Library {
    fn default() -> Library {
        let base = Brush::ALL.map(|kernel| {
            let record = Record::base(kernel);
            let pinned = kernel.starts_on_wheel();
            Entry { name: kernel.name().to_owned(), kernel, origin: Origin::Base, pinned, made: record.clone(), record }
        });
        let shipped = shipped::all().into_iter().map(|(name, kernel, record)| Entry::shipped(name, kernel, record));
        Library {
            entries: base.into_iter().chain(shipped).collect(),
            live: std::array::from_fn(|kernel| kernel),
            shift: Brush::Smooth.index(),
            removed: Vec::new(),
        }
    }
}

impl Library {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The brush in hand: the live brush of the kernel the settings are on.
    pub fn selected(&self, s: &Settings) -> usize {
        self.live[s.brush.index()]
    }

    /// `kernel`'s base brush.
    pub fn base(kernel: Brush) -> usize {
        kernel.index()
    }

    /// The brush Shift smooths with.
    pub fn shift(&self) -> usize {
        self.shift
    }

    /// What a stroke begun now goes by: "Mask" while a mask is painted, the
    /// brush Shift smooths with when `shift` is held, else the brush in hand.
    pub fn stroke_name(&self, s: &Settings, shift: bool) -> &str {
        if s.mask_mode {
            return "Mask";
        }
        &self.entries[if shift { self.shift } else { self.selected(s) }].name
    }

    /// The brush called `name`, whatever the case of its letters.
    pub fn named(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.name.eq_ignore_ascii_case(name.trim()))
    }

    /// The names of the shipped brushes the user has deleted.
    pub fn removed(&self) -> &[String] {
        &self.removed
    }

    fn is_live(&self, i: usize) -> bool {
        self.live[self.entries[i].kernel.index()] == i
    }

    /// Brush `i`'s settings as they are now: the working set's if it is
    /// live, else its own record.
    pub fn record(&self, i: usize, s: &Settings) -> Record {
        let entry = &self.entries[i];
        if self.is_live(i) { Record::lifted(s, entry.kernel, &entry.record.numbers) } else { entry.record.clone() }
    }

    /// Lifts the live brush of `kernel` out of the working set into its
    /// record.
    fn lift(&mut self, kernel: Brush, s: &Settings) {
        let i = self.live[kernel.index()];
        self.entries[i].record = Record::lifted(s, kernel, &self.entries[i].record.numbers);
    }

    /// Makes brush `i` its kernel's live brush without taking it in hand.
    fn make_live(&mut self, i: usize, s: &mut Settings) {
        let kernel = self.entries[i].kernel;
        if self.live[kernel.index()] != i {
            self.lift(kernel, s);
            self.live[kernel.index()] = i;
            self.entries[i].record.lay(s, kernel);
        }
        self.hand_shift(s);
    }

    /// Makes every kernel's base brush its live one; whatever else was live
    /// goes back into its record as it is. The settings file's brush
    /// settings are the base brushes', so this comes before a settings file
    /// is laid in the working sets ([`Self::restore`] comes after).
    pub fn lay_base(&mut self, s: &mut Settings) {
        for kernel in Brush::ALL {
            self.make_live(Self::base(kernel), s);
        }
    }

    /// Takes brush `i` in hand. Mask painting is left, as it is whenever a
    /// brush is picked, so the pick never looks dead.
    pub fn select(&mut self, i: usize, s: &mut Settings) {
        if i >= self.entries.len() {
            return;
        }
        self.make_live(i, s);
        (s.brush, s.mask_mode) = (self.entries[i].kernel, false);
    }

    /// Takes the brush called `name` in hand. False if there is none.
    pub fn select_named(&mut self, name: &str, s: &mut Settings) -> bool {
        let found = self.named(name);
        if let Some(i) = found {
            self.select(i, s);
        }
        found.is_some()
    }

    /// Keeps `Settings::shift` in step: the Shift brush's record, unless
    /// that brush is the live Smooth, whose working set strokes read anyway.
    fn hand_shift(&self, s: &mut Settings) {
        s.shift = (!self.is_live(self.shift)).then(|| {
            let record = &self.entries[self.shift].record;
            ShiftBrush { settings: record.settings, falloff: record.falloff.clone() }
        });
    }

    /// Marks brush `i` as the one Shift smooths with. Only a Smooth-kernel
    /// brush can be; false for any other.
    pub fn set_shift(&mut self, i: usize, s: &mut Settings) -> bool {
        let fits = self.entries.get(i).is_some_and(|e| e.kernel == Brush::Smooth);
        if fits {
            self.shift = i;
            self.hand_shift(s);
        }
        fits
    }

    pub fn set_pinned(&mut self, i: usize, pinned: bool) {
        if let Some(entry) = self.entries.get_mut(i) {
            entry.pinned = pinned;
        }
    }

    /// Puts every brush back on the wheel or off it as the app starts out.
    pub fn reset_pins(&mut self) {
        for entry in &mut self.entries {
            entry.pinned = entry.starts_pinned();
        }
    }

    /// Why `name` cannot be a brush's name, if it cannot: empty, too long,
    /// or some other brush's (one that ships with the app keeps its name
    /// even while it is deleted, so that it can come back). `but` is the
    /// brush being named, which may keep its own.
    pub fn name_refused(&self, name: &str, but: Option<usize>) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return Some("A brush needs a name.".to_owned());
        }
        if name.chars().count() > MAX_NAME {
            return Some(format!("A brush's name can be {MAX_NAME} letters at most."));
        }
        let taken = self.entries.iter().enumerate().any(|(i, e)| Some(i) != but && e.name.eq_ignore_ascii_case(name))
            || self.removed.iter().any(|gone| gone.eq_ignore_ascii_case(name));
        taken.then(|| format!("There is already a brush called {name}."))
    }

    /// A name no brush has, starting from `wanted`: itself, or with the
    /// first number after it that is free.
    pub fn free_name(&self, wanted: &str) -> String {
        let wanted: String = wanted.trim().chars().take(MAX_NAME - 3).collect();
        let wanted = if wanted.is_empty() { "Brush".to_owned() } else { wanted };
        if self.name_refused(&wanted, None).is_none() {
            return wanted;
        }
        (2..).map(|n| format!("{wanted} {n}")).find(|name| self.name_refused(name, None).is_none()).unwrap_or(wanted)
    }

    /// Adds a brush of the user's own and gives back where it is.
    pub fn add(&mut self, name: &str, kernel: Brush, record: Record, made: Record) -> usize {
        let name = self.free_name(name);
        self.entries.push(Entry { name, kernel, origin: Origin::User, pinned: false, record, made });
        self.entries.len() - 1
    }

    /// Duplicates brush `i` as a new brush of the user's own, as it is now,
    /// and takes the new one in hand. Reset on the new brush goes back to
    /// this moment.
    pub fn duplicate(&mut self, i: usize, s: &mut Settings) -> usize {
        let record = self.record(i, s);
        let (name, kernel) = (format!("{} Copy", self.entries[i].name), self.entries[i].kernel);
        let new = self.add(&name, kernel, record.clone(), record);
        self.select(new, s);
        new
    }

    /// Renames one of the user's brushes.
    pub fn rename(&mut self, i: usize, name: &str) -> Result<(), String> {
        let entry = self.entries.get(i).ok_or("There is no such brush.")?;
        if entry.origin != Origin::User {
            return Err(format!("{} came with the app and keeps its name. Duplicate it and name the copy.", entry.name));
        }
        if let Some(why) = self.name_refused(name, Some(i)) {
            return Err(why);
        }
        self.entries[i].name = name.trim().to_owned();
        Ok(())
    }

    /// Deletes brush `i`: any but the fourteen base brushes. If it was in
    /// hand, or live, or the one Shift uses, its kernel's base brush takes
    /// its place.
    pub fn delete(&mut self, i: usize, s: &mut Settings) -> Result<(), String> {
        let entry = self.entries.get(i).ok_or("There is no such brush.")?;
        if entry.origin == Origin::Base {
            return Err(format!("{} is one of the fourteen base brushes, which stay.", entry.name));
        }
        let (kernel, in_hand) = (entry.kernel, self.selected(s) == i);
        if self.is_live(i) {
            // The base brush's record goes down over it; nothing is lifted.
            self.live[kernel.index()] = Self::base(kernel);
            self.entries[Self::base(kernel)].record.lay(s, kernel);
        }
        if self.shift == i {
            self.shift = Self::base(Brush::Smooth);
        }
        let gone = self.entries.remove(i);
        if gone.origin == Origin::Shipped {
            self.removed.push(gone.name);
        }
        // Everything after it has moved up one.
        let moved = |at: &mut usize| *at -= usize::from(*at > i);
        self.live.iter_mut().for_each(moved);
        moved(&mut self.shift);
        self.hand_shift(s);
        if in_hand {
            self.select(Self::base(kernel), s);
        }
        Ok(())
    }

    /// Puts brush `i` back as it shipped or, one of the user's own, as it
    /// was made.
    pub fn reset(&mut self, i: usize, s: &mut Settings) {
        let Some(entry) = self.entries.get_mut(i) else { return };
        entry.record = entry.made.clone();
        let (kernel, record) = (entry.kernel, entry.record.clone());
        if self.is_live(i) {
            record.lay(s, kernel);
        }
        self.hand_shift(s);
    }

    /// Brings back the shipped brushes that were deleted, as they shipped
    /// and each in the place it had.
    pub fn restore_shipped(&mut self) {
        self.removed.clear();
        // Walking the shipped list in order, each brush belongs right after
        // the one before it.
        let mut at = Brush::COUNT;
        for (name, kernel, record) in shipped::all() {
            if self.entries.get(at).is_none_or(|e| e.name != name) {
                self.entries.insert(at, Entry::shipped(name, kernel, record));
                // Everything from there on has moved down one.
                let moved = |i: &mut usize| *i += usize::from(*i >= at);
                self.live.iter_mut().for_each(moved);
                moved(&mut self.shift);
            }
            at += 1;
        }
    }

    /// Sets brush `i`'s record: what a file read for it. Laid in the working
    /// set at once if the brush is live.
    pub fn set_record(&mut self, i: usize, record: Record, s: &mut Settings) {
        let Some(entry) = self.entries.get_mut(i) else { return };
        entry.record = record.in_range();
        let (kernel, record) = (entry.kernel, entry.record.clone());
        if self.is_live(i) {
            record.lay(s, kernel);
        }
        self.hand_shift(s);
    }

    /// The working settings as they would be with every kernel's BASE brush
    /// live: what the settings file keeps under each kernel's name, as it
    /// did when the twelve were all there was.
    pub fn base_image(&self, s: &Settings) -> Settings {
        let mut image = s.clone();
        for kernel in Brush::ALL {
            if self.live[kernel.index()] != Self::base(kernel) {
                self.entries[Self::base(kernel)].record.lay(&mut image, kernel);
            }
        }
        image.shift = None;
        image
    }

    /// Start-up, once the settings file has been laid in the working sets:
    /// those are the base brushes' settings, and every base brush is live.
    /// The rest of what was remembered follows: the brushes deleted, the
    /// pins, the Shift brush and the brush in hand, each by name. A name
    /// that is no brush's any more costs only itself.
    pub fn restore(&mut self, removed: &[String], pinned: Option<&[String]>, shift: &str, selected: &str, s: &mut Settings) {
        for name in removed {
            if let Some(i) = self.named(name).filter(|&i| self.entries[i].origin == Origin::Shipped) {
                let _ = self.delete(i, s);
            }
        }
        if let Some(pinned) = pinned {
            for entry in &mut self.entries {
                entry.pinned = pinned.iter().any(|name| name.eq_ignore_ascii_case(&entry.name));
            }
        }
        if let Some(i) = self.named(shift) {
            self.set_shift(i, s);
        }
        // The kernel the settings file left the app on is its base brush,
        // unless the file names a brush.
        let (kernel, mask) = (s.brush, s.mask_mode);
        self.select(self.named(selected).unwrap_or(Self::base(kernel)), s);
        s.mask_mode = mask;
    }
}
