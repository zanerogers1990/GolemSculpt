//! What the page does for the app: the pen's pressure, picking a file,
//! handing one back as a download, and keeping the scene in the browser's
//! own storage across a refresh. The JavaScript halves live in index.html
//! (`window.golem`); these are the Rust halves.

use std::cell::RefCell;
use std::rc::Rc;

#[cfg(target_arch = "wasm32")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = golem, js_name = penPressure)]
        pub fn pen_pressure() -> f64;
        #[wasm_bindgen(js_namespace = golem, js_name = download)]
        pub fn download(name: &str, bytes: &[u8], mime: &str);
        #[wasm_bindgen(js_namespace = golem, js_name = pickFile, catch)]
        pub async fn pick_file() -> Result<JsValue, JsValue>;
        #[wasm_bindgen(js_namespace = golem, js_name = pickFile, catch)]
        pub async fn pick_file_of(accept: &str) -> Result<JsValue, JsValue>;
        #[wasm_bindgen(js_namespace = golem, js_name = storeScene, catch)]
        pub async fn store_scene(bytes: &[u8]) -> Result<JsValue, JsValue>;
        #[wasm_bindgen(js_namespace = golem, js_name = loadScene, catch)]
        pub async fn load_scene() -> Result<JsValue, JsValue>;
        #[wasm_bindgen(js_namespace = golem, js_name = ready)]
        pub fn ready();
        #[wasm_bindgen(js_namespace = golem, js_name = pref)]
        pub fn pref(key: &str) -> String;
        #[wasm_bindgen(js_namespace = golem, js_name = setPref)]
        pub fn set_pref(key: &str, value: &str);
    }
}

/// The pen's pressure (0 to 1) while a pen is on the screen, `None` for a
/// mouse or a finger.
pub fn pen_pressure() -> Option<f32> {
    #[cfg(target_arch = "wasm32")]
    {
        let p = js::pen_pressure();
        if p >= 0.0 { Some(p.clamp(0.0, 1.0) as f32) } else { None }
    }
    #[cfg(not(target_arch = "wasm32"))]
    None
}

pub fn download(name: &str, bytes: &[u8], mime: &str) {
    #[cfg(target_arch = "wasm32")]
    js::download(name, bytes, mime);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (name, bytes, mime);
}

/// Tells the page the app has started, so it takes its loading note down.
pub fn ready() {
    #[cfg(target_arch = "wasm32")]
    js::ready();
}

/// A small setting kept between visits (the browser's local storage), or
/// an empty string.
pub fn pref(key: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    return js::pref(key);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        String::new()
    }
}

pub fn set_pref(key: &str, value: &str) {
    #[cfg(target_arch = "wasm32")]
    js::set_pref(key, value);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (key, value);
}

/// Something the page will hand over later: a picked file, a stored scene.
pub type Later<T> = Rc<RefCell<Option<T>>>;

/// Opens the browser's file picker. The file (its name and bytes) arrives in
/// the returned slot; nothing arrives if the picker is closed.
pub fn pick_file() -> Later<(String, Vec<u8>)> {
    let slot: Later<(String, Vec<u8>)> = Rc::default();
    #[cfg(target_arch = "wasm32")]
    {
        let out = slot.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(value) = js::pick_file().await {
                if let Some(picked) = name_and_bytes(&value) {
                    *out.borrow_mut() = Some(picked);
                }
            }
        });
    }
    slot
}

/// The file picker offering only the kinds `accept` names (".png,.jpg").
pub fn pick_file_of(accept: &str) -> Later<(String, Vec<u8>)> {
    let slot: Later<(String, Vec<u8>)> = Rc::default();
    #[cfg(target_arch = "wasm32")]
    {
        let out = slot.clone();
        let accept = accept.to_owned();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(value) = js::pick_file_of(&accept).await {
                if let Some(picked) = name_and_bytes(&value) {
                    *out.borrow_mut() = Some(picked);
                }
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = accept;
    slot
}

/// Reads a file dropped on the page; it arrives in the returned slot.
pub fn read_dropped(file: egui::DroppedFileHandle) -> Later<(String, Vec<u8>)> {
    let slot: Later<(String, Vec<u8>)> = Rc::default();
    let name = file.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    #[cfg(target_arch = "wasm32")]
    {
        let out = slot.clone();
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(bytes) = file.bytes_async().await {
                *out.borrow_mut() = Some((name, bytes));
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(bytes) = file.bytes() {
        *slot.borrow_mut() = Some((name, bytes));
    }
    slot
}

#[cfg(target_arch = "wasm32")]
fn name_and_bytes(value: &wasm_bindgen::JsValue) -> Option<(String, Vec<u8>)> {
    use wasm_bindgen::JsCast;
    let array = value.dyn_ref::<js_sys::Array>()?;
    let name = array.get(0).as_string()?;
    let bytes = array.get(1).dyn_into::<js_sys::Uint8Array>().ok()?.to_vec();
    Some((name, bytes))
}

pub fn store_scene(bytes: Vec<u8>) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        let _ = js::store_scene(&bytes).await;
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = bytes;
}

/// The scene kept from the last visit, if there is one. `Some(None)` in the
/// slot means the page looked and there was none.
pub fn load_scene() -> Later<Option<Vec<u8>>> {
    let slot: Later<Option<Vec<u8>>> = Rc::default();
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        let out = slot.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let bytes = js::load_scene()
                .await
                .ok()
                .and_then(|v| v.dyn_into::<js_sys::Uint8Array>().ok())
                .map(|a| a.to_vec());
            *out.borrow_mut() = Some(bytes);
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        *slot.borrow_mut() = Some(None);
    }
    slot
}
