//! The Claude bridge's `deform` tool: a deformer with its settings, applied
//! to the selection as one undo step, or left on the model for the person at
//! the screen to apply or cancel. Numbers are given as the card shows them:
//! degrees, and percentages of the range or of the model.

use serde_json::Value;

use super::super::Running;
use crate::deform::{Anchor, Kind, Lattice, Params, lattice};
use crate::doc::{Action, Tool};
use crate::ui::deform::Ask;

fn number(args: &Value, name: &str) -> Option<f32> {
    args.get(name).and_then(Value::as_f64).map(|v| v as f32)
}

fn word(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(|s| s.to_lowercase().replace([' ', '_', '-'], ""))
}

/// Up to three numbers in a list.
fn numbers<const N: usize>(args: &Value, name: &str) -> Result<Option<[f32; N]>, String> {
    let Some(list) = args.get(name).filter(|v| !v.is_null()) else { return Ok(None) };
    let values: Vec<f32> = list.as_array().into_iter().flatten().filter_map(Value::as_f64).map(|v| v as f32).collect();
    let got: [f32; N] = values.try_into().map_err(|_| format!("{name} needs {N} numbers."))?;
    Ok(Some(got))
}

/// The settings the arguments ask for, from the defaults the card starts with.
pub(super) fn params_from(args: &Value) -> Result<Params, String> {
    let wanted = word(args, "kind").ok_or("deform needs a kind.")?;
    let kind = Kind::ALL
        .into_iter()
        .find(|k| k.name().to_lowercase().replace(' ', "") == wanted)
        .ok_or_else(|| format!("There is no deformer \"{wanted}\". They are: bend, bend_arc, twist, taper, flatten, inflate, stretch, lattice."))?;
    let mut p = Params { kind, ..Params::default() };
    if let Some(axis) = word(args, "axis") {
        p.axis = ["x", "y", "z"].iter().position(|a| *a == axis).ok_or("axis is x, y or z.")?;
    }
    if let Some(anchor) = word(args, "anchor") {
        p.anchor = Anchor::ALL.into_iter().find(|a| a.name().to_lowercase() == anchor).ok_or("anchor is start, middle or end.")?;
    }
    let percent = |v: f32| v / 100.0;
    p.angle = number(args, "angle").unwrap_or(p.angle);
    p.direction = number(args, "direction").unwrap_or(p.direction);
    p.scales[0] = number(args, "start_size").map_or(p.scales[0], percent);
    p.scales[1] = number(args, "end_size").map_or(p.scales[1], percent);
    p.stretch = number(args, "length").map_or(p.stretch, percent);
    p.keep_volume = args.get("keep_volume").and_then(Value::as_bool).unwrap_or(p.keep_volume);
    p.softness = number(args, "softness").map_or(p.softness, percent);
    if let Some(amount) = number(args, "amount") {
        match kind {
            Kind::Flatten => p.flatten = percent(amount),
            Kind::Inflate => p.inflate = percent(amount),
            _ => return Err(format!("amount is for flatten and inflate, not {}.", kind.name().to_lowercase())),
        }
    }
    if let Some(range) = numbers::<2>(args, "range")? {
        p.range = range.map(percent);
    }
    if let Some(tip) = numbers::<3>(args, "tip")? {
        p.tip = tip.map(percent);
    }
    if let Some(count) = numbers::<3>(args, "lattice_points")? {
        p.lattice = Lattice::default().with_count(count.map(|n| n.round().max(0.0) as usize));
    }
    for moved in args.get("lattice_moves").and_then(Value::as_array).into_iter().flatten() {
        let point = numbers::<3>(moved, "point")?.ok_or("Each of lattice_moves needs a point: [x, y, z], counting from 1.")?;
        let by = numbers::<3>(moved, "by")?.ok_or("Each of lattice_moves needs by: [x, y, z] in percent of the cage.")?;
        let ijk = point.map(|n| (n.round() as i64 - 1).max(0) as usize);
        if (0..3).any(|c| ijk[c] >= p.lattice.count[c]) {
            return Err(format!("The cage has {:?} points along X, Y and Z; there is no point {point:?}.", p.lattice.count));
        }
        let index = p.lattice.index(ijk);
        p.lattice.moved[index] = by.map(percent);
    }
    let p = p.sanitized();
    debug_assert!(p.lattice.count.iter().all(|n| (lattice::MIN_POINTS..=lattice::MAX_POINTS).contains(n)));
    Ok(p)
}

impl Running {
    /// The `deform` tool.
    pub(in crate::app) fn deform_call(&mut self, args: &Value) -> Result<String, String> {
        let params = params_from(args)?;
        if !args.get("preview").and_then(Value::as_bool).unwrap_or(false) {
            // Through the card's own command, as the app's buttons go.
            self.transform.deform.ran = None;
            self.act(Action::Deform(Ask::Run(Box::new(params))));
            return self.transform.deform.ran.take().unwrap_or_else(|| Err("The deformation could not be made now.".into()));
        }
        // Left on the model, with the card and its handles up, for the person to judge.
        self.deform_commit();
        if self.doc.tool != Tool::Transform {
            self.set_tool(Tool::Transform);
        }
        let card = &mut self.doc.opts.deform;
        card.on = true;
        card.params = params.clone();
        Ok(format!(
            "The {} is showing on the model, not yet kept. Apply (Enter) keeps it as one undo step; Cancel (Esc) puts the model back. \
             Any other command applies it first.",
            params.kind.name().to_lowercase()
        ))
    }
}
