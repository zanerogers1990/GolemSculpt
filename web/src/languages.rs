//! Tooltips in 11 languages besides English, as on the desktop: the
//! Interface card's Tooltip Language, and on a first visit the question of
//! which language they should be in (the desktop's own
//! `ui/first_language.rs`, compiled here as it is). The interface stays in
//! English; the tool kit's `translate` looks each tooltip up.
//!
//! What differs is where the words and letters come from. The desktop has
//! every language's file built in and reads Windows' fonts for Japanese,
//! Chinese and Korean; a page can do neither without making every visitor
//! download all of it. So the page fetches a language's file
//! (`translations/<code>.txt`, beside index.html) when that language is
//! picked, and for Japanese, Chinese and Korean a font cut down to the
//! letters their tooltips use (`fonts/cjk-<code>.otf`, made by
//! `web/fonts/subset_cjk.py` from Noto Sans CJK). Until a file arrives the
//! tooltips stay in English. The choice is kept in the browser
//! (`golem.pref`, key "language"), so the question is asked once.

use std::cell::RefCell;

use eframe::egui;
use sculpt_toolkit::translate::{self, Script};

use super::GolemApp;
use super::ui::theme;
use crate::browser::Later;

// The desktop's question, as it is. It finds the theme as `super::theme`.
#[path = "../../src/ui/first_language.rs"]
#[allow(dead_code)]
mod first_language;

/// The key the browser keeps the choice under.
const PREF: &str = "language";

/// The letters other than Latin, the font each needs, and the language its
/// font is cut for.
const FONTS: [(Script, &str); 3] =
    [(Script::Japanese, "fonts/cjk-ja.otf"), (Script::Chinese, "fonts/cjk-zh.otf"), (Script::Korean, "fonts/cjk-ko.otf")];

struct Languages {
    /// The language tooltips are in, by its code.
    code: &'static str,
    started: bool,
    /// Files on their way: where from, and the slot they arrive in.
    coming: Vec<(String, Later<Option<Vec<u8>>>)>,
    /// Files asked for already (arrived or not), so a missing one is not
    /// asked for every frame.
    asked: Vec<String>,
}

thread_local! {
    static STATE: RefCell<Languages> =
        const { RefCell::new(Languages { code: "en", started: false, coming: Vec::new(), asked: Vec::new() }) };
}

/// The browser's own language, if it is one of the tooltips' (the desktop
/// asks Windows).
fn browser_language() -> &'static str {
    #[cfg(target_arch = "wasm32")]
    {
        let navigator = js_sys::Reflect::get(&js_sys::global(), &"navigator".into()).ok();
        let lang = navigator.and_then(|n| js_sys::Reflect::get(&n, &"language".into()).ok()).and_then(|l| l.as_string());
        let lang = lang.unwrap_or_default().to_ascii_lowercase();
        translate::LANGUAGES.iter().map(|l| l.code).find(|code| lang.starts_with(code)).unwrap_or("en")
    }
    #[cfg(not(target_arch = "wasm32"))]
    "en"
}

/// Fetches a file from beside the page; `None` arrives if it is not there.
fn fetch(url: &str) -> Later<Option<Vec<u8>>> {
    let slot: Later<Option<Vec<u8>>> = Default::default();
    #[cfg(target_arch = "wasm32")]
    {
        let out = slot.clone();
        let url = url.to_owned();
        wasm_bindgen_futures::spawn_local(async move {
            let bytes = fetch_now(&url).await;
            *out.borrow_mut() = Some(bytes);
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = url;
        *slot.borrow_mut() = Some(None);
    }
    slot
}

#[cfg(target_arch = "wasm32")]
async fn fetch_now(url: &str) -> Option<Vec<u8>> {
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    let global = js_sys::global();
    let fetch: Function = Reflect::get(&global, &"fetch".into()).ok()?.dyn_into().ok()?;
    let promise: Promise = fetch.call1(&global, &url.into()).ok()?.dyn_into().ok()?;
    let response = JsFuture::from(promise).await.ok()?;
    if !Reflect::get(&response, &"ok".into()).ok()?.as_bool()? {
        return None;
    }
    let read: Function = Reflect::get(&response, &"arrayBuffer".into()).ok()?.dyn_into().ok()?;
    let promise: Promise = read.call0(&response).ok()?.dyn_into().ok()?;
    let buffer = JsFuture::from(promise).await.ok()?;
    Some(js_sys::Uint8Array::new(&buffer).to_vec())
}

impl Languages {
    /// Asks for `url` once.
    fn want(&mut self, url: String) {
        if !self.asked.contains(&url) {
            self.coming.push((url.clone(), fetch(&url)));
            self.asked.push(url);
        }
    }

    /// Hands over what has arrived; true while something is still coming.
    fn land(&mut self, ctx: &egui::Context) -> bool {
        let mut i = 0;
        while i < self.coming.len() {
            let arrived = self.coming[i].1.borrow_mut().take();
            let Some(bytes) = arrived else {
                i += 1;
                continue;
            };
            let (url, _) = self.coming.remove(i);
            let Some(bytes) = bytes else {
                log::warn!("GolemSculpt: {url} could not be fetched; those tooltips stay in English");
                continue;
            };
            supply(ctx, &url, bytes);
        }
        !self.coming.is_empty()
    }
}

#[cfg(target_arch = "wasm32")]
fn supply(ctx: &egui::Context, url: &str, bytes: Vec<u8>) {
    if let Some(&(script, _)) = FONTS.iter().find(|(_, file)| *file == url) {
        translate::web::supply_font(ctx, script, bytes);
    } else if let Some(code) = url.strip_prefix("translations/").and_then(|f| f.strip_suffix(".txt")) {
        translate::web::supply(code, &String::from_utf8_lossy(&bytes));
        ctx.request_repaint();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn supply(_ctx: &egui::Context, _url: &str, _bytes: Vec<u8>) {}

/// Whether the tooltips of `code` have arrived, and the font of `script`.
fn has_text(code: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    return translate::web::has(code);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = code;
        true
    }
}

fn has_font(script: Script) -> bool {
    #[cfg(target_arch = "wasm32")]
    return translate::web::has_font(script);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = script;
        true
    }
}

/// Asks for what the language `code` needs, and for every script's font
/// when the languages' names are showing in their own letters.
fn fetch_for(state: &mut Languages, code: &'static str, names: bool) {
    if !has_text(code) {
        state.want(format!("translations/{code}.txt"));
    }
    let own = translate::find(code).script;
    for (script, file) in FONTS {
        if (names || script == own) && !has_font(script) {
            state.want(file.to_owned());
        }
    }
}

impl GolemApp {
    /// Once a frame, before the welcome card: the first visit's question,
    /// the language put to the tool kit, and its files fetched. True while
    /// the question is up (the welcome card waits for it).
    pub(super) fn languages(&mut self, ctx: &egui::Context) -> bool {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if !state.started {
                state.started = true;
                let kept = crate::browser::pref(PREF);
                if kept.is_empty() {
                    // The browser's language to start with, as the desktop starts with Windows'.
                    state.code = translate::find(browser_language()).code;
                    first_language::ask(ctx);
                } else {
                    state.code = translate::find(&kept).code;
                }
            }
            let mut asking = first_language::asking(ctx);
            if asking {
                let mut o = crate::doc::Options { tooltip_language: state.code, ..Default::default() };
                first_language::show(ctx, &mut o);
                state.code = o.tooltip_language;
                asking = first_language::asking(ctx);
                if !asking {
                    crate::browser::set_pref(PREF, state.code);
                    ctx.request_repaint();
                }
            }
            translate::set(ctx, state.code);
            let code = state.code;
            fetch_for(&mut state, code, asking);
            if state.land(ctx) {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            asking
        })
    }

    /// The Interface card's Tooltip Language (the tool kit's picker).
    pub(super) fn language_picker(&mut self, ui: &mut egui::Ui, tips: bool) {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let mut code = state.code;
            if translate::picker(ui, &mut code, tips) {
                state.code = code;
                crate::browser::set_pref(PREF, code);
                translate::set(ui.ctx(), code);
            }
            // The list is open: it shows every language's name in its own letters.
            let names = egui::Popup::is_any_open(ui.ctx());
            fetch_for(&mut state, code, names);
            if !has_text(code) {

                theme::hint(ui, "Fetching the tooltips in this language...");
            }
        });
    }
}
