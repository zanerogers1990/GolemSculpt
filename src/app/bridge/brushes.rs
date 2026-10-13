//! The brush library through the Claude bridge: `set_brush` picking any
//! brush of the library by name, listing it, duplicating, renaming and
//! deleting, importing and exporting by path, and setting the values of a
//! brush's record that `set_brush` did not already reach.
//!
//! Three steps of `set_brush` are here, in the order they are taken:
//! [`Running::bridge_library`] (which brush is in hand), then the values
//! (`set_brush`'s own, and [`Running::bridge_brush_values`]), then
//! [`Running::bridge_library_after`] (what is done with the brush as it now
//! is: export it, delete it). Import and export take a path and never open
//! a file box; the path is held to the bridge's rule about paths.

use serde_json::{Value, json};

use super::{flag, key, num, round, text};
use crate::app::Running;
use crate::sculpt::alpha::AlphaSettings;
use crate::sculpt::library::{Ask, Library, Origin};
use crate::sculpt::{Brush, FalloffCurve, FalloffPreset};

/// A brush's place in the library by a name as a caller may write it:
/// spaces, case and punctuation aside.
fn find(library: &Library, name: &str) -> Result<usize, String> {
    let wanted = key(name);
    library.named(name).or_else(|| library.entries().iter().position(|e| key(&e.name) == wanted)).ok_or_else(|| {
        let names: Vec<&str> = library.entries().iter().map(|e| e.name.as_str()).collect();
        format!("There is no brush \"{name}\". The brushes are: {}.", names.join(", "))
    })
}

fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::Base => "base",
        Origin::Shipped => "shipped",
        Origin::User => "yours",
    }
}

/// A falloff curve as a caller gives it: "default" for the kernel's own, the
/// name of a starting shape, or a list of [distance, weight] points.
fn falloff(value: &Value) -> Result<Option<FalloffCurve>, String> {
    const SHAPE: &str = "falloff is \"default\", \"smooth\", \"sharp\", \"plateau\", \"linear\", or a list of \
                         [distance, weight] points, each from 0 to 1, with distance 0 the brush's centre.";
    if let Some(name) = value.as_str() {
        if key(name) == "default" {
            return Ok(None);
        }
        let shape = FalloffPreset::ALL.into_iter().find(|shape| key(shape.name()) == key(name)).ok_or(SHAPE)?;
        return Ok(Some(FalloffCurve::preset(shape)));
    }
    let points = value.as_array().filter(|points| points.len() >= 2).ok_or(SHAPE)?;
    let part = |p: &Value, i: usize| p.get(i).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite());
    let points: Option<Vec<glam::Vec2>> = points.iter().map(|p| Some(glam::Vec2::new(part(p, 0)?, part(p, 1)?))).collect();
    let mut curve = FalloffCurve { points: points.ok_or(SHAPE)? };
    curve.normalize();
    Ok(Some(curve))
}

impl Running {
    /// `set_brush`, first step: the brush in hand. `brush` picks one by
    /// name, `import_file` brings one in, `duplicate_as` copies the one in
    /// hand under a new name, `rename` renames it and `reset` puts it back
    /// as it shipped or was made. Gives back what the app would have said
    /// on its status line for each.
    pub(super) fn bridge_library(&mut self, args: &Value) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        if let Some(name) = text(args, "brush") {
            let brush = find(&self.doc.sculptor.library, name)?;
            self.brush_edit(Ask::Select(brush))?;
        }
        if args.get("import_file").is_some() {
            let (path, extension) = Self::path_named(args, "import_file")?;
            if extension != "json" {
                return Err("A brush file is a .json file.".into());
            }
            said.push(self.import_brush(&path)?);
        }
        if let Some(name) = text(args, "duplicate_as") {
            let library = &self.doc.sculptor.library;
            if let Some(why) = library.name_refused(name, None) {
                return Err(why);
            }
            let from = library.selected(&self.doc.sculptor.settings);
            self.brush_edit(Ask::Duplicate(from))?;
            let copy = self.doc.sculptor.library.selected(&self.doc.sculptor.settings);
            said.push(self.brush_edit(Ask::Rename(copy, name.to_owned()))?);
        }
        let in_hand = self.doc.sculptor.library.selected(&self.doc.sculptor.settings);
        if let Some(name) = text(args, "rename") {
            said.push(self.brush_edit(Ask::Rename(in_hand, name.to_owned()))?);
        }
        if flag(args, "reset") == Some(true) {
            said.push(self.brush_edit(Ask::Reset(in_hand))?);
        }
        Ok(said)
    }

    /// `set_brush`, the values of the record its own code does not set:
    /// Build-Up and Front Facing, the kernel's own numbers, the rest of the
    /// alpha and the falloff curve. A kernel's number is refused on a brush
    /// of another kernel, where it would silently do nothing.
    pub(super) fn bridge_brush_values(&mut self, args: &Value) -> Result<(), String> {
        let s = &mut self.doc.sculptor.settings;
        let (kernel, slot) = (s.brush, s.brush.index());
        let within = |name: &str, low: f32, high: f32| num(args, name).map(|value| value.clamp(low, high));
        let b = s.current_mut();
        b.build_up = within("build_up", 0.1, 3.0).unwrap_or(b.build_up);
        b.front_facing = flag(args, "front_facing").unwrap_or(b.front_facing);
        b.stay_in_group = flag(args, "stay_in_group").unwrap_or(b.stay_in_group);
        // Each number with the kernel that reads it and the range of its slider.
        let numbers: [(&str, Brush, &mut f32, f32, f32); 7] = [
            ("clay_depth", Brush::Clay, &mut s.clay_depth, 0.1, 1.5),
            ("clay_softness", Brush::Clay, &mut s.clay_softness, 0.05, 1.0),
            ("clay_relax", Brush::Clay, &mut s.clay_relax, 0.0, 1.0),
            ("crease_pinch", Brush::Crease, &mut s.crease_pinch, 0.0, 1.0),
            ("crease_depth", Brush::Crease, &mut s.crease_depth, 0.05, 1.0),
            ("flatten_offset", Brush::Flatten, &mut s.flatten_offset, -0.5, 0.5),
            ("layer_height", Brush::Layer, &mut s.layer_height, 0.02, 1.0),
        ];
        let not_its = |name: &str, of: Brush| {
            format!("{name} is a {} brush's setting, and the brush in hand is a {} brush.", of.name(), kernel.name())
        };
        for (name, of, value, low, high) in numbers {
            let Some(given) = within(name, low, high) else { continue };
            if kernel != of {
                return Err(not_its(name, of));
            }
            *value = given;
        }
        if let Some(on) = flag(args, "move_connected") {
            if kernel != Brush::Move {
                return Err(not_its("move_connected", Brush::Move));
            }
            s.move_connected = on;
        }
        if let Some(given) = args.get("falloff").filter(|value| !value.is_null()) {
            s.falloff[slot] = falloff(given)?;
        }
        let (scale, a) = (AlphaSettings::SCALE, &mut s.alpha[slot]);
        const ALPHA: [&str; 6] =
            ["alpha_size", "alpha_rotation", "alpha_invert", "alpha_repeat", "alpha_turn_with_stroke", "alpha_drag_to_place"];
        if !kernel.takes_alpha() && ALPHA.iter().any(|name| args.get(name).is_some_and(|value| !value.is_null())) {
            return Err(format!("{} does not print an alpha. Standard, Clay, Inflate and Layer do.", kernel.name()));
        }
        a.scale = within("alpha_size", *scale.start(), *scale.end()).unwrap_or(a.scale);
        a.rotation = within("alpha_rotation", 0.0, 360.0).unwrap_or(a.rotation);
        a.invert = flag(args, "alpha_invert").unwrap_or(a.invert);
        a.tile = flag(args, "alpha_repeat").unwrap_or(a.tile);
        a.follow = flag(args, "alpha_turn_with_stroke").unwrap_or(a.follow);
        a.drag = flag(args, "alpha_drag_to_place").unwrap_or(a.drag);
        Ok(())
    }

    /// `set_brush`, last step: what is done with the brush in hand as it
    /// now is. `pinned` puts it on the Space wheel or takes it off,
    /// `shift_uses` makes it the brush Shift smooths with, `export_file`
    /// writes it to a file, and `delete` takes it out of the library. Gives
    /// back what the app would have said on its status line for each.
    pub(super) fn bridge_library_after(&mut self, args: &Value) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        let in_hand = self.doc.sculptor.library.selected(&self.doc.sculptor.settings);
        if let Some(pinned) = flag(args, "pinned") {
            self.brush_edit(Ask::Pin(in_hand, pinned))?;
        }
        if let Some(on) = flag(args, "shift_uses") {
            // Off hands Shift back to the base Smooth.
            said.push(self.brush_edit(Ask::ShiftUses(if on { in_hand } else { Library::base(Brush::Smooth) }))?);
        }
        if args.get("export_file").is_some() {
            let (path, extension) = Self::path_named(args, "export_file")?;
            if extension != "json" {
                return Err("A brush file is a .json file: give a path that ends in .json.".into());
            }
            said.push(self.export_brush(in_hand, &path)?);
        }
        if flag(args, "delete") == Some(true) {
            said.push(self.brush_edit(Ask::Delete(in_hand))?);
        }
        Ok(said)
    }

    /// What `brush_state` says of the brush in hand as a brush of the
    /// library, and of the library itself, added to `state`.
    pub(super) fn bridge_brush_state(&self, state: &mut Value) {
        let (library, s) = (&self.doc.sculptor.library, &self.doc.sculptor.settings);
        let in_hand = library.selected(s);
        let entry = &library.entries()[in_hand];
        let (b, a, slot) = (s.current(), &s.alpha[s.brush.index()], s.brush.index());
        let numbers = match entry.kernel {
            Brush::Clay => json!({
                "clay_depth": round(s.clay_depth),
                "clay_softness": round(s.clay_softness),
                "clay_relax": round(s.clay_relax),
            }),
            Brush::Crease => json!({ "crease_pinch": round(s.crease_pinch), "crease_depth": round(s.crease_depth) }),
            Brush::Flatten => json!({ "flatten_offset": round(s.flatten_offset) }),
            Brush::Layer => json!({ "layer_height": round(s.layer_height) }),
            Brush::Move => json!({ "move_connected": s.move_connected }),
            _ => json!({}),
        };
        let curve = s.falloff[slot].as_ref().map(|curve| curve.points.iter().map(|p| [round(p.x), round(p.y)]).collect::<Vec<_>>());
        let more = json!({
            "name": entry.name,
            "kernel": entry.kernel.name(),
            "origin": origin_name(entry.origin),
            "pinned": entry.pinned,
            "shift_uses": library.shift() == in_hand,
            "build_up": round(b.build_up),
            "front_facing": b.front_facing,
            "stay_in_group": b.stay_in_group,
            "falloff": curve,
            "alpha_size": round(a.scale),
            "alpha_rotation": round(a.rotation),
            "alpha_invert": a.invert,
            "alpha_repeat": a.tile,
            "alpha_turn_with_stroke": a.follow,
            "alpha_drag_to_place": a.drag,
            "shift_brush": library.entries()[library.shift()].name,
            "library": library.entries().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        });
        let Some(state) = state.as_object_mut() else { return };
        for part in [more, numbers] {
            if let Value::Object(fields) = part {
                state.extend(fields);
            }
        }
    }

    /// Every brush of the library with what it is, for `set_brush` with
    /// `list` on.
    pub(super) fn bridge_library_list(&self) -> Value {
        let library = &self.doc.sculptor.library;
        let brushes: Vec<Value> = library
            .entries()
            .iter()
            .enumerate()
            .map(|(i, e)| {
                json!({
                    "name": e.name,
                    "kernel": e.kernel.name(),
                    "origin": origin_name(e.origin),
                    "pinned": e.pinned,
                    "shift_uses": library.shift() == i,
                    "about": crate::sculpt::library::about(&e.name),
                })
            })
            .collect();
        json!(brushes)
    }
}
