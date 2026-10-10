//! The left panel of a region tool while it is up: the shape to draw with,
//! what a drag will do, and the last result.

use egui::{Color32, vec2};
use sculpt_toolkit::Ctx;
use sculpt_toolkit::theme::{self, ACCENT, ButtonStyle};

use crate::action::Action;
use crate::gesture::Shape;
use crate::tool::Region;
use crate::{hide, mask};

/// What a whole-object button does.
type Whole = fn(&mut Ctx) -> Result<String, String>;

const SHORTCUTS: Color32 = Color32::from_rgb(230, 204, 102);

/// The card's title. Each tool's differs, since a card remembers whether it
/// is open by its title.
fn title(tool: &Region) -> &'static str {
    match tool.action {
        Action::Mask => "Mask Region",
        Action::Hide => "Hide Region",
        Action::Trim => "Trim",
        Action::Split => "Split",
    }
}

pub fn panel(tool: &mut Region, ui: &mut egui::Ui, cx: &mut Ctx) {
    let tint = tool.action.tint();
    let summary = format!("{} {}", tool.shape.name(), tool.action.name());
    theme::category(ui, title(tool), tint, &summary, true, |ui| {
        theme::heading(ui, "Shape");
        shape_row(tool, ui, cx);
        theme::hint(ui, tool.action.prompt(tool.shape));

        if tool.action.cuts() && cx.mirror.iter().any(|&on| on) {
            theme::line(ui, "Mirror is on: the cut repeats on the other side.", theme::HINT);
        }
        match (&tool.job, &tool.last) {
            (Some(job), _) => {
                let doing = job.progress();
                let doing = if doing.is_empty() { "Working" } else { doing.as_str() };
                theme::line(ui, &format!("{doing}..."), theme::OK);
            }
            (None, Some((text, ok))) => theme::line(ui, text, if *ok { theme::OK } else { theme::ERROR }),
            (None, None) => {}
        }
        match tool.action {
            Action::Mask => {
                let clear = "Unmasks the whole object. A click in the viewport does the same.";
                let invert = "Swaps masked and free: mask what to keep, invert, work on the rest.";
                let buttons = [("Clear Mask", clear, mask::clear as Whole), ("Invert Mask", invert, mask::invert as Whole)];
                whole_object_row(tool, ui, cx, buttons);
            }
            Action::Hide => {
                let show = "Shows every hidden face again. A click in the viewport does the same.";
                let invert = "Swaps hidden and showing: hide what is in the way, invert, and only it is left.";
                let buttons = [("Show All", show, hide::show_all as Whole), ("Invert Visible", invert, hide::invert as Whole)];
                whole_object_row(tool, ui, cx, buttons);
            }
            Action::Trim | Action::Split => {}
        }

        let key = tool.hotkey().map(|key| key.name()).unwrap_or("Esc");
        let style = ButtonStyle { badge: key, ..Default::default() };
        let r = theme::button(ui, ui.available_width(), "Back to Sculpt", style);
        if theme::tip(r, cx.tips, "Put the tool away and sculpt. A drag on the background orbits again.").clicked() {
            cx.finish();
        }
    });

    theme::category(ui, &format!("{} Shortcuts", tool.name()), SHORTCUTS, "", false, |ui| {
        theme::shortcuts(ui, &shortcuts(tool));
    });
}

fn shape_row(tool: &mut Region, ui: &mut egui::Ui, cx: &mut Ctx) {
    let shapes = tool.action.shapes();
    let help = |shape: Shape| match shape {
        Shape::Box => "Drag from one corner to the opposite one.",
        Shape::Lasso => "Draw freehand round what you want; the outline closes itself back to where it began.",
        Shape::Line => "Drag a straight line; it carries on past both ends. Shift turns it in steps of 15 degrees.",
    };
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, shapes.len(), 6.0);
        for &shape in shapes {
            let active = (tool.shape == shape).then_some(ACCENT);
            let r = theme::tile(ui, vec2(w, 46.0), cx.icon(tool.action.icon(shape)), shape.name(), "", active);
            if theme::tip(r, cx.tips, help(shape)).clicked() {
                tool.set_shape(cx, shape);
            }
        }
    });
}

/// Two buttons that act on the whole object: the counterparts of the
/// gesture (Clear Mask | Invert Mask, Show All | Invert Visible).
fn whole_object_row(tool: &mut Region, ui: &mut egui::Ui, cx: &mut Ctx, buttons: [(&str, &str, Whole); 2]) {
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, buttons.len(), 6.0);
        for (label, help, act) in buttons {
            let r = theme::button(ui, w, label, ButtonStyle::default());
            if theme::tip(r, cx.tips, help).clicked() {
                let result = act(cx);
                tool.report(cx, result);
            }
        }
    });
}

fn shortcuts(tool: &Region) -> Vec<(&'static str, &'static str)> {
    let mut rows: Vec<(&'static str, &'static str)> = vec![("Drag", "Draw the shape")];
    match tool.action {
        Action::Mask | Action::Hide => {
            let (add, remove, reset) = if tool.action == Action::Mask {
                ("Mask everything else", "Unmask instead", "Clear the mask")
            } else {
                ("Hide everything else", "Show instead", "Show everything")
            };
            rows.push(("Ctrl, or right drag", remove));
            rows.push(("Shift", add));
            rows.push(("Click", reset));
        }
        Action::Trim => {
            rows.push(("Ctrl, or right drag", "Keep the covered part"));
            rows.push(("Shift (box, lasso)", "Keep the covered part"));
            rows.push(("Shift (line)", "Turn in 15 degree steps"));
        }
        Action::Split => rows.push(("Shift (line)", "Turn in 15 degree steps")),
    }
    rows.push(("Esc", "Cancel the drag, else leave"));
    match tool.hotkey() {
        Some(sculpt_toolkit::Key::N) => rows.push(("N", "Next shape, then leave")),
        Some(sculpt_toolkit::Key::K) => rows.push(("K", "Next shape, then leave")),
        _ => {}
    }
    rows.push(("Space (hold)", "Shape wheel"));
    rows.push(("1 - 9", "Back to sculpting"));
    rows.push(("Alt + drag", "Orbit"));
    rows.push(("Z", "Undo"));
    rows
}
