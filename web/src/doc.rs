//! The few settings of the desktop's `doc::Options` that desktop files
//! compiled here reach for: the Base Colour the paint export lays paint over
//! (`io/painted.rs`, for FBX), and the tooltip language the first-run
//! question sets (`ui/first_language.rs`). The desktop's defaults.

pub struct Options {
    pub base_color: [f32; 3],
    pub tooltip_language: &'static str,
}

impl Default for Options {
    fn default() -> Options {
        Options { base_color: [0.65, 0.65, 0.68], tooltip_language: "en" }
    }
}
