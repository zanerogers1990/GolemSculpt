//! egui's default fonts for the web build, with two of the four left out to
//! make the download smaller (about 440 KB once compressed, a tenth of it).
//!
//! web/Cargo.toml puts this crate in place of the real `epaint_default_fonts`
//! (a `[patch.crates-io]` entry), so egui's own font set-up is unchanged and
//! every font family it and the tool kit make still exists. What changes:
//!
//! - Hack, egui's fixed-width font, is drawn with Ubuntu Light instead. The
//!   interface never asks for fixed-width text.
//! - Noto Emoji is drawn with the emoji icon font instead. That font has the
//!   symbols egui itself draws (the menu arrows, ticks and crosses); Noto
//!   only added pictures such as faces and animals, which no label uses.
//!
//! The two kept fonts are byte for byte egui's 0.36.2 files, with their
//! licences beside them in `fonts/`.

/// Ubuntu Light stands in for Hack.
pub const HACK_REGULAR: &[u8] = UBUNTU_LIGHT;

/// The emoji icon font stands in for Noto Emoji.
pub const NOTO_EMOJI_REGULAR: &[u8] = EMOJI_ICON;

pub const UBUNTU_LIGHT: &[u8] = include_bytes!("../fonts/Ubuntu-Light.ttf");

pub const EMOJI_ICON: &[u8] = include_bytes!("../fonts/emoji-icon-font.ttf");
