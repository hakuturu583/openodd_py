//! The browser's side: files the user opens and saves, and the draft kept in
//! local storage between visits.

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, HtmlInputElement, Url};

const DRAFT: &str = "openodd-editor/draft";
const NAME: &str = "openodd-editor/name";

/// The files an `<input type="file">` holds, as (name, text), in order.
pub async fn read_files(input: &HtmlInputElement) -> Vec<(String, String)> {
    let mut out = vec![];
    let Some(list) = input.files() else {
        return out;
    };
    for i in 0..list.length() {
        let Some(file) = list.get(i) else { continue };
        if let Ok(text) = JsFuture::from(file.text()).await {
            out.push((file.name(), text.as_string().unwrap_or_default()));
        }
    }
    input.set_value("");
    out
}

/// Offer `text` as a download named `name`.
pub fn download(name: &str, text: &str) {
    let parts = js_sys::Array::of1(&text.into());
    let options = BlobPropertyBag::new();
    options.set_type("application/yaml");
    let Ok(blob) = Blob::new_with_str_sequence_and_options(&parts, &options) else {
        return;
    };
    let Ok(url) = Url::create_object_url_with_blob(&blob) else {
        return;
    };
    let document = web_sys::window().and_then(|w| w.document());
    if let Some(a) = document
        .and_then(|d| d.create_element("a").ok())
        .and_then(|a| a.dyn_into::<HtmlAnchorElement>().ok())
    {
        a.set_href(&url);
        a.set_download(name);
        a.click();
    }
    let _ = Url::revoke_object_url(&url);
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// The draft saved last time: (file name, YAML).
pub fn load_draft() -> Option<(String, String)> {
    let s = storage()?;
    let text = s.get_item(DRAFT).ok()??;
    let name = s.get_item(NAME).ok()?.unwrap_or_else(|| "odd.yaml".into());
    Some((name, text))
}

pub fn save_draft(name: &str, text: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(DRAFT, text);
        let _ = s.set_item(NAME, name);
    }
}
