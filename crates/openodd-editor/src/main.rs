use wasm_bindgen::JsCast;

/// The page's element the studio mounts in: `#openodd-studio` when a page
/// embeds it, else the body.
fn main() {
    console_error_panic_hook::set_once();
    let host = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("openodd-studio"))
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
    match host {
        Some(el) => leptos::mount::mount_to(el, openodd_editor::app::App).forget(),
        None => leptos::mount::mount_to_body(openodd_editor::app::App),
    }
}
