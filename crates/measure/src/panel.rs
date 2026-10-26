//! The tool's side of the interface: the ruler readout in the left panel
//! while the tool is up, and the Measure & Size card in the Scene panel,
//! which is there whatever tool is up.

use egui::Color32;
use glam::Quat;
use sculpt_geom::{Aabb, LengthUnit, units};
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};
use sculpt_toolkit::{Command, Ctx, MeshId, ObjectId, Placement};

use crate::rulers::{Rulers, axis_breakdown};
use crate::size;

const ACCENT_MEASURE: Color32 = Color32::from_rgb(255, 209, 64);
const CARD_TITLE: &str = "Measure & Size";
const AXIS_NAMES: [&str; 3] = ["Width (X)", "Height (Y)", "Depth (Z)"];
/// The exact size of a turned object is a pass over every vertex, and
/// nothing says when a brush has moved them, so it is taken again this often
/// rather than every frame.
const REFRESH_SECONDS: f64 = 0.25;

pub fn other_unit(unit: LengthUnit) -> LengthUnit {
    match unit {
        LengthUnit::Millimetres => LengthUnit::Inches,
        LengthUnit::Inches => LengthUnit::Millimetres,
    }
}

pub fn unit_name(unit: LengthUnit) -> &'static str {
    match unit {
        LengthUnit::Millimetres => "Millimetres",
        LengthUnit::Inches => "Inches",
    }
}

/// Millimetres | Inches.
fn unit_row(ui: &mut egui::Ui, cx: &mut Ctx) {
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 2, 6.0);
        for unit in LengthUnit::ALL {
            let style = ButtonStyle { active: (cx.unit == unit).then_some(ACCENT()), ..Default::default() };
            let help = format!("Show and type every length in {} (U toggles while measuring).", unit_name(unit).to_lowercase());
            if theme::tip(theme::button(ui, w, unit_name(unit), style), cx.tips, &help).clicked() {
                cx.set_unit(unit);
            }
        }
    });
}

/// Text in several colours on one wrapped line.
fn coloured_line(ui: &mut egui::Ui, parts: &[(String, Color32)], size: f32) {
    let mut job = egui::text::LayoutJob::default();
    for (text, color) in parts {
        job.append(text, 0.0, egui::TextFormat::simple(theme::font(size), *color));
    }
    ui.add(egui::Label::new(job).wrap());
}

/// The left panel while the tool is up.
pub fn panel(rulers: &mut Rulers, ui: &mut egui::Ui, cx: &mut Ctx) {
    let count = rulers.len();
    let summary = match count {
        0 => String::new(),
        1 => "1 ruler".to_owned(),
        n => format!("{n} rulers"),
    };
    theme::category(ui, "Measure", ACCENT_MEASURE, &summary, true, |ui| {
        theme::heading(ui, "Units");
        unit_row(ui, cx);

        theme::heading(ui, "Rulers");
        match rulers.selected() {
            Some(ruler) => {
                let length = units::format(ruler.length(), cx.unit, true);
                coloured_line(ui, &[("Ruler: ".to_owned(), theme::TEXT()), (length, Color32::WHITE)], 13.0);
                coloured_line(ui, &axis_breakdown(ruler.delta(), cx.unit), 11.0);
            }
            None if count > 0 => theme::line(ui, &format!("{summary}. Click one for its detail."), theme::HINT()),
            None => theme::line(ui, "No rulers yet. Drag across the model to lay one.", theme::HINT()),
        }
        ui.horizontal(|ui| {
            let w = theme::row_width(ui, 2, 6.0);
            ui.add_enabled_ui(rulers.selected().is_some(), |ui| {
                let style = ButtonStyle { badge: "Del", ..Default::default() };
                let r = theme::button(ui, w, "Delete", style);
                if theme::tip(r, cx.tips, "Removes the selected ruler (Delete, or right-click a ruler).").clicked() {
                    rulers.delete_selected();
                }
            });
            ui.add_enabled_ui(count > 0, |ui| {
                let r = theme::button(ui, w, "Clear All", ButtonStyle::default());
                if theme::tip(r, cx.tips, "Removes every ruler.").clicked() {
                    rulers.clear();
                }
            });
        });
        theme::hint(ui, "Ends snap to the surface. Rulers are a scaffold: they are not saved and not on the undo history.");
        theme::hint(ui, "The selected object's real size is under Measure & Size in the Scene panel.");
        let style = ButtonStyle { badge: "L", ..Default::default() };
        let r = theme::button(ui, ui.available_width(), "Back to Sculpt", style);
        if theme::tip(r, cx.tips, "Put the rulers away and sculpt (L). They are still there next time.").clicked() {
            cx.finish();
        }
    });

    theme::category(ui, "Measure Shortcuts", Color32::from_rgb(230, 204, 102), "", false, |ui| {
        theme::shortcuts(
            ui,
            &[
                ("Drag on the model", "New ruler"),
                ("Click, then click", "New ruler, end by end"),
                ("Drag an end", "Re-measure"),
                ("Click a ruler", "Select it"),
                ("Right-click a ruler", "Delete it"),
                ("Delete", "Delete the selected"),
                ("Shift + drag", "Lock to an axis"),
                ("Ctrl + drag", "Ignore the surface"),
                ("U", "mm / in"),
                ("Esc", "Cancel, deselect, leave"),
                ("L", "Measure on / off"),
                ("Drag on background", "Orbit"),
            ],
        );
    });
}

/// The selected object as last measured.
#[derive(Clone, Copy)]
struct Measured {
    object: ObjectId,
    mesh: MeshId,
    placement: Placement,
    vertices: usize,
    bounds: Aabb,
    at: f64,
}

/// The Measure & Size card: the unit, the door to the tool, and the selected
/// object's real size as three editable fields.
pub struct SizeCard {
    keep_proportions: bool,
    /// What is in each box, and what was last put there by a measurement.
    fields: [String; 3],
    shown: [String; 3],
    measured: Option<Measured>,
    reveal: bool,
}

impl Default for SizeCard {
    fn default() -> SizeCard {
        SizeCard { keep_proportions: true, fields: Default::default(), shown: Default::default(), measured: None, reveal: false }
    }
}

impl SizeCard {
    /// Opens the card the next time it is drawn.
    pub fn reveal(&mut self) {
        self.reveal = true;
    }

    /// The selected object's exact world box, re-measured when it is out of
    /// date.
    fn measure(&mut self, cx: &Ctx, force: bool) -> Option<Measured> {
        let scene = cx.scene();
        let info = scene.active_info()?;
        if info.vertices == 0 {
            self.measured = None;
            return None;
        }
        let fresh = self.measured.filter(|m| {
            !force
                && m.object == info.id
                && m.mesh == info.mesh
                && m.placement == info.placement
                && m.vertices == info.vertices
                && cx.time - m.at < REFRESH_SECONDS
        });
        if fresh.is_none() {
            // The quick box is already exact for an object that is not
            // turned, which is most of them.
            let bounds = if info.placement.rotation.abs_diff_eq(Quat::IDENTITY, 1e-6) {
                scene.bounds(info.id)
            } else {
                size::exact_bounds(scene.mesh(info.id)?.positions, &info.matrix())
            };
            self.measured = Some(Measured {
                object: info.id,
                mesh: info.mesh,
                placement: info.placement,
                vertices: info.vertices,
                bounds,
                at: cx.time,
            });
        }
        self.measured.filter(|m| !m.bounds.is_empty())
    }

    /// For the card's header: the selected object's longest dimension as
    /// last measured, or just the unit. Never measures by itself; the header
    /// is drawn whether or not the card is open.
    fn summary(&self, cx: &Ctx) -> String {
        match self.measured.filter(|m| m.object == cx.scene().active() && !m.bounds.is_empty()) {
            Some(m) => units::format(m.bounds.size().max_element(), cx.unit, true),
            None => cx.unit.suffix().to_owned(),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        if std::mem::take(&mut self.reveal) {
            theme::open_category(ui, CARD_TITLE);
        }
        let summary = self.summary(cx);
        theme::category(ui, CARD_TITLE, ACCENT_MEASURE, &summary, false, |ui| {
            theme::heading(ui, "Units");
            unit_row(ui, cx);

            theme::heading(ui, "Measure");
            let (label, active) = if cx.active { ("Done", Some(ACCENT())) } else { ("Measure", None) };
            let style = ButtonStyle { icon: cx.icon("tool_measure"), active, badge: "L", ..Default::default() };
            let r = theme::button(ui, ui.available_width(), label, style);
            let help = "Lay rulers on the model: drag from one point to another, then drag either end to re-measure. Ends snap to the surface. L toggles the tool.";
            if theme::tip(r, cx.tips, help).clicked() {
                if cx.active { cx.finish() } else { cx.start() }
            }

            theme::heading(ui, "Selected Object Size");
            self.size_fields(ui, cx);
        });
    }

    fn size_fields(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let measured = self.measure(cx, false);
        let name = cx.scene().active_info().map(|info| info.name.to_owned());
        let Some((measured, name)) = measured.zip(name) else {
            theme::line(ui, "Nothing to measure.", theme::HINT());
            return;
        };
        theme::line(ui, &name, theme::HINT());

        let unit = cx.unit;
        let size = measured.bounds.size();
        let mut submitted = None;
        for axis in 0..3 {
            self.shown[axis] = units::format_number(units::to_unit(size[axis], unit), unit);
            let label = format!("{}  {}", AXIS_NAMES[axis], unit.suffix());
            let r = theme::labelled_field(ui, &label, &mut self.fields[axis]);
            if r.lost_focus() {
                // Enter or a click elsewhere takes the size, Escape drops it.
                let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if !escaped && self.fields[axis].trim() != self.shown[axis] {
                    submitted = Some(axis);
                }
            }
            // Left alone while it is being typed in, or the refresh would
            // overwrite a half-typed value.
            if !r.has_focus() && submitted != Some(axis) {
                self.fields[axis].clone_from(&self.shown[axis]);
            }
            let help = "The selected object's real size along this axis. Type a new size and press Enter to resize it: a unit (\"3in\", \"76mm\", \"1 1/4in\") overrides the display unit. Undo (Ctrl+Z) puts it back.";
            theme::tip(r, cx.tips, help);
        }
        let label = if self.keep_proportions { "Keep Proportions" } else { "Keep Proportions (Off)" };
        let r = theme::checkbox(ui, &mut self.keep_proportions, label, ACCENT());
        theme::tip(
            r,
            cx.tips,
            "On, a new size scales the whole object evenly. Off, it stretches the object along that one axis only.",
        );
        theme::hint(ui, "In a slicer: X = width, Y = depth, Z = height. Export STL writes this size in mm.");

        if let Some(axis) = submitted {
            let text = std::mem::take(&mut self.fields[axis]);
            self.submit(cx, measured.object, &name, axis, text.trim());
        }
    }

    /// A size typed into one of the boxes: resizes the object as one undo
    /// step, or says why not.
    fn submit(&mut self, cx: &mut Ctx, object: ObjectId, name: &str, axis: usize, text: &str) {
        let Some(target) = units::parse(text, cx.unit) else {
            cx.status(format!("\"{text}\" is not a size - try 50, 50mm or 2in."), false);
            return;
        };
        match resize(cx, object, axis, target, self.keep_proportions) {
            Ok(()) => {
                let axis_name = AXIS_NAMES[axis].to_lowercase();
                cx.status(format!("{name}: {axis_name} set to {}. Undo (Ctrl+Z) puts it back.", units::format(target, cx.unit, true)), true);
                // Re-measured right away rather than on the timer, so the
                // other two boxes update on the same frame.
                self.measure(cx, true);
            }
            Err(e) => cx.status(e, false),
        }
    }
}

/// Sizes `object` so its world box measures `target` world units along world
/// `axis`. One undo step.
pub fn resize(cx: &mut Ctx, object: ObjectId, axis: usize, target: f32, uniform: bool) -> Result<(), String> {
    let scene = cx.scene();
    let (Some(info), Some(mesh)) = (scene.info(object), scene.mesh(object)) else {
        return Err("Select an object to size it.".into());
    };
    let placement = info.placement;
    let scale = size::resized_scale(mesh.positions, placement, axis, target, uniform)?;
    if scale != placement.scale {
        cx.commit("Set Size", vec![Command::Place { object, placement: Placement { scale, ..placement } }]);
    }
    Ok(())
}
