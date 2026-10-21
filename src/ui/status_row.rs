//! One quiet line over the bottom bar's switches that says what every stroke
//! is working under, so it can be seen without going to the Mesh tab: the
//! brush in hand, the selected object's mirror axes and radial symmetry,
//! whether it has a mask, and what is hidden. Only what is on is named; a
//! plain scene shows the brush alone. It is for reading: the cards and keys
//! change these.

use egui::{RichText, Ui};

use super::{fmt_count, rebuild_notes, theme};
use crate::doc::Doc;
use crate::scene::Symmetry;

/// The line's words, item by item: the brush in hand (or mask painting),
/// the selected object's symmetry, whether it has a mask, how many of its
/// faces and how many objects are hidden.
fn words(brush: String, symmetry: Symmetry, masked: bool, faces: usize, objects: usize) -> Vec<String> {
    let mut items = vec![brush];
    let axes: Vec<&str> = ["X", "Y", "Z"].into_iter().zip(symmetry.mirror).filter(|(_, on)| *on).map(|(a, _)| a).collect();
    if !axes.is_empty() {
        items.push(format!("Mirror {}", axes.join(" ")));
    }
    if symmetry.radial.on {
        items.push(format!("Radial {}", symmetry.radial.count));
    }
    if masked {
        items.push("Masked".to_owned());
    }
    if faces > 0 {
        items.push(format!("{} faces hidden", fmt_count(faces)));
    }
    if objects > 0 {
        items.push(if objects == 1 { "1 object hidden".to_owned() } else { format!("{objects} objects hidden") });
    }
    items
}

/// The line, in the bottom bar's area.
pub(super) fn show(ui: &mut Ui, doc: &Doc) {
    let (sculptor, s) = (&doc.sculptor, &doc.sculptor.settings);
    let brush = if s.mask_mode {
        "Mask painting".to_owned()
    } else {
        sculptor.library.entries()[sculptor.library.selected(s)].name.clone()
    };
    let masked = rebuild_notes::masked(ui, &doc.mesh, doc.history.edits);
    let faces = doc.mesh.hidden_triangle_count();
    let objects = doc.scene.objects.iter().filter(|o| !o.visible).count();
    let text = words(brush, doc.scene.active_symmetry(), masked, faces, objects).join("   \u{b7}   ");
    ui.label(RichText::new(text).font(theme::font(11.5)).color(theme::HINT()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_is_on_is_named() {
        let plain = Symmetry::default();
        assert_eq!(words("Clay".into(), plain, false, 0, 0), ["Clay"], "a plain scene: the brush alone");
        let mut symmetry = plain;
        symmetry.mirror = [true, false, true];
        symmetry.radial.on = true;
        symmetry.radial.count = 6;
        assert_eq!(
            words("Dam Standard".into(), symmetry, true, 1500, 2),
            ["Dam Standard", "Mirror X Z", "Radial 6", "Masked", "2k faces hidden", "2 objects hidden"]
        );
        assert_eq!(words("Clay".into(), plain, false, 0, 1), ["Clay", "1 object hidden"]);
    }
}
