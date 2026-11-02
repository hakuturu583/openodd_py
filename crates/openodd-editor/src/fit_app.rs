//! Fitting the rules to data, in the browser: load measured rows as JSON,
//! see which of them each factor of the graph can read, and fit every
//! factor's distribution to them by maximum likelihood (openodd's
//! `rules_fit`, the same code the Python package runs). What a part's
//! `fixed:` holds stays as written; the fitted values are written into the
//! document, and one step undoes it.
//!
//! The fit runs in a Web Worker (`src/bin/fit_worker.rs`), so the page stays
//! free, and shows how far it has come and about how long is left; it can
//! be cancelled. Where no worker starts (a page embedding the studio without
//! the worker's files), it runs on the page's thread instead.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use openodd::rules::RuleSet;
use openodd::rules_fit::{observations_from_json, Observations};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{ErrorEvent, HtmlInputElement, MessageEvent, Worker};

use crate::app::event_target;
use crate::fit_job::{self, Job, QuantityReport, Reply};
use crate::icons::Icon;
use crate::io;
use crate::rules_app::{rctx, RCtx};
use crate::rules_doc::{num, short, RulesDoc};

/// The worker's script, beside the page (Trunk's loader shim for
/// `fit_worker`).
const WORKER: &str = "./fit_worker_loader.js";

const DATA_KEY: &str = "openodd-editor/fit-data";
const DATA_NAME_KEY: &str = "openodd-editor/fit-data-name";

/// The fit's state, shared by its views.
#[derive(Clone, Copy)]
pub struct FCtx {
    /// The data loaded: (file name, JSON).
    pub data: RwSignal<Option<(String, String)>>,
    /// The rows of the data, as the rules read them, or why they do not read.
    pub observations: Memo<Option<Result<Observations, String>>>,
    /// The last fit's report.
    pub report: RwSignal<Option<Vec<QuantityReport>>>,
    /// The document before the last fit: what undo restores.
    pub before: RwSignal<Option<RulesDoc>>,
    pub running: RwSignal<bool>,
    pub min_spread: RwSignal<f64>,
    /// How far the running fit has come: (fraction, quantity, index, count).
    pub progress: RwSignal<Option<(f64, String, usize, usize)>>,
    /// When it started (`Date.now()`, ms).
    pub started: RwSignal<f64>,
    /// It runs on the page's thread: no progress until it ends.
    pub on_thread: RwSignal<bool>,
    worker: StoredValue<Option<Running>, LocalStorage>,
}

/// A fit running in a worker, and the handlers that must live as long.
struct Running {
    worker: Worker,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _error: Closure<dyn FnMut(ErrorEvent)>,
}

pub fn fctx() -> FCtx {
    expect_context::<FCtx>()
}

pub fn fit_context() -> FCtx {
    let r = rctx();
    let data = RwSignal::new(
        io::load_text(DATA_KEY).map(|t| (io::load_text(DATA_NAME_KEY).unwrap_or_default(), t)),
    );
    let observations = Memo::new(move |_| {
        let (_, text) = data.get()?;
        let names = r.good.with(|g| g.as_ref().map(|s| s.data_names()))?;
        Some(observations_from_json(&text, &names).map_err(|e| e.to_string()))
    });
    // Keep the data between visits, as the rules are kept.
    Effect::new(move |_| {
        if let Some((name, text)) = data.get() {
            io::save_text(DATA_NAME_KEY, &name);
            io::save_text(DATA_KEY, &text);
        }
    });
    FCtx {
        data,
        observations,
        report: RwSignal::new(None),
        before: RwSignal::new(None),
        running: RwSignal::new(false),
        min_spread: RwSignal::new(1e-3),
        progress: RwSignal::new(None),
        started: RwSignal::new(0.0),
        on_thread: RwSignal::new(false),
        worker: StoredValue::new_local(None),
    }
}

impl FCtx {
    /// Stop the worker, if one runs; its handlers go once this call is done.
    fn stop_worker(&self) {
        let mut taken = None;
        self.worker.update_value(|w| taken = w.take());
        if let Some(running) = taken {
            running.worker.terminate();
            // Not dropped here: this may run inside one of its handlers.
            set_timeout(move || drop(running), Duration::ZERO);
        }
    }

    fn end(&self) {
        self.running.set(false);
        self.progress.set(None);
        self.on_thread.set(false);
    }

    /// Fit `set` (the rules as `job` writes them) to the job's data: in a
    /// worker, else on the page's thread.
    fn start(&self, r: RCtx, set: RuleSet, job: Job) {
        let f = *self;
        f.running.set(true);
        f.on_thread.set(false);
        f.progress.set(Some((0.0, String::new(), 0, 0)));
        f.started.set(js_sys::Date::now());
        let Ok(worker) = Worker::new(WORKER) else {
            return f.run_here(r, set, job);
        };
        let heard = RwSignal::new(false);
        let (set_m, job_e, set_e) = (set.clone(), job.clone(), set);
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            heard.set(true);
            let text = ev.data().as_string().unwrap_or_default();
            match serde_json::from_str::<Reply>(&text) {
                Ok(Reply::Progress {
                    fraction,
                    quantity,
                    index,
                    count,
                }) => f.progress.set(Some((fraction, quantity, index, count))),
                Ok(done) => {
                    f.stop_worker();
                    f.finish(r, &set_m, done);
                }
                Err(e) => {
                    f.stop_worker();
                    f.finish(
                        r,
                        &set_m,
                        Reply::Failed {
                            error: e.to_string(),
                        },
                    );
                }
            }
        });
        let error = Closure::<dyn FnMut(ErrorEvent)>::new(move |ev: ErrorEvent| {
            f.stop_worker();
            if heard.get_untracked() {
                f.finish(
                    r,
                    &set_e,
                    Reply::Failed {
                        error: ev.message(),
                    },
                );
            } else {
                // The worker never started (its script is not beside the
                // page): fit here instead.
                f.run_here(r, set_e.clone(), job_e.clone());
            }
        });
        worker.set_onmessage(Some(message.as_ref().unchecked_ref()));
        worker.set_onerror(Some(error.as_ref().unchecked_ref()));
        let sent = serde_json::to_string(&job)
            .map_err(|e| e.to_string())
            .and_then(|t| {
                worker
                    .post_message(&JsValue::from_str(&t))
                    .map_err(|_| "cannot reach the fit's worker".to_string())
            });
        f.worker.set_value(Some(Running {
            worker,
            _message: message,
            _error: error,
        }));
        if let Err(e) = sent {
            f.stop_worker();
            f.finish(r, &RuleSet::default(), Reply::Failed { error: e });
        }
    }

    /// Fit on the page's thread: it waits, so no progress is shown.
    fn run_here(&self, r: RCtx, set: RuleSet, job: Job) {
        let f = *self;
        f.on_thread.set(true);
        // Let the page show that it runs before the fit takes the thread.
        set_timeout(
            move || {
                let reply = fit_job::run(&job, &mut |_| {});
                f.finish(r, &set, reply);
            },
            Duration::from_millis(30),
        );
    }

    /// Write a finished fit into the document (`set`: the rules it fitted).
    fn finish(&self, r: RCtx, set: &RuleSet, reply: Reply) {
        match reply {
            Reply::Done { overlay, report } => match fit_job::fitted(set, &overlay) {
                Ok(fitted) => {
                    let names: Vec<String> = report.iter().map(|q| q.name.clone()).collect();
                    self.before.set(Some(r.doc.get_untracked()));
                    r.doc.update(|d| d.apply_fitted(&fitted, &names));
                    let secs = (js_sys::Date::now() - self.started.get_untracked()) / 1000.0;
                    r.notice.set(Some((
                        true,
                        match names.len() {
                            0 => "Nothing to fit: no factor has a row it reads".to_string(),
                            1 => format!("Fitted {} in {}", names[0], duration(secs)),
                            n => format!("Fitted {n} quantities in {}", duration(secs)),
                        },
                    )));
                    self.report.set(Some(report));
                }
                Err(e) => r.notice.set(Some((false, format!("Fit: {e}")))),
            },
            Reply::Failed { error } => r.notice.set(Some((false, format!("Fit: {error}")))),
            Reply::Progress { .. } => return,
        }
        self.end();
    }
}

/// `secs` as people read it: `0.4 s`, `12 s`, `3 min 5 s`.
fn duration(secs: f64) -> String {
    if secs < 10.0 {
        format!("{:.1} s", secs.max(0.0))
    } else if secs < 60.0 {
        format!("{:.0} s", secs)
    } else {
        let s = secs.round() as u64;
        format!("{} min {} s", s / 60, s % 60)
    }
}

const EXAMPLE: &str = r#"{
  "rows": [
    {"water": 70, "fog": 10, "tire_friction": 0.62, "visibility": 310},
    {"water": 20, "tire_friction": 0.85},
    {"fog": 80, "visibility": null, "dt": 0.5}
  ],
  "weight": "dt"
}"#;

/// The fit's card: the data, what each factor can read of it, and the fit.
#[component]
pub fn FitCard() -> impl IntoView {
    let r = rctx();
    let f = fctx();
    let load = move |ev: leptos::ev::Event| {
        let input: HtmlInputElement = event_target(&ev);
        spawn_local(async move {
            if let Some((name, text)) = io::read_files(&input).await.into_iter().next() {
                f.report.set(None);
                f.data.set(Some((name, text)));
            }
        });
    };
    let clear = move |_| {
        f.data.set(None);
        f.report.set(None);
        io::save_text(DATA_KEY, "");
    };
    let run = move |_| {
        let (Ok(set), Ok(yaml)) = (r.checked.get_untracked(), r.yaml.get_untracked()) else {
            r.notice.set(Some((
                false,
                "The rules do not read: fix them first".into(),
            )));
            return;
        };
        let Some((_, data)) = f.data.get_untracked() else {
            return;
        };
        let job = Job {
            yaml,
            name: r.name.get_untracked(),
            library: r.library.get_untracked(),
            data,
            min_spread: f.min_spread.get_untracked(),
        };
        f.start(r, set, job);
    };

    let undo = move |_| {
        if let Some(doc) = f.before.get_untracked() {
            r.doc.set(doc);
            f.before.set(None);
            f.report.set(None);
            r.notice.set(Some((true, "Undid the fit".into())));
        }
    };
    let can_fit = move || {
        !f.running.get()
            && r.checked.with(|c| c.is_ok())
            && matches!(f.observations.get(), Some(Ok(o)) if !o.rows.is_empty())
    };
    view! {
        <section class="card fit-card">
            <div class="card-title">
                <Icon name="flask"/>"Fit to data"
                <span class="hint">"maximum likelihood, factor by factor"</span>
            </div>
            {move || match f.data.get() {
                None => view! {
                    <p class="muted">
                        "Load rows measured in the real world, as JSON: each row the values seen together. "
                        "A row may leave values out; each factor reads the rows that give its quantity and everything it reads."
                    </p>
                }
                .into_any(),
                Some((name, _)) => view! {
                    <div class="fit-file">
                        <Icon name="doc"/><span class="fit-name">{name}</span>
                        <button class="mini" on:click=clear title="Forget the data"><Icon name="x"/></button>
                    </div>
                }
                .into_any(),
            }}
            <div class="fit-actions">
                <label class="button">
                    <Icon name="upload"/>"Load data (JSON)"
                    <input type="file" accept=".json,application/json" class="hidden" on:change=load/>
                </label>
                <button class="button primary" disabled=move || !can_fit() on:click=run>
                    <Icon name="sparkle"/>{move || if f.running.get() { "Fitting…" } else { "Fit" }}
                </button>
                <Show when=move || f.before.with(|b| b.is_some())>
                    <button class="button subtle" on:click=undo>"Undo fit"</button>
                </Show>
            </div>
            <Show when=move || f.running.get()>
                <FitProgress/>
            </Show>
            <DataSummary/>
            <FitReport/>
            <details class="fit-help">
                <summary>"Data format and options"</summary>
                <p class="hint">
                    "A list of rows, or an object with "<code>"rows"</code>" and, optionally, "
                    <code>"weight"</code>": the column weighing each row (1 where a row has none). "
                    "A value left out or "<code>"null"</code>" is not observed; keys the rules do not read are ignored. "
                    "Columns ("<code>"{\"water\": [...], ...}"</code>") read too."
                </p>
                <pre class="fit-example">{EXAMPLE}</pre>
                <p class="hint">
                    "Each parameter at each point a part writes is estimated, but what a part's "
                    <b>"Fit holds"</b>" says stays as written, and a normal's low and high always do. "
                    "A point no row is near keeps its value."
                </p>
                <label class="field">
                    <span>"Least width (min_spread, a share of the values' spread)"</span>
                    <input type="number" step="0.0001" min="0"
                        prop:value=move || num(f.min_spread.get())
                        on:change=move |ev| {
                            if let Ok(v) = event_target_value(&ev).trim().parse::<f64>() {
                                if v > 0.0 { f.min_spread.set(v); }
                            }
                        }/>
                </label>
            </details>
        </section>
    }
}

/// The running fit: a bar of how far it has come, and about how long is left.
#[component]
fn FitProgress() -> impl IntoView {
    let r = rctx();
    let f = fctx();
    let cancel = move |_| {
        f.stop_worker();
        f.end();
        r.notice.set(Some((true, "Cancelled the fit".into())));
    };
    let fraction = move || f.progress.get().map_or(0.0, |p| p.0);
    let width = move || format!("width: {:.1}%", 100.0 * fraction());
    let text = move || {
        if f.on_thread.get() {
            return "Fitting on the page (no worker): it waits until done…".to_string();
        }
        let Some((fr, quantity, index, count)) = f.progress.get() else {
            return String::new();
        };
        let elapsed = (js_sys::Date::now() - f.started.get_untracked()) / 1000.0;
        let which = if count > 0 {
            format!("{quantity} ({} of {count})", index + 1)
        } else {
            "starting".to_string()
        };
        // The time left, from the pace so far, once there is a pace to go by.
        let left = if fr >= 0.03 && elapsed >= 0.3 {
            format!(" · about {} left", duration(elapsed * (1.0 - fr) / fr))
        } else {
            " · estimating the time…".to_string()
        };
        format!("Fitting {which} · {:.0}%{left}", 100.0 * fr)
    };
    view! {
        <div class="fit-progress" role="progressbar" aria-valuemin="0" aria-valuemax="100"
            aria-valuenow=move || format!("{:.0}", 100.0 * fraction())>
            <div class="fit-progress-bar" class:indeterminate=move || f.on_thread.get()>
                <div style=width></div>
            </div>
            <div class="fit-progress-line">
                <span class="hint">{text}</span>
                <Show when=move || !f.on_thread.get()>
                    <button class="mini" on:click=cancel>"Cancel"</button>
                </Show>
            </div>
        </div>
    }
}

/// What the data gives: per variable, the rows observing it; per factor, the
/// rows it reads and those it skips.
#[component]
fn DataSummary() -> impl IntoView {
    let r = rctx();
    let f = fctx();
    move || {
        let obs = match f.observations.get()? {
            Ok(o) => o,
            Err(e) => {
                return Some(view! { <p class="fit-error"><Icon name="warn"/>{e}</p> }.into_any())
            }
        };
        let n = obs.rows.len();
        let usable = r
            .good
            .with(|g| g.as_ref().map(|s| s.usable(&obs.rows)))
            .unwrap_or_default();
        let variables = obs
            .observed
            .iter()
            .map(|(name, k)| {
                let share = if n == 0 { 0.0 } else { *k as f64 / n as f64 };
                let width = format!("width: {:.1}%", 100.0 * share);
                view! {
                    <tr>
                        <td>{name.clone()}</td>
                        <td class="num">{format!("{k}")}</td>
                        <td class="fit-bar-cell"><div class="fit-bar"><div style=width></div></div></td>
                    </tr>
                }
            })
            .collect_view();
        let factors = usable
            .iter()
            .map(|(q, (rows, skipped))| {
                let class = if *rows == 0 { "fit-none" } else { "" };
                let skips = *skipped > 0;
                view! {
                    <tr class=class>
                        <td>{q.clone()}</td>
                        <td class="num">{format!("{rows}")}</td>
                        <td class="num" class:fit-skipped=skips>{format!("{skipped}")}</td>
                    </tr>
                }
            })
            .collect_view();
        let weight = obs
            .weight_column
            .map(|w| format!(", weighed by {w}"))
            .unwrap_or_default();
        Some(
            view! {
                <p class="hint">{format!("{n} rows{weight}")}</p>
                <table class="rules-table fit-table">
                    <thead><tr><th>"Variable"</th><th class="num">"rows giving it"</th><th></th></tr></thead>
                    <tbody>{variables}</tbody>
                </table>
                <table class="rules-table fit-table">
                    <thead><tr>
                        <th>"Factor of"</th>
                        <th class="num" title="Rows giving the quantity and everything it reads">"rows read"</th>
                        <th class="num" title="Rows giving the quantity but missing something it reads">"skipped"</th>
                    </tr></thead>
                    <tbody>{factors}</tbody>
                </table>
            }
            .into_any(),
        )
    }
}

/// The last fit, per quantity.
#[component]
fn FitReport() -> impl IntoView {
    let f = fctx();
    move || {
        let report = f.report.get()?;
        if report.is_empty() {
            return None;
        }
        let rows = report
            .into_iter()
            .map(|q| {
                let estimated: usize = q.parts.iter().map(|p| p.estimated).sum();
                let state = if q.converged { "converged" } else { "stopped: max iterations" };
                let ll = |v: Option<f64>| v.map_or_else(|| "−∞".to_string(), short);
                let title = q
                    .parts
                    .iter()
                    .map(|p| {
                        format!(
                            "{}: {} values estimated, {} of {} points with data",
                            p.name, p.estimated, p.points_with_data, p.points
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                view! {
                    <tr title=title>
                        <td>{q.name.clone()}</td>
                        <td class="num">{format!("{}", q.rows)}</td>
                        <td class="num">{format!("{} → {}", ll(q.log_likelihood_before), ll(q.log_likelihood))}</td>
                        <td class="num">{format!("{estimated}")}</td>
                        <td class:fit-warn=!q.converged>{state}</td>
                    </tr>
                }
            })
            .collect_view();
        Some(view! {
            <h4 class="fit-head">"Last fit"</h4>
            <table class="rules-table fit-table">
                <thead><tr>
                    <th>"Quantity"</th><th class="num">"rows"</th>
                    <th class="num">"log-likelihood"</th><th class="num">"estimated"</th><th></th>
                </tr></thead>
                <tbody>{rows}</tbody>
            </table>
        })
    }
}
