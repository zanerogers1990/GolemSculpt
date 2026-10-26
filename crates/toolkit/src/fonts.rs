//! Windows' fonts for the letters Segoe UI has not got: Japanese, Chinese
//! and Korean, for tooltips in those languages and for the languages' names
//! (see [`crate::translate`]).
//!
//! Each goes after Segoe UI, as a fallback. egui places a fallback font's
//! letters by centring its line height on Segoe's, which leaves them standing
//! above the line (these fonts have much taller lines), so each is shifted
//! down by what puts its baseline on Segoe's, worked out from the two fonts'
//! own measurements.
//!
//! Each script also has a family of its own (`Language::name_font`): Segoe,
//! then that script's font ahead of the others, so a language's name is drawn
//! in its own font throughout (简 is not in the Japanese font, and would
//! otherwise come from another font than the rest of 简体中文).

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

use crate::translate::{self, Script};

/// Adds the fonts of the scripts the interface needs now (after "segoe",
/// which must be in already), and every script's family.
pub(crate) fn add_scripts(fonts: &mut FontDefinitions) {
    let segoe = fonts.font_data.get("segoe").and_then(|data| line_metrics(&data.font, data.index));
    let mut loaded: Vec<(Script, &str)> = Vec::new();
    for script in translate::scripts() {
        for file in script.fonts() {
            let Ok(bytes) = std::fs::read(format!("C:\\Windows\\Fonts\\{file}")) else { continue };
            let mut data = FontData::from_owned(bytes);
            if let (Some(segoe), Some(own)) = (segoe, line_metrics(&data.font, data.index)) {
                data.tweak.y_offset_factor = baseline_shift(segoe, own);
            }
            fonts.font_data.insert(file.to_string(), Arc::new(data));
            let list = fonts.families.entry(FontFamily::Proportional).or_default();
            list.insert((1 + loaded.len()).min(list.len()), file.to_string());
            loaded.push((script, file));
            break;
        }
    }
    let proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for script in Script::OTHERS {
        let mut list = proportional.clone();
        if let Some(&(_, file)) = loaded.iter().find(|(s, _)| *s == script) {
            list.retain(|name| name != file);
            list.insert(1.min(list.len()), file.to_string());
        }
        fonts.families.insert(FontFamily::Name(script.family().into()), list);
    }
}

/// How far down, as a fraction of the font size, a fallback font's letters
/// go to sit on the main font's baseline. egui puts a fallback letter's
/// baseline at its own ascent plus half the difference of the two line
/// heights; the main font's letters sit at the main font's ascent.
fn baseline_shift(main: [f32; 3], fallback: [f32; 3]) -> f32 {
    let height = |[ascent, descent, gap]: [f32; 3]| ascent - descent + gap;
    main[0] - fallback[0] - 0.5 * (height(main) - height(fallback))
}

/// A font file's ascent, descent (below zero) and line gap, as fractions of
/// its size, read as egui's font reader reads them: the typographic figures
/// where the font asks for them, else the hhea table's, else the typographic
/// ones again. `index` picks the face of a collection (.ttc).
fn line_metrics(file: &[u8], index: u32) -> Option<[f32; 3]> {
    let u16_at = |at: usize| file.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    let u32_at = |at: usize| file.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let face = if file.get(0..4)? == b"ttcf" { u32_at(12 + 4 * index as usize)? as usize } else { 0 };
    let tables = u16_at(face + 4)? as usize;
    let table = |tag: &[u8; 4]| {
        (0..tables).find_map(|i| {
            let record = face + 12 + 16 * i;
            if file.get(record..record + 4)? == tag { u32_at(record + 8).map(|at| at as usize) } else { None }
        })
    };
    let per_em = u16_at(table(b"head")? + 18)? as f32;
    let figure = |at: usize| u16_at(at).map(|v| v as i16 as f32 / per_em);
    let three = |at: usize| Some([figure(at)?, figure(at + 2)?, figure(at + 4)?]);
    let os2 = table(b"OS/2");
    let typo = os2.and_then(|at| three(at + 68));
    // fsSelection's USE_TYPO_METRICS.
    let use_typo = os2.and_then(|at| u16_at(at + 62)).is_some_and(|flags| flags & 0x80 != 0);
    let hhea = table(b"hhea").and_then(|at| three(at + 4));
    match (use_typo, typo, hhea) {
        (true, Some(typo), _) => Some(typo),
        (_, _, Some(hhea)) if hhea != [0.0; 3] => Some(hhea),
        (_, typo, _) => typo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fallback_font_with_the_same_lines_is_not_moved() {
        assert_eq!(baseline_shift([1.0, -0.25, 0.0], [1.0, -0.25, 0.0]), 0.0);
        // Taller above the line by 0.2, as tall below: egui centres the 0.2,
        // putting the baseline 0.1 high, so it goes 0.1 down.
        assert!((baseline_shift([1.0, -0.25, 0.0], [1.2, -0.25, 0.0]) - (-0.1)).abs() < 1e-6);
    }

    #[test]
    fn windows_fonts_measure_as_fonts_do() {
        // Only where Windows' fonts are: elsewhere there is nothing to read.
        let Ok(segoe) = std::fs::read("C:\\Windows\\Fonts\\segoeui.ttf") else { return };
        let [ascent, descent, gap] = line_metrics(&segoe, 0).expect("Segoe UI reads");
        assert!(ascent > 0.7 && ascent < 1.5 && descent < 0.0 && descent > -0.6 && gap >= 0.0, "{ascent} {descent} {gap}");
        for script in Script::OTHERS {
            for file in script.fonts() {
                if let Ok(bytes) = std::fs::read(format!("C:\\Windows\\Fonts\\{file}")) {
                    let [ascent, descent, _] = line_metrics(&bytes, 0).expect(file);
                    assert!(ascent > 0.5 && descent < 0.0, "{file}: {ascent} {descent}");
                }
            }
        }
        assert!(line_metrics(b"not a font", 0).is_none());
    }
}
