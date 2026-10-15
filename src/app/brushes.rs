//! The brush library, the app's side: reading the brush folder at start-up
//! and keeping it up to date, and doing what the panel asks of the library
//! (duplicate, rename, delete, Reset, Import Brush and Export Brush). The
//! library itself is `sculpt/library.rs`, and the files are `brush_files.rs`.
//!
//! The two file boxes are opened here and nowhere else, each over a routine
//! that takes a path ([`Running::import_brush`], [`Running::export_brush`]),
//! which is what tests and the Claude bridge call: nothing scripted may put
//! a file box on the user's screen.

use std::path::Path;

use super::Running;
use crate::brush_files;
use crate::prefs::Prefs;
use crate::sculpt::Brush;
use crate::sculpt::library::{Ask, Library, Origin};

impl Running {
    /// Puts back the brushes a settings file remembers, in the one order
    /// that works. The file's brush settings are the fourteen base brushes',
    /// so the base brushes are made the live ones before those settings are
    /// laid in the working sets (at start-up they already are; a settings
    /// file read later must not land on whatever brush is in hand). Then
    /// the brushes in the brush folder join the library, the first time
    /// only, and last the brush that was in hand is taken up again.
    pub(super) fn brushes_restore(&mut self, kept: &Prefs) {
        let sculptor = &mut self.doc.sculptor;
        let (library, s) = (&mut sculptor.library, &mut sculptor.settings);
        library.lay_base(s);
        kept.brush.apply(s);
        let notes = self.files.brushes.load(library, s);
        kept.library.apply(library, s);
        if !notes.is_empty() {
            self.status(notes.join(" "), false);
        }
    }

    /// Writes the brushes that have changed to the brush folder. Called
    /// with the settings, now and then between strokes and as the app closes.
    pub(super) fn brushes_keep(&mut self) {
        let sculptor = &self.doc.sculptor;
        self.files.brushes.sync(&sculptor.library, &sculptor.settings);
    }

    /// Does what the panel asked of the library and says what happened.
    pub(super) fn brush_ask(&mut self, ask: Ask) {
        let said = match ask {
            Ask::Import => {
                let Some(path) = pick_to_import(self.files.brushes.folder()) else { return };
                self.import_brush(&path)
            }
            Ask::Export(i) => {
                let Some(entry) = self.doc.sculptor.library.entries().get(i) else { return };
                let Some(path) = pick_to_export(&entry.name) else { return };
                self.export_brush(i, &path)
            }
            other => self.brush_edit(other),
        };
        match said {
            Ok(message) if message.is_empty() => {}
            Ok(message) => self.status(message, true),
            Err(why) => self.status(why, false),
        }
    }

    /// Every ask that needs no file box: what the Claude bridge may ask too.
    /// Gives back what to say, which is nothing for the plain picks.
    pub(super) fn brush_edit(&mut self, ask: Ask) -> Result<String, String> {
        let sculptor = &mut self.doc.sculptor;
        let (library, s) = (&mut sculptor.library, &mut sculptor.settings);
        let name = |library: &Library, i: usize| {
            library.entries().get(i).map(|e| e.name.clone()).ok_or("There is no such brush.")
        };
        match ask {
            Ask::Select(i) => {
                name(library, i)?;
                library.select(i, s);
                Ok(String::new())
            }
            Ask::Duplicate(i) => {
                let from = name(library, i)?;
                let new = library.duplicate(i, s);
                Ok(format!("{} is a copy of {from}, and yours to change.", library.entries()[new].name))
            }
            Ask::Rename(i, to) => {
                let from = name(library, i)?;
                library.rename(i, &to)?;
                Ok(format!("{from} is now called {}.", library.entries()[i].name))
            }
            Ask::Delete(i) => self.delete_brush(i),
            Ask::Reset(i) => {
                let entry = library.entries().get(i).ok_or("There is no such brush.")?;
                let (name, how) = (entry.name.clone(), if entry.origin == Origin::User { "it was made" } else { "it shipped" });
                library.reset(i, s);
                Ok(format!("{name} is back as {how}."))
            }
            Ask::Pin(i, pinned) => {
                let name = name(library, i)?;
                let was = library.entries()[i].pinned;
                library.set_pinned(i, pinned);
                Ok(match (was, pinned) {
                    (false, true) => format!("{name} is on the Space wheel."),
                    (true, false) => format!("{name} is off the Space wheel."),
                    _ => String::new(),
                })
            }
            Ask::ResetWheel => {
                library.reset_pins();
                Ok("The Space wheel has the brushes it started with.".to_owned())
            }
            Ask::ShiftUses(i) => {
                let name = name(library, i)?;
                if !library.set_shift(i, s) {
                    return Err(format!("{name} is not a smoothing brush, so Shift cannot use it."));
                }
                Ok(format!("Shift now smooths with {name}."))
            }
            Ask::RestoreShipped => {
                let back = library.removed().len();
                library.restore_shipped();
                Ok(match back {
                    0 => "Every brush that came with the app is already here.".to_owned(),
                    1 => "1 brush that came with the app is back.".to_owned(),
                    n => format!("{n} brushes that came with the app are back."),
                })
            }
            Ask::Import | Ask::Export(_) => Err("That needs a file to be named.".into()),
        }
    }

    /// Deletes a brush. One of the user's own, or a shipped one they had
    /// changed, is kept as a file in the brush folder's `deleted` folder.
    fn delete_brush(&mut self, i: usize) -> Result<String, String> {
        let sculptor = &mut self.doc.sculptor;
        let entry = sculptor.library.entries().get(i).ok_or("There is no such brush.")?;
        if entry.origin == Origin::Base {
            return Err(format!("{} is one of the fourteen base brushes, which stay.", entry.name));
        }
        let (name, shipped) = (entry.name.clone(), entry.origin == Origin::Shipped);
        let aside = self.files.brushes.set_aside(&sculptor.library, i, &sculptor.settings);
        sculptor.library.delete(i, &mut sculptor.settings)?;
        let kept = match (aside, shipped) {
            (Some(_), _) => " A copy is in the brushes folder, under deleted; Import Brush brings it back.",
            (None, true) => " Restore Shipped Brushes brings it back.",
            (None, false) => "",
        };
        Ok(format!("Deleted the brush {name}.{kept}"))
    }

    /// Brings the brush file at `path` into the library as one of the
    /// user's own, and takes it in hand.
    pub(super) fn import_brush(&mut self, path: &Path) -> Result<String, String> {
        let sculptor = &mut self.doc.sculptor;
        brush_files::import(path, &mut sculptor.library, &mut sculptor.settings).map(|(_, said)| said)
    }

    /// Writes brush `i` to `path` as it is now.
    pub(super) fn export_brush(&mut self, i: usize, path: &Path) -> Result<String, String> {
        let sculptor = &self.doc.sculptor;
        brush_files::export(path, &sculptor.library, i, &sculptor.settings)
    }
}

/// Asks which brush file to import, starting in the folder deleted brushes
/// are kept under if there is one. A file box: never in a scripted run.
impl Running {
    /// Whether a stroke about to begin has to be refused because it is Erase
    /// Layer's and there is no layer for it to rub out; says what to do if
    /// so. `shift` is whether Shift is held, which makes the stroke a Smooth.
    pub(super) fn erase_refuses_stroke(&mut self, shift: bool) -> bool {
        match self.doc.erase_refusal(shift) {
            Some(why) => {
                self.status(why, false);
                true
            }
            None => false,
        }
    }
}

impl crate::doc::Doc {
    /// Why a stroke begun now on the active object would do nothing, if it
    /// is Erase Layer's and no layer is recording (`sculpt/erase.rs`). A
    /// stroke that came to nothing would look like a brush that is broken.
    pub(crate) fn erase_refusal(&self, smooth_held: bool) -> Option<String> {
        let erasing = !self.sculptor.settings.mask_mode && self.sculptor.effective_brush(smooth_held) == Brush::EraseLayer;
        if erasing { crate::sculpt::erase::refusal(&self.mesh) } else { None }
    }
}

fn pick_to_import(brushes: Option<&Path>) -> Option<std::path::PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Import Brush").add_filter("Brushes", &["json"]);
    if let Some(folder) = brushes.filter(|folder| folder.is_dir()) {
        dialog = dialog.set_directory(folder);
    }
    dialog.pick_file()
}

/// Asks where to write the brush called `name`. A file box: never in a
/// scripted run.
fn pick_to_export(name: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .set_title("Export Brush")
        .add_filter("Brushes", &["json"])
        .set_file_name(format!("{}.json", brush_files::file_stem(name)))
        .save_file()
}
