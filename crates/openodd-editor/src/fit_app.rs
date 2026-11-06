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

use indexmap::IndexMap;
use leptos::prelude::*;
use leptos::task::spawn_local;
use openodd::rules_fit::{
    observations_from_json, Calibration, Observations, Progress, QuantityFit,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{ErrorEvent, HtmlInputElement, MessageEvent, Worker};

use crate::app::event_target;
use crate::fit_job::{self, Job, Reply};
use crate::icons::Icon;
use crate::io;
use crate::rules_app::{parse_num, rctx, RCtx};
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
    /// Per quantity, the rows its factor reads and those it skips, and how
    /// well the rules as they are match those it reads.
    factors: Memo<Option<IndexMap<String, Factor>>>,
    /// The last fit's report.
    pub report: RwSignal<Option<Vec<QuantityFit>>>,
    /// The document before the last fit: what undo restores.
    pub before: RwSignal<Option<RulesDoc>>,
    /// The fit running, if one is.
    pub run: RwSignal<Option<Run>>,
    pub min_spread: RwSignal<f64>,
    worker: StoredValue<Option<Running>, LocalStorage>,
    /// The job a worker gets once it says it is ready.
    job: StoredValue<Option<Job>, LocalStorage>,
}

/// What a quantity's factor makes of the data, before a fit.
#[derive(Clone, Debug, PartialEq)]
struct Factor {
    /// The rows it reads, and those it skips (missing something it reads).
    read: usize,
    skipped: usize,
    /// How well the rules as they are match the rows it reads.
    matches: Option<Calibration>,
}

/// A fit running.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// How far it has come, once the worker says.
    pub progress: Option<Progress>,
    /// It runs on the page's thread: no progress until it ends.
    pub on_thread: bool,
    /// When it started (`Date.now()`, ms).
    pub started: f64,
}

/// A worker, and the handlers that must live as long.
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
        io::load_text(DATA_KEY)
            .filter(|t| !t.is_empty())
            .map(|t| (io::load_text(DATA_NAME_KEY).unwrap_or_default(), t)),
    );
    // The names the rules read: an edit that leaves them be leaves the data
    // unread again.
    let names = Memo::new(move |_| {
        r.good
            .with(|g| g.as_ref().map(|s| (s.data_names(), s.input_labels())))
    });
    let observations = Memo::new(move |_| {
        names.get()?;
        r.good.with(|g| {
            let set = g.as_ref()?;
            data.with(|d| {
                d.as_ref()
                    .map(|(_, text)| observations_from_json(text, set).map_err(|e| e.to_string()))
            })
        })
    });
    let factors = Memo::new(move |_| {
        observations.with(|o| {
            let Some(Ok(o)) = o else { return None };
            r.good.with(|g| {
                let set = g.as_ref()?;
                let mut matches = set.calibration(&o.rows, o.weights()).unwrap_or_default();
                Some(
                    set.usable(&o.rows)
                        .into_iter()
                        .map(|(q, (read, skipped))| {
                            let matches = matches.shift_remove(&q);
                            (
                                q,
                                Factor {
                                    read,
                                    skipped,
                                    matches,
                                },
                            )
                        })
                        .collect(),
                )
            })
        })
    });
    // Keep the data between visits, as the rules are kept (not on the first
    // run: that is the data just loaded from there).
    Effect::new(move |seen: Option<()>| {
        data.with(|d| {
            if let (Some(()), Some((name, text))) = (seen, d) {
                io::save_text(DATA_NAME_KEY, name);
                io::save_text(DATA_KEY, text);
            }
        })
    });
    FCtx {
        data,
        observations,
        factors,
        report: RwSignal::new(None),
        before: RwSignal::new(None),
        run: RwSignal::new(None),
        min_spread: RwSignal::new(1e-3),
        worker: StoredValue::new_local(None),
        job: StoredValue::new_local(None),
    }
}

impl FCtx {
    pub fn running(&self) -> bool {
        self.run.with(Option::is_some)
    }

    /// Stop the worker, if one runs; its handlers go once this call is done.
    fn stop_worker(&self) {
        let mut taken = None;
        self.worker.update_value(|w| taken = w.take());
        self.job.set_value(None);
        if let Some(running) = taken {
            running.worker.terminate();
            // Not dropped here: this may run inside one of its handlers.
            set_timeout(move || drop(running), Duration::ZERO);
        }
    }

    /// Fit as `job` says: in a worker, else on the page's thread.
    fn start(&self, r: RCtx, job: Job) {
        let f = *self;
        f.run.set(Some(Run {
            progress: None,
            on_thread: false,
            started: js_sys::Date::now(),
        }));
        let Ok(worker) = Worker::new(WORKER) else {
            return f.run_here(r);
        };
        f.job.set_value(Some(job));
        let ready = RwSignal::new(false);
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            let text = ev.data().as_string().unwrap_or_default();
            match serde_json::from_str::<Reply>(&text) {
                Ok(Reply::Ready) => {
                    ready.set(true);
                    f.send_job();
                }
                Ok(Reply::Progress(p)) => f.run.update(|run| {
                    if let Some(run) = run {
                        run.progress = Some(p);
                    }
                }),
                Ok(Reply::Done { overlay, report }) => {
                    f.stop_worker();
                    f.finish(r, Ok((overlay, report)));
                }
                Ok(Reply::Failed { error }) => {
                    f.stop_worker();
                    f.finish(r, Err(error));
                }
                Err(e) => {
                    f.stop_worker();
                    f.finish(r, Err(e.to_string()));
                }
            }
        });
        let error = Closure::<dyn FnMut(ErrorEvent)>::new(move |ev: ErrorEvent| {
            f.stop_worker();
            if ready.get_untracked() {
                f.finish(r, Err(ev.message()));
            } else {
                // No worker started (its script is not beside the page):
                // fit here instead.
                f.run_here(r);
            }
        });
        worker.set_onmessage(Some(message.as_ref().unchecked_ref()));
        worker.set_onerror(Some(error.as_ref().unchecked_ref()));
        f.worker.set_value(Some(Running {
            worker,
            _message: message,
            _error: error,
        }));
    }

    /// Give the ready worker its job.
    fn send_job(&self) {
        let mut job = None;
        self.job.update_value(|j| job = j.take());
        let sent = self.worker.with_value(|w| {
            let (Some(w), Some(job)) = (w, job) else {
                return Ok(());
            };
            let text = serde_json::to_string(&job).map_err(|e| e.to_string())?;
            w.worker
                .post_message(&JsValue::from_str(&text))
                .map_err(|_| "cannot reach the fit's worker".to_string())
        });
        if let Err(e) = sent {
            self.stop_worker();
            self.finish(rctx(), Err(e));
        }
    }

    /// Fit on the page's thread, the rules and the data as read already:
    /// it waits, so no progress is shown.
    fn run_here(&self, r: RCtx) {
        let f = *self;
        f.run.update(|run| {
            if let Some(run) = run {
                run.on_thread = true;
            }
        });
        // Let the page show that it runs before the fit takes the thread.
        set_timeout(
            move || {
                let reply = match (r.checked.get_untracked(), f.observations.get_untracked()) {
                    (Ok(set), Some(Ok(obs))) => {
                        fit_job::run_parsed(&set, &obs, f.min_spread.get_untracked(), &mut |_| {})
                    }
                    (Err(e), _) | (_, Some(Err(e))) => Reply::Failed { error: e },
                    _ => Reply::Failed {
                        error: "no data".into(),
                    },
                };
                f.finish(
                    r,
                    match reply {
                        Reply::Done { overlay, report } => Ok((overlay, report)),
                        Reply::Failed { error } => Err(error),
                        _ => Err("the fit did not end".into()),
                    },
                );
            },
            Duration::from_millis(30),
        );
    }

    /// Write a finished fit (its overlay and report) into the document.
    fn finish(&self, r: RCtx, result: Result<(String, Vec<QuantityFit>), String>) {
        let started = self
            .run
            .with_untracked(|run| run.as_ref().map(|run| run.started));
        self.run.set(None);
        let (overlay, report) = match result {
            Ok(done) => done,
            Err(e) => return r.notice.set(Some((false, format!("Fit: {e}")))),
        };
        let before = r.doc.get_untracked();
        let mut doc = before.clone();
        if let Err(e) = doc.apply_overlay(&overlay) {
            return r.notice.set(Some((false, format!("Fit: {e}"))));
        }
        self.before.set(Some(before));
        r.doc.set(doc);
        let secs = started.map_or(0.0, |s| (js_sys::Date::now() - s) / 1000.0);
        r.notice.set(Some((
            true,
            match report.as_slice() {
                [] => "Nothing to fit: no factor has a row it reads".to_string(),
                [q] => format!("Fitted {} in {}", q.name, duration(secs)),
                qs => format!("Fitted {} quantities in {}", qs.len(), duration(secs)),
            },
        )));
        self.report.set(Some(report));
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
        let (true, Ok(yaml)) = (
            r.checked.with_untracked(Result::is_ok),
            r.yaml.get_untracked(),
        ) else {
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
        f.start(r, job);
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
        !f.running()
            && r.checked.with(Result::is_ok)
            && f.observations
                .with(|o| matches!(o, Some(Ok(o)) if !o.rows.is_empty()))
    };
    view! {
        <section class="card fit-card">
            <div class="card-title">
                <Icon name="flask"/>"Fit to data"
                <span class="hint">"maximum likelihood, factor by factor"</span>
            </div>
            {move || match f.data.with(|d| d.as_ref().map(|(name, _)| name.clone())) {
                None => view! {
                    <p class="muted">
                        "Load rows measured in the real world, as JSON: each row the values seen together. "
                        "A row may leave values out; each factor reads the rows that give its quantity and everything it reads."
                    </p>
                }
                .into_any(),
                Some(name) => view! {
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
                    <Icon name="sparkle"/>{move || if f.running() { "Fitting…" } else { "Fit" }}
                </button>
                <Show when=move || f.before.with(|b| b.is_some())>
                    <button class="button subtle" on:click=undo>"Undo fit"</button>
                </Show>
            </div>
            <Show when=move || f.running()>
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
                            if let Some(v) = parse_num(&event_target_value(&ev)).filter(|v| *v > 0.0) {
                                f.min_spread.set(v);
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
        f.run.set(None);
        r.notice.set(Some((true, "Cancelled the fit".into())));
    };
    let on_thread = move || {
        f.run
            .with(|run| run.as_ref().is_some_and(|run| run.on_thread))
    };
    let fraction = move || {
        f.run.with(|run| {
            run.as_ref()
                .and_then(|run| run.progress.as_ref())
                .map_or(0.0, |p| p.fraction)
        })
    };
    let width = move || format!("width: {:.1}%", 100.0 * fraction());
    let text = move || {
        f.run.with(|run| {
            let Some(run) = run else {
                return String::new();
            };
            if run.on_thread {
                return "Fitting on the page (no worker): it waits until done…".to_string();
            }
            let Some(p) = &run.progress else {
                return "Starting…".to_string();
            };
            let elapsed = (js_sys::Date::now() - run.started) / 1000.0;
            // The time left, from the pace so far, once there is a pace to go by.
            let left = if p.fraction >= 0.03 && elapsed >= 0.3 {
                format!(
                    " · about {} left",
                    duration(elapsed * (1.0 - p.fraction) / p.fraction)
                )
            } else {
                " · estimating the time…".to_string()
            };
            format!(
                "Fitting {} ({} of {}) · {:.0}%{left}",
                p.quantity,
                p.index + 1,
                p.count,
                100.0 * p.fraction
            )
        })
    };
    view! {
        <div class="fit-progress" role="progressbar" aria-valuemin="0" aria-valuemax="100"
            aria-valuenow=move || format!("{:.0}", 100.0 * fraction())>
            <div class="fit-progress-bar" class:indeterminate=on_thread>
                <div style=width></div>
            </div>
            <div class="fit-progress-line">
                <span class="hint">{text}</span>
                <Show when=move || !on_thread()>
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
    let f = fctx();
    move || {
        f.observations.with(|obs| {
            let obs = match obs.as_ref()? {
                Ok(o) => o,
                Err(e) => {
                    let e = e.clone();
                    return Some(
                        view! { <p class="fit-error"><Icon name="warn"/>{e}</p> }.into_any(),
                    );
                }
            };
            let n = obs.rows.len();
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
            let factors = f.factors.with(|u| {
                u.iter()
                    .flatten()
                    .map(|(q, Factor { read: rows, skipped, matches })| {
                        let class = if *rows == 0 { "fit-none" } else { "" };
                        let skips = *skipped > 0;
                        let (band, hist) = match matches {
                            Some(c) => (band_cell(c), histogram(c)),
                            None => (view! { <td class="num">"–"</td> }.into_any(), view! { <td></td> }.into_any()),
                        };
                        view! {
                            <tr class=class>
                                <td>{q.clone()}</td>
                                <td class="num">{format!("{rows}")}</td>
                                <td class="num" class:fit-skipped=skips>{format!("{skipped}")}</td>
                                {band}
                                {hist}
                            </tr>
                        }
                    })
                    .collect_view()
            });
            let weight = obs
                .weight
                .as_ref()
                .map(|(w, _)| format!(", weighed by {w}"))
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
                            <th class="num" title=BAND_HELP>"in 90% band"</th>
                            <th title=HIST_HELP>"where rows fall"</th>
                        </tr></thead>
                        <tbody>{factors}</tbody>
                    </table>
                    <p class="hint fit-legend">
                        "How well the rules as they are match: rules that match put 90% of the rows in "
                        "their central 90% and spread the rows evenly over their quantiles (a flat bar). "
                        "A U says too narrow, a hump too wide, a slope off-centre."
                    </p>
                }
                .into_any(),
            )
        })
    }
}

const BAND_HELP: &str = "The share of rows within the distribution's central 90% at their inputs: 90% when the rules match; less when too narrow or off-centre, more when too wide";
const HIST_HELP: &str = "Where each row's value falls in the distribution at its inputs, its quantile, by tenths: flat when the rules match";

/// How a share of rows in the 90% band reads: close to 90% matches.
fn band_class(c: &Calibration) -> &'static str {
    let off = (c.in_band - 0.9).abs();
    if c.outside > 0.0 || off > 0.1 {
        "fit-bad"
    } else if off > 0.03 {
        "fit-meh"
    } else {
        "fit-good"
    }
}

fn band_cell(c: &Calibration) -> AnyView {
    let title = format!(
        "{:.1}% in the central 90%, {:.1}% impossible under the rules; KS distance {:.3} (about {:.3} is chance); log-likelihood per row {}",
        100.0 * c.in_band,
        100.0 * c.outside,
        c.ks,
        1.36 / (c.rows.max(1) as f64).sqrt(),
        if c.mean_log_likelihood.is_finite() { short(c.mean_log_likelihood) } else { "−∞".into() },
    );
    view! {
        <td class=format!("num {}", band_class(c)) title=title>{format!("{:.0}%", 100.0 * c.in_band)}</td>
    }
    .into_any()
}

/// A row's quantiles by tenths, as ten bars: flat when the rules match.
fn histogram(c: &Calibration) -> AnyView {
    let top = c.histogram.iter().cloned().fold(0.1f64, f64::max);
    let bars = c
        .histogram
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let height = 16.0 * h / top;
            view! {
                <rect x=format!("{}", i * 6) y=format!("{:.2}", 16.0 - height)
                    width="5" height=format!("{height:.2}")/>
            }
        })
        .collect_view();
    view! {
        <td class="fit-hist-cell" title=HIST_HELP>
            <svg class=format!("fit-hist {}", band_class(c)) viewBox="0 0 60 17" width="60" height="17" aria-hidden="true">
                // Where each bar stands when the rules match.
                <line x1="0" x2="60" y1=format!("{:.2}", 16.0 - 16.0 * 0.1 / top) y2=format!("{:.2}", 16.0 - 16.0 * 0.1 / top)/>
                {bars}
            </svg>
        </td>
    }
    .into_any()
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
                let ll = |v: f64| if v.is_finite() { short(v) } else { "−∞".to_string() };
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
                        <td class="num" title=format!("log-likelihood {} → {}", ll(q.log_likelihood_before), ll(q.log_likelihood))>
                            {format!("{} → {}", ll(q.calibration_before.mean_log_likelihood), ll(q.calibration.mean_log_likelihood))}
                        </td>
                        <td class="num">
                            <span class=band_class(&q.calibration_before)>{format!("{:.0}%", 100.0 * q.calibration_before.in_band)}</span>
                            " → "
                            <span class=band_class(&q.calibration)>{format!("{:.0}%", 100.0 * q.calibration.in_band)}</span>
                        </td>
                        {histogram(&q.calibration)}
                        <td class="num">{format!("{estimated}")}</td>
                        <td class:fit-warn=!q.converged title=state>{if q.converged { "✓" } else { "!" }}</td>
                    </tr>
                }
            })
            .collect_view();
        Some(view! {
            <h4 class="fit-head">"Last fit"</h4>
            <table class="rules-table fit-table">
                <thead><tr>
                    <th>"Quantity"</th><th class="num">"rows"</th>
                    <th class="num" title="Log-likelihood per row, before → after: higher is likelier">"log-lik / row"</th>
                    <th class="num" title=BAND_HELP>"in 90% band"</th>
                    <th title=HIST_HELP>"after"</th>
                    <th class="num">"estimated"</th><th></th>
                </tr></thead>
                <tbody>{rows}</tbody>
            </table>
        })
    }
}
