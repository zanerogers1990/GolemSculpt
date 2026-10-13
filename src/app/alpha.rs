//! Brush alphas, the app's side: bringing a picture of the user's own into
//! the alpha folder, and looking through that folder again. The alphas
//! themselves, and what a stroke does with one, are in `sculpt/alpha.rs`.

use std::path::Path;

use super::Running;
use crate::sculpt::{Brush, alpha};
use crate::ui::alpha_preview::Preview;

impl Running {
    /// The picture to show inside the brush's ring, if `brush` (the brush a
    /// stroke begun now would use) prints one and the pointer is over the
    /// model: so a print can be aimed before it is made. Its top points up
    /// the screen or, once a stroke with Turn With Stroke is moving, the way
    /// the stroke is going, and then it is turned by the alpha's Rotation,
    /// as the print is. Nothing while a print is being dragged out: that
    /// print is on the surface to be seen.
    pub(super) fn alpha_under_cursor(&self, brush: Brush) -> Option<Preview> {
        let sculptor = &self.doc.sculptor;
        let s = &sculptor.settings;
        if s.mask_mode || !brush.takes_alpha() {
            return None;
        }
        let a = &s.alpha[brush.index()];
        let image = a.image?;
        let stroking = sculptor.is_active();
        if (stroking && a.drag) || (!stroking && self.input.hover.is_none()) {
            return None;
        }
        // On the screen x goes across and y down; the view's own directions
        // are the model's, as the stroke's heading is.
        let on_screen = |v: glam::Vec3| egui::vec2(v.dot(self.local.right), -v.dot(self.local.up));
        let heading = sculptor.stroke_heading().filter(|_| a.follow).map(on_screen).filter(|h| h.length() > 1e-6);
        let top = heading.map_or(egui::vec2(0.0, -1.0), |h| h.normalized());
        // Right of a top that points up the screen is across it.
        let across = egui::vec2(-top.y, top.x);
        let (sin, cos) = a.rotation.to_radians().sin_cos();
        Some(Preview {
            image,
            scale: a.scale.clamp(*alpha::AlphaSettings::SCALE.start(), *alpha::AlphaSettings::SCALE.end()),
            right: across * cos + top * sin,
            up: top * cos - across * sin,
            invert: a.invert,
        })
    }

    /// Asks for a picture and imports it. This opens a file box, so nothing
    /// scripted may call it; [`Self::import_alpha`] is the part underneath.
    pub(super) fn pick_alpha(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Import Alpha")
            .add_filter("Alpha pictures", &["png", "jpg", "jpeg"])
            .pick_file();
        if let Some(path) = picked {
            self.import_alpha(&path);
        }
    }

    /// Copies the picture at `path` into the alpha folder, gives it to the
    /// brush in hand if that brush takes an alpha, and says so.
    pub(super) fn import_alpha(&mut self, path: &Path) {
        match alpha::import(path) {
            Ok(name) => {
                let s = &mut self.doc.sculptor.settings;
                let brush = s.brush;
                let message = if brush.takes_alpha() {
                    s.alpha[brush.index()].image = alpha::find(&name);
                    format!("Imported alpha {name}, now on the {} brush", brush.name())
                } else {
                    format!("Imported alpha {name}")
                };
                self.status(message, true);
            }
            Err(e) => self.status(e, false),
        }
    }

    /// Lists the alpha folder afresh. A brush keeps the picture it has, even
    /// one whose file is gone, until another is picked.
    pub(super) fn rescan_alphas(&mut self) {
        let message = alpha::rescan();
        self.status(message, true);
    }
}
