//! Tooltips in another language. The interface stays in English; a person
//! whose first language is another can have what each button does explained
//! in theirs (the Interface card's Tooltip Language).
//!
//! A tooltip is looked up by its English. Each language is a file in
//! `translations/` beside this crate, built into the app: pairs of lines,
//! the English and the translation, which `tools/translations.py` keeps in
//! step with the source. A sentence made with `format!` is kept with its
//! placeholders (`{0}`, `{name}`) and matched around them. A tooltip pieced
//! together from several sentences is translated a line, then a sentence, at
//! a time. Anything not in the file is shown in English, as before.
//!
//! The lookup happens only while a tooltip is showing, so a frame with none
//! pays nothing for the language.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use egui::{Response, Ui};

/// What a language's letters need beyond Segoe UI, which has Latin, Greek and
/// Cyrillic: Windows' own font for the language, added when it is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Latin,
    Japanese,
    Chinese,
    Korean,
}

impl Script {
    /// Fonts in `C:\Windows\Fonts` with the script's characters, best first.
    pub(crate) fn fonts(self) -> &'static [&'static str] {
        match self {
            Script::Latin => &[],
            Script::Japanese => &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
            Script::Chinese => &["msyh.ttc", "simsun.ttc"],
            Script::Korean => &["malgun.ttf", "gulim.ttc"],
        }
    }

    pub(crate) const OTHERS: [Script; 3] = [Script::Japanese, Script::Chinese, Script::Korean];

    /// The font family that draws this script in its own font first (see
    /// [`crate::fonts`]).
    pub(crate) fn family(self) -> &'static str {
        match self {
            Script::Latin => "script-latin",
            Script::Japanese => "script-ja",
            Script::Chinese => "script-zh",
            Script::Korean => "script-ko",
        }
    }
}

pub struct Language {
    /// As the settings file keeps it.
    pub code: &'static str,
    /// As its speakers write it, which is how the picker lists it.
    pub name: &'static str,
    /// In English, beside it.
    pub english: &'static str,
    pub script: Script,
    file: &'static str,
}

impl Language {
    /// The font to show its name in: its own script's, so 简体中文 is all one
    /// font. Needs the fonts installed (`theme::install`).
    pub fn name_font(&self, size: f32) -> egui::FontId {
        match self.script {
            Script::Latin => crate::theme::font(size),
            script => egui::FontId::new(size, egui::FontFamily::Name(script.family().into())),
        }
    }
}

const COUNT: usize = 12;

pub static LANGUAGES: [Language; COUNT] = [
    Language { code: "en", name: "English", english: "English", script: Script::Latin, file: "" },
    Language { code: "es", name: "Español", english: "Spanish", script: Script::Latin, file: include_str!("../translations/es.txt") },
    Language { code: "pt", name: "Português (Brasil)", english: "Portuguese", script: Script::Latin, file: include_str!("../translations/pt.txt") },
    Language { code: "fr", name: "Français", english: "French", script: Script::Latin, file: include_str!("../translations/fr.txt") },
    Language { code: "de", name: "Deutsch", english: "German", script: Script::Latin, file: include_str!("../translations/de.txt") },
    Language { code: "it", name: "Italiano", english: "Italian", script: Script::Latin, file: include_str!("../translations/it.txt") },
    Language { code: "pl", name: "Polski", english: "Polish", script: Script::Latin, file: include_str!("../translations/pl.txt") },
    Language { code: "tr", name: "Türkçe", english: "Turkish", script: Script::Latin, file: include_str!("../translations/tr.txt") },
    Language { code: "ru", name: "Русский", english: "Russian", script: Script::Latin, file: include_str!("../translations/ru.txt") },
    Language { code: "ja", name: "日本語", english: "Japanese", script: Script::Japanese, file: include_str!("../translations/ja.txt") },
    Language { code: "zh", name: "简体中文", english: "Chinese (Simplified)", script: Script::Chinese, file: include_str!("../translations/zh.txt") },
    Language { code: "ko", name: "한국어", english: "Korean", script: Script::Korean, file: include_str!("../translations/ko.txt") },
];

/// The language tooltips are in, as an index into [`LANGUAGES`].
static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Every language's font is loaded, not just the one in use: the picker is
/// open and shows each language's name in its own letters.
static EVERY_SCRIPT: AtomicBool = AtomicBool::new(false);

static TABLES: [OnceLock<Table>; COUNT] = [const { OnceLock::new() }; COUNT];

/// The language with this code; English for one that is none of them.
pub fn find(code: &str) -> &'static Language {
    LANGUAGES.iter().find(|l| l.code == code).unwrap_or(&LANGUAGES[0])
}

/// The language tooltips are in.
pub fn current() -> &'static Language {
    &LANGUAGES[CURRENT.load(Ordering::Relaxed)]
}

/// Puts tooltips in the language with this `code` (English for an unknown
/// one), and loads the font its letters need. Cheap when nothing changes,
/// so the app calls it every frame with what its settings say.
pub fn set(ctx: &egui::Context, code: &str) {
    let wanted = LANGUAGES.iter().position(|l| l.code == code).unwrap_or(0);
    let before = CURRENT.swap(wanted, Ordering::Relaxed);
    if before != wanted && !EVERY_SCRIPT.load(Ordering::Relaxed) && LANGUAGES[before].script != LANGUAGES[wanted].script {
        crate::theme::install_fonts(ctx);
    }
}

/// Has every language's font loaded, so their names show in their own
/// letters: while the picker or the first run's question lists them.
pub fn load_every_script(ctx: &egui::Context) {
    if !EVERY_SCRIPT.swap(true, Ordering::Relaxed) {
        crate::theme::install_fonts(ctx);
    }
}

/// The language Windows is shown in, if it is one of these; English if not.
pub fn system_language() -> &'static Language {
    find(system_code())
}

#[cfg(windows)]
fn system_code() -> &'static str {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: takes nothing and only reads the user's setting.
    let id = unsafe { GetUserDefaultUILanguage() };
    // The primary language, the low ten bits (Windows' LANG_* values).
    match id & 0x3ff {
        0x0a => "es",
        0x16 => "pt",
        0x0c => "fr",
        0x07 => "de",
        0x10 => "it",
        0x15 => "pl",
        0x1f => "tr",
        0x19 => "ru",
        0x11 => "ja",
        0x04 => "zh",
        0x12 => "ko",
        _ => "en",
    }
}

#[cfg(not(windows))]
fn system_code() -> &'static str {
    // "de_DE.UTF-8" and the like: the first two letters.
    let lang = std::env::var("LANG").unwrap_or_default();
    LANGUAGES.iter().map(|l| l.code).find(|code| lang.starts_with(code)).unwrap_or("en")
}

/// The scripts whose fonts the interface needs now: the language's first, so
/// its characters are drawn in its own font where two fonts have them.
pub(crate) fn scripts() -> Vec<Script> {
    let own = current().script;
    let mut scripts = vec![own];
    if EVERY_SCRIPT.load(Ordering::Relaxed) {
        scripts.extend(Script::OTHERS.into_iter().filter(|&s| s != own));
    }
    scripts.retain(|&s| s != Script::Latin);
    scripts
}

/// `english` in the language tooltips are in, or as it is if it is English,
/// or a sentence the language's file does not have.
pub fn tooltip(english: &str) -> Cow<'_, str> {
    let index = CURRENT.load(Ordering::Relaxed);
    if index == 0 {
        return Cow::Borrowed(english);
    }
    let table = TABLES[index].get_or_init(|| Table::parse(LANGUAGES[index].code, LANGUAGES[index].file));
    match table.translate(english) {
        Some(text) => Cow::Owned(text),
        None => Cow::Borrowed(english),
    }
}

/// `response.on_hover_text(english)`, in the language tooltips are in.
pub fn hover(response: Response, english: &str) -> Response {
    response.on_hover_ui(|ui| {
        // As egui's own on_hover_text: a tooltip as wide as tooltips are.
        ui.set_max_width(ui.spacing().tooltip_width);
        ui.add(egui::Label::new(tooltip(english).as_ref()));
    })
}

/// The Interface card's Tooltip Language: every language by its own name.
/// The choice is `code`; returns whether it changed.
pub fn picker(ui: &mut Ui, code: &mut &'static str, tips: bool) -> bool {
    let chosen = find(code);
    let mut changed = false;
    let width = ui.available_width();
    ui.horizontal(|ui| {
        let label = ui.label(egui::RichText::new("Tooltip Language").font(crate::theme::font(12.0)));
        let combo = egui::ComboBox::from_id_salt("tooltip_language")
            .selected_text(egui::RichText::new(chosen.name).font(chosen.name_font(12.0)))
            .width((width - label.rect.width() - 14.0).max(80.0))
            .show_ui(ui, |ui| {
                // The names are in their own letters: have every font while they show.
                load_every_script(ui.ctx());
                for language in &LANGUAGES {
                    let text = if language.name == language.english {
                        language.name.to_owned()
                    } else {
                        format!("{}  ({})", language.name, language.english)
                    };
                    let text = egui::RichText::new(text).font(language.name_font(12.0));
                    if ui.selectable_label(language.code == chosen.code, text).clicked() && *code != language.code {
                        *code = language.code;
                        changed = true;
                    }
                }
            });
        if tips {
            hover(
                combo.response,
                "Which language the tooltips are shown in. The buttons and cards keep their English names, \
                 so a tooltip names them as they are on screen.",
            );
        }
    });
    changed
}

/// One language's file, read.
struct Table {
    exact: HashMap<String, String>,
    /// Longest English first, so the most particular one matches.
    templates: Vec<Template>,
}

impl Table {
    fn parse(code: &str, file: &str) -> Table {
        let prefix = format!("{code}: ");
        let mut table = Table { exact: HashMap::new(), templates: Vec::new() };
        let mut english = None;
        for line in file.lines() {
            if let Some(text) = line.strip_prefix("en: ") {
                english = Some(unescape(text));
            } else if let Some(text) = line.strip_prefix(&prefix) {
                if let Some(english) = english.take() {
                    table.add(english, unescape(text));
                }
            }
        }
        table.templates.sort_by_key(|t| std::cmp::Reverse(t.pieces.iter().map(|p| p.len()).sum::<usize>()));
        table
    }

    fn add(&mut self, english: String, translated: String) {
        let parts = parts(&english);
        if parts.iter().any(|p| matches!(p, Part::Slot(_))) {
            if let Some(template) = Template::new(&parts, self::parts(&translated)) {
                self.templates.push(template);
            }
        } else {
            self.exact.insert(english, translated);
        }
    }

    /// `text` translated whole: as one sentence of the file, or one made with
    /// `format!` from one of the file's.
    fn lookup(&self, text: &str) -> Option<String> {
        let core = text.trim();
        if core.is_empty() {
            return None;
        }
        let translated =
            self.exact.get(core).cloned().or_else(|| self.templates.iter().find_map(|t| t.fill(core)))?;
        let start = text.len() - text.trim_start().len();
        let end = text.trim_end().len();
        Some(format!("{}{translated}{}", &text[..start], &text[end..]))
    }

    fn translate(&self, text: &str) -> Option<String> {
        if let Some(whole) = self.lookup(text) {
            return Some(whole);
        }
        if text.contains('\n') {
            let mut any = false;
            let lines: Vec<String> = text
                .split('\n')
                .map(|line| match self.lookup(line).or_else(|| self.sentences(line)) {
                    Some(t) => {
                        any = true;
                        t
                    }
                    None => line.to_owned(),
                })
                .collect();
            return any.then(|| lines.join("\n"));
        }
        self.sentences(text)
    }

    /// A line made of sentences the file has: each as long a run of them as
    /// it has whole, and any it does not have left in English.
    fn sentences(&self, text: &str) -> Option<String> {
        let bytes = text.as_bytes();
        let mut cuts: Vec<usize> =
            (1..bytes.len()).filter(|&i| bytes[i] == b' ' && b".?!:)".contains(&bytes[i - 1])).collect();
        if cuts.is_empty() {
            return None;
        }
        cuts.push(text.len());
        let (mut out, mut start, mut any) = (String::new(), 0, false);
        while start < text.len() {
            let found = cuts
                .iter()
                .rev()
                .filter(|&&end| end > start && !(start == 0 && end == text.len()))
                .find_map(|&end| self.lookup(&text[start..end]).map(|t| (end, t)));
            let end = match found {
                Some((end, translated)) => {
                    out.push_str(&translated);
                    any = true;
                    end
                }
                None => {
                    let end = cuts.iter().copied().find(|&c| c > start).unwrap_or(text.len());
                    out.push_str(&text[start..end]);
                    end
                }
            };
            start = end;
            while text[start..].starts_with(' ') {
                out.push(' ');
                start += 1;
            }
        }
        any.then_some(out)
    }
}

#[derive(Debug, PartialEq)]
enum Part {
    Text(String),
    Slot(String),
}

/// A sentence made with `format!`: the English between its placeholders, and
/// the translation to fill with what was there.
struct Template {
    /// One more than there are slots; the first and last may be empty.
    pieces: Vec<String>,
    slots: Vec<String>,
    out: Vec<Part>,
}

impl Template {
    fn new(english: &[Part], out: Vec<Part>) -> Option<Template> {
        let (mut pieces, mut slots) = (vec![String::new()], Vec::new());
        for part in english {
            match part {
                Part::Text(text) => pieces.last_mut()?.push_str(text),
                Part::Slot(name) => {
                    slots.push(name.clone());
                    pieces.push(String::new());
                }
            }
        }
        // Two placeholders side by side cannot be told apart.
        let middle = &pieces[1..pieces.len() - 1];
        let known = out.iter().all(|p| match p {
            Part::Slot(name) => slots.contains(name),
            Part::Text(_) => true,
        });
        (known && middle.iter().all(|p| !p.is_empty())).then_some(Template { pieces, slots, out })
    }

    fn fill(&self, text: &str) -> Option<String> {
        let mut rest = text.strip_prefix(self.pieces[0].as_str())?;
        let mut values = Vec::with_capacity(self.slots.len());
        let last = self.pieces.len() - 1;
        for (i, piece) in self.pieces.iter().enumerate().skip(1) {
            if i == last {
                values.push(rest.strip_suffix(piece.as_str())?);
            } else {
                let at = rest.find(piece.as_str())?;
                values.push(&rest[..at]);
                rest = &rest[at + piece.len()..];
            }
        }
        let mut filled = String::new();
        for part in &self.out {
            match part {
                Part::Text(text) => filled.push_str(text),
                Part::Slot(name) => filled.push_str(values[self.slots.iter().position(|s| s == name)?]),
            }
        }
        Some(filled)
    }
}

/// A file's sentence, with `{name}` and `{0}` as slots and `{{` `}}` as braces.
fn parts(text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("{{") || rest.starts_with("}}") {
            plain.push(c);
            rest = &rest[2..];
            continue;
        }
        if c == '{' {
            let name_len = rest[1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|n| n + 1);
            if let Some(end) = name_len.filter(|&e| e > 1 && rest[e..].starts_with('}')) {
                if !plain.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut plain)));
                }
                parts.push(Part::Slot(rest[1..end].to_owned()));
                rest = &rest[end + 1..];
                continue;
            }
        }
        plain.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if !plain.is_empty() {
        parts.push(Part::Text(plain));
    }
    parts
}

/// A line of a file as the sentence it holds: `\n` is a line break.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# A comment, and a blank line after it.

en: Say what a button does when the pointer rests on it.
es: Muestra qué hace un botón.

en: Keeps the {kept} side and rebuilds the {rebuilt} side.
es: Conserva el lado {kept} y rehace el lado {rebuilt}.

en: Too big: {copies} copies x {0} vertices.
es: Demasiado grande: {0} vértices por {copies} copias.

en: Undo (Ctrl+Z) brings it back.
es: Deshacer (Ctrl+Z) lo recupera.

en: Two lines:\\nthe second.
es: Dos líneas:\\nla segunda.

en: Braces {{like these}} stay {0}.
es: Las llaves {{así}} quedan {0}.
";

    fn table() -> Table {
        Table::parse("es", FILE)
    }

    #[test]
    fn a_sentence_in_the_file_is_translated_whole() {
        let t = table();
        assert_eq!(t.translate("Say what a button does when the pointer rests on it.").unwrap(), "Muestra qué hace un botón.");
        assert_eq!(t.translate("Two lines:\nthe second.").unwrap(), "Dos líneas:\nla segunda.");
        // Space around it, as a label sometimes has, stays around the translation.
        assert_eq!(t.translate("  Undo (Ctrl+Z) brings it back.\n").unwrap(), "  Deshacer (Ctrl+Z) lo recupera.\n");
    }

    #[test]
    fn a_sentence_made_with_format_is_filled_in_around_its_placeholders() {
        let t = table();
        assert_eq!(
            t.translate("Keeps the left side and rebuilds the right side.").unwrap(),
            "Conserva el lado left y rehace el lado right."
        );
        // Placeholders the translation puts in another order.
        assert_eq!(t.translate("Too big: 12 copies x 4000 vertices.").unwrap(), "Demasiado grande: 4000 vértices por 12 copias.");
        assert_eq!(t.translate("Braces {like these} stay 3.").unwrap(), "Las llaves {así} quedan 3.");
    }

    #[test]
    fn a_tooltip_pieced_from_sentences_is_translated_a_sentence_at_a_time() {
        let t = table();
        assert_eq!(
            t.translate("Say what a button does when the pointer rests on it. Undo (Ctrl+Z) brings it back.").unwrap(),
            "Muestra qué hace un botón. Deshacer (Ctrl+Z) lo recupera."
        );
        // One the file does not have stays in English between them.
        assert_eq!(
            t.translate("Something new here. Undo (Ctrl+Z) brings it back.").unwrap(),
            "Something new here. Deshacer (Ctrl+Z) lo recupera."
        );
        assert_eq!(
            t.translate("A first line.\nUndo (Ctrl+Z) brings it back.").unwrap(),
            "A first line.\nDeshacer (Ctrl+Z) lo recupera."
        );
    }

    #[test]
    fn what_the_file_does_not_have_is_left_alone() {
        let t = table();
        assert_eq!(t.translate("Not a sentence the file has."), None);
        assert_eq!(t.translate("Two. Unknown sentences."), None);
        assert_eq!(t.translate(""), None);
        assert_eq!(t.translate("Keeps the left side."), None);
    }

    #[test]
    fn english_is_the_language_until_another_is_chosen() {
        assert_eq!(current().code, "en");
        assert_eq!(tooltip("Say what a button does when the pointer rests on it."), "Say what a button does when the pointer rests on it.");
        assert_eq!(find("xx").code, "en", "a code the app does not know is English");
    }

    #[test]
    fn every_language_file_reads_and_keeps_each_sentences_placeholders() {
        for language in &LANGUAGES[1..] {
            let mut pairs = 0;
            let mut english = None;
            let prefix = format!("{}: ", language.code);
            for (n, line) in language.file.lines().enumerate() {
                let at = format!("translations/{}.txt line {}", language.code, n + 1);
                if let Some(text) = line.strip_prefix("en: ") {
                    assert!(english.is_none(), "{at}: an English line with no translation before it");
                    english = Some(unescape(text));
                } else if let Some(text) = line.strip_prefix(&prefix) {
                    let en = english.take().unwrap_or_else(|| panic!("{at}: a translation with no English"));
                    let slots = |text: &str| {
                        let mut s: Vec<String> =
                            parts(text).into_iter().filter_map(|p| if let Part::Slot(n) = p { Some(n) } else { None }).collect();
                        s.sort();
                        s
                    };
                    assert_eq!(slots(&en), slots(&unescape(text)), "{at}: the placeholders differ");
                    assert!(!text.trim().is_empty(), "{at}: an empty translation");
                    pairs += 1;
                } else {
                    assert!(line.is_empty() || line.starts_with('#'), "{at}: a line that is not en:, {}: or a # note", language.code);
                }
            }
            assert!(english.is_none(), "translations/{}.txt ends on English with no translation", language.code);
            assert!(pairs > 0, "translations/{}.txt has no translations", language.code);
            // The Tooltips switch's own tip, which every language has.
            let table = Table::parse(language.code, language.file);
            let tip = table.translate("Say what a button does when the pointer rests on it.");
            assert!(tip.is_some_and(|t| !t.starts_with("Say")), "translations/{}.txt: the Tooltips tip", language.code);
        }
        assert!(LANGUAGES.iter().any(|l| std::ptr::eq(l, system_language())), "Windows' language is one of them, or English");
    }

    #[test]
    fn every_language_has_a_file_and_a_name_of_its_own() {
        for (i, language) in LANGUAGES.iter().enumerate() {
            assert!(LANGUAGES[..i].iter().all(|l| l.code != language.code && l.name != language.name), "{}", language.code);
            assert_eq!(language.file.is_empty(), i == 0, "only English has no file: {}", language.code);
        }
    }
}
