//! The fit's Web Worker: it says it is ready, receives a [`Job`] as JSON,
//! fits, and posts [`Reply`]s back -- progress at most every 50 ms, then the
//! result -- so a fit of many rows leaves the page free. Trunk builds it
//! beside the editor (`index.html`: `data-type="worker"`, with a loader shim
//! the page starts: `fit_worker_loader.js`).

use openodd_editor::fit_job::{run, Job, Reply};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

/// The least time between two progress messages, in milliseconds.
const EVERY_MS: f64 = 50.0;

fn post(scope: &DedicatedWorkerGlobalScope, reply: &Reply) {
    if let Ok(text) = serde_json::to_string(reply) {
        let _ = scope.post_message(&JsValue::from_str(&text));
    }
}

fn main() {
    console_error_panic_hook::set_once();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let me = scope.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
        let text = ev.data().as_string().unwrap_or_default();
        let reply = match serde_json::from_str::<Job>(&text) {
            Ok(job) => {
                let mut last = f64::NEG_INFINITY;
                run(&job, &mut |p| {
                    // Built and sent only when it is time to: the fit tells
                    // every iteration.
                    let now = js_sys::Date::now();
                    if p.fraction >= 1.0 || now - last >= EVERY_MS {
                        last = now;
                        post(&me, &Reply::Progress(p.clone()));
                    }
                })
            }
            Err(e) => Reply::Failed {
                error: format!("the fit's worker cannot read its job: {e}"),
            },
        };
        post(&me, &reply);
    });
    scope.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();
    post(&scope, &Reply::Ready);
}
