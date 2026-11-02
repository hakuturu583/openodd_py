//! Fitting the rules to data, in the browser: load measured rows as JSON,
//! see which of them each factor of the graph can read, and fit every
//! factor's distribution to them by maximum likelihood (openodd's
//! `rules_fit`, the same code the Python package runs). What a part's
//! `fixed:` holds stays as written; the fitted values are written into the
//! document, and one step undoes it.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use openodd::rules_fit::{observations_from_json, FitOptions, Observations, QuantityFit};
use web_sys::HtmlInputElement;

use crate::app::event_target;
use crate::icons::Icon;
use crate::io;
use crate::rules_app::rctx;
use crate::rules_doc::{num, short, RulesDoc};

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
    pub report: RwSignal<Option<Vec<QuantityFit>>>,
    /// The document before the last fit: what undo restores.
    pub before: RwSignal<Option<RulesDoc>>,
    pub running: RwSignal<bool>,
    pub min_spread: RwSignal<f64>,
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
        let Ok(set) = r.checked.get_untracked() else {
            r.notice.set(Some((
                false,
                "The rules do not read: fix them first".into(),
            )));
            return;
        };
        let Some(Ok(obs)) = f.observations.get_untracked() else {
            return;
        };
        f.running.set(true);
        // Let the page show that it runs before the fit takes the thread.
        set_timeout(
            move || {
                let options = FitOptions {
                    min_spread: f.min_spread.get_untracked(),
                    ..FitOptions::default()
                };
                match set.fit(&obs.rows, obs.weights.as_deref(), &options) {
                    Ok(fit) => {
                        let names: Vec<String> =
                            fit.quantities.iter().map(|q| q.name.clone()).collect();
                        f.before.set(Some(r.doc.get_untracked()));
                        r.doc.update(|d| d.apply_fitted(&fit.rules, &names));
                        r.notice.set(Some((
                            true,
                            match names.len() {
                                0 => "Nothing to fit: no factor has a row it reads".to_string(),
                                1 => format!("Fitted {}", names[0]),
                                n => format!("Fitted {n} quantities"),
                            },
                        )));
                        f.report.set(Some(fit.quantities));
                    }
                    Err(e) => r.notice.set(Some((false, format!("Fit: {e}")))),
                }
                f.running.set(false);
            },
            Duration::from_millis(30),
        );
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
