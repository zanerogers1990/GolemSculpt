//! The window's title: the sculpt's name, with a star while it has changes
//! that are not saved, as most programs show it. The File card's header says
//! the same, from `UiState::unsaved`, which is set here.

use std::path::Path;

use super::Running;

/// "dragon head * - GolemSculpt". A scene that has never been saved is
/// "Untitled", and the scene the app starts with, untouched, is the app's
/// name alone.
pub(super) fn title(path: Option<&Path>, unsaved: bool) -> String {
    let name = path.and_then(Path::file_stem).map(|n| n.to_string_lossy().into_owned());
    match (name, unsaved) {
        (None, false) => "GolemSculpt".to_owned(),
        (name, unsaved) => {
            let star = if unsaved { " *" } else { "" };
            format!("{}{star} - GolemSculpt", name.as_deref().unwrap_or("Untitled"))
        }
    }
}

impl Running {
    /// Once a frame, before the interface is drawn. Asking whether anything
    /// is unsaved is cheap (a tenth of a microsecond an object: app/unsaved.rs),
    /// and the title is only set when it changes.
    pub(super) fn title_frame(&mut self) {
        let unsaved = self.unsaved();
        self.ui.unsaved = unsaved;
        let title = title(self.doc.path.as_deref(), unsaved);
        if self.window.title() != title {
            self.window.set_title(&title);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::title;

    #[test]
    fn the_title_names_the_file_and_stars_unsaved_changes() {
        let file = PathBuf::from("D:/Sculpts/dragon head.sclp");
        assert_eq!(title(Some(&file), false), "dragon head - GolemSculpt");
        assert_eq!(title(Some(&file), true), "dragon head * - GolemSculpt");
        assert_eq!(title(None, true), "Untitled * - GolemSculpt");
        assert_eq!(title(None, false), "GolemSculpt");
    }
}
